#!/usr/bin/env bash
#
# build_map_assets.sh — produce everything the app bundles for offline Berlin.
#
#   1. data/berlin-latest.osm.pbf     full Geofabrik extract (download, ~70 MB)
#   2. data/berlin-routing.osm.pbf    streets + buildings + surveillance nodes
#                                     only — what the Rust core ingests (the
#                                     scored cache below is built from it)
#   3. data/berlin.pmtiles            vector basemap tiles (Planetiler)
#   4. berlin-routing.graphcache      the scored routing graph built from 2
#
# 3 and 4 are copied into app/src/main/assets/map/ for the APK build. The app
# loads the cache alone (Router::open_cache), so 2 is NOT bundled: it was 59 MB
# of the APK, copied a second time onto the phone. Only if the cache cannot be
# built (SKIP_CACHE=1, no cargo, or a failed pre-build) is 2 bundled instead, so
# that the app still routes, scoring the graph itself on first launch.
#
# Prerequisites: curl, osmium (osmium-tool), md5sum, java 21+ (for Planetiler).
# Run it from anywhere; paths resolve relative to the repo.
#
# Privacy note: this script is the ONE network step of the whole project.
# It runs on your build machine, never on the phone; the app itself makes
# no network requests and declares no INTERNET permission.
#
# Knobs (all optional):
#   EXTRACT_URL=<url>       use a different/mirror source for the extract
#   PLANETILER_VERSION=vX   pin a different Planetiler release
#   SKIP_TILES=1            stop after the routing snapshot
#   SKIP_CHECKSUM=1         proceed without verifying the extract (unsafe)
#   RETRIES=<n>             download attempts per file (default 5)
#   REFRESH=1               discard any cached extract and fetch the CURRENT one,
#                           so the bundled snapshot always has the latest cameras
#   LIVE_CAMERAS=1          pull the surveillance nodes live from OpenStreetMap
#                           (Overpass) and swap them into the routing snapshot,
#                           so the cameras are minutes old instead of as old as
#                           the daily extract. Roads and buildings still come
#                           from the extract. One query per run; if Overpass
#                           fails or looks wrong the extract's own cameras are
#                           kept and the build carries on. Off by default; the
#                           nightly/main/manual release builds turn it on, tagged
#                           (Stable) releases do not.
#   OVERPASS_URL=<url>      use a different Overpass endpoint
#
# Camera freshness: cameras are OSM nodes carried inside the extract, so "latest
# cameras" just means "latest extract". A clean checkout has no cached extract
# and therefore always downloads the current one -- which is exactly what the
# release build does. On a working tree that already has data/berlin-latest.osm.pbf
# from an earlier run, that older extract is reused as-is; pass REFRESH=1 to pull
# the current one instead. Either way the provenance (OSM snapshot date + camera
# count) is written to data/build-info.txt and printed at the end.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DATA="$ROOT/data"
ASSETS="$ROOT/app/src/main/assets/map"
mkdir -p "$DATA" "$ASSETS"

GEOFABRIK="${EXTRACT_URL:-https://download.geofabrik.de/europe/germany/berlin-latest.osm.pbf}"
RAW="$DATA/berlin-latest.osm.pbf"
ROUTING="$DATA/berlin-routing.osm.pbf"
TILES="$DATA/berlin.pmtiles"

OVERPASS_URL="${OVERPASS_URL:-https://overpass-api.de/api/interpreter}"
# Overpass asks API users to identify themselves; this tells its operators who
# to contact and lets them rate-limit us specifically rather than everybody.
USER_AGENT="schattenweg-build/1.0 (+https://github.com/phrag/SchattenWeg; one query per build)"

PLANETILER_VERSION="${PLANETILER_VERSION:-v0.10.2}"
PLANETILER_JAR="$DATA/planetiler-$PLANETILER_VERSION.jar"
PLANETILER_URL="https://github.com/onthegomap/planetiler/releases/download/$PLANETILER_VERSION/planetiler.jar"

# Glyph pack for map labels. The full Noto Sans stack is ~33 MB of all
# Unicode; Berlin only needs the Latin ranges, so just those are bundled.
FONTS_URL="https://github.com/openmaptiles/fonts/releases/download/v2.0/noto-sans.zip"
FONTS_ZIP="$DATA/noto-sans.zip"
# One fontstack is enough; the style references exactly this name.
FONTSTACK="Noto Sans Regular"
# Code-point ranges to keep: Basic Latin through Latin Extended-B and the
# punctuation/symbol blocks a European street map uses. Each file is one
# 256-code-point range named "<start>-<end>.pbf".
GLYPH_RANGES=(0 256 512 768 1024 1280 1536 1792 2048 7936 8192 8448 8704)

RETRIES="${RETRIES:-5}"

die() {
    echo >&2
    echo "ERROR: $*" >&2
    exit 1
}

# Print the MD5 hex of a file. GNU systems have md5sum; stock macOS ships
# BSD md5 instead, so accept either rather than demanding coreutils.
md5_hex() {
    if command -v md5sum >/dev/null 2>&1; then
        md5sum "$1" | awk '{print $1}'
    else
        md5 -q "$1"
    fi
}

# Print the SHA-256 hex of a file. Linux has sha256sum; macOS ships `shasum`.
sha256_hex() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

# Verify file $1 against the SHA-256 a release publishes beside it at URL $2.
# The tiny checksum is fetched fresh every run, so a corrupted or tampered
# download -- including a poisoned build cache holding an old jar -- is caught
# each time, not just on first download. If the checksum host is unreachable, a
# previously fetched copy is reused rather than failing an offline re-run.
verify_sha256() {
    local file="$1" url="$2" what="$3" shafile
    shafile="$file.sha256"
    if curl -fsSL --retry "$RETRIES" --retry-delay 3 \
        --connect-timeout 20 --max-time 60 "$url" -o "$shafile.new" 2>/dev/null; then
        mv "$shafile.new" "$shafile"
    else
        rm -f "$shafile.new"
        if [[ -f "$shafile" ]] && grep -qE '^[0-9a-fA-F]{64}' "$shafile"; then
            echo "  (could not refetch $what checksum; using cached copy)"
        else
            die "have $what but could not fetch its checksum from
  $url
Re-run when the host is reachable, or verify by hand and compare against
$url:
  sha256sum $file"
        fi
    fi
    local published actual
    published="$(awk '{print $1}' "$shafile")"
    actual="$(sha256_hex "$file")"
    if [[ -z "$published" || "$published" != "$actual" ]]; then
        die "checksum mismatch for $file
  published: ${published:-<none found>}
  actual:    $actual
The download is corrupt or was tampered with. Delete it and re-run:
  rm $file && $0"
    fi
    echo "  $what checksum OK ($actual)"
}

# Download to a .part file and move it into place only on success, so an
# interrupted or failed transfer never leaves a truncated file that the next
# run mistakes for a complete one. Retries cover transient 5xx (502/503/504),
# 429 and connection failures — exactly the class of failure that makes a
# large download flaky.
fetch() {
    local url="$1" out="$2" what="$3"
    local part="$out.part"
    local code rc=0 diagnosis

    echo "Downloading $what..."
    echo "  $url"
    # stderr is left alone so the progress meter and curl's own error line
    # reach the terminal; we classify from the exit code and HTTP status.
    code=$(curl -fL --show-error \
        --retry "$RETRIES" --retry-delay 3 --retry-connrefused \
        --connect-timeout 20 --speed-limit 1024 --speed-time 30 \
        -w '%{http_code}' \
        -C - -o "$part" "$url") || rc=$?

    if (( rc == 0 )); then
        mv "$part" "$out"
        return 0
    fi
    rm -f "$part"
    case "$code" in
        5*) diagnosis="HTTP $code -- the server answered but failed. That is
load or maintenance at their end, not a problem with your setup, and it
usually clears on its own within minutes." ;;
        429) diagnosis="HTTP 429 -- rate limited. Wait a few minutes." ;;
        404) diagnosis="HTTP 404 -- not found. The URL moved, or a version
pinned in this script has gone stale." ;;
        401 | 403) diagnosis="HTTP $code -- access denied. Usually a proxy or
network filter rather than the host itself." ;;
        000 | "")
            case "$rc" in
                6) diagnosis="DNS lookup failed (curl 6)." ;;
                7) diagnosis="Could not connect (curl 7) -- firewall or proxy." ;;
                28) diagnosis="Timed out (curl 28)." ;;
                35 | 60) diagnosis="TLS failed (curl $rc) -- often TLS
interception by a corporate proxy presenting its own certificate." ;;
                *) diagnosis="No HTTP response reached curl (exit $rc). A proxy
can reject with its own status -- 403 or 502 -- before the real server is ever
contacted, so the status in curl's line above may be the proxy's, not the
download host's." ;;
            esac
            ;;
        *) diagnosis="HTTP $code (curl exit $rc)." ;;
    esac

    die "could not download $what.
  $url
$diagnosis

What you can do:
  * re-run; finished files are kept, so only what's missing is fetched
  * use a mirror:  EXTRACT_URL=<url> $0
    Known alternatives for city extracts:
      https://download.bbbike.org/osm/bbbike/Berlin/Berlin.osm.pbf
      https://download.openstreetmap.fr/extracts/europe/germany/
  * fetch the file by hand into $DATA and re-run
  * skip tiles for now and still get a routable app:  SKIP_TILES=1 $0"
}

# --- 0. Preflight ------------------------------------------------------------
# Check every tool up front: discovering a missing one after a 94 MB download
# (or twenty minutes into tiling) wastes real time.
missing=()
for tool in curl osmium; do
    command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
done
command -v md5sum >/dev/null 2>&1 || command -v md5 >/dev/null 2>&1 \
    || missing+=("md5sum or md5")

JAVA_MIN=21
if [[ "${SKIP_TILES:-0}" != "1" ]]; then
    if ! command -v java >/dev/null 2>&1; then
        missing+=("java (${JAVA_MIN}+, for Planetiler)")
    else
        # "21.0.10" -> 21; legacy "1.8.0_302" -> 1, correctly below the bar.
        java_major="$(java -version 2>&1 | awk -F'"' '/version/ {print $2}' \
            | cut -d. -f1)"
        if [[ "$java_major" =~ ^[0-9]+$ ]] && (( java_major < JAVA_MIN )); then
            die "Planetiler needs Java $JAVA_MIN+, but 'java' here is $java_major.
  macOS:          brew install openjdk@$JAVA_MIN
  Debian/Ubuntu:  sudo apt-get install openjdk-${JAVA_MIN}-jre
Then re-run -- the extract is already downloaded, so this is quick.
Or skip the basemap for now:  SKIP_TILES=1 $0"
        fi
    fi
fi

if (( ${#missing[@]} > 0 )); then
    die "missing required tool(s): ${missing[*]}
  macOS:          brew install osmium-tool curl coreutils openjdk@$JAVA_MIN
  Debian/Ubuntu:  sudo apt-get install osmium-tool curl coreutils default-jre
Nothing has been downloaded yet, so install these and re-run."
fi

# --- 1. Geofabrik extract ----------------------------------------------------
# REFRESH=1 drops the cached extract (and its checksum) so the newest published
# extract is fetched and re-verified. Without it, an extract already on disk is
# reused -- fast, but as old as the day it was downloaded.
if [[ "${REFRESH:-0}" == "1" && -f "$RAW" ]]; then
    echo "REFRESH=1 -- discarding cached extract to fetch the current one."
    rm -f "$RAW" "$RAW.md5"
fi

if [[ -f "$RAW" ]]; then
    echo "Using existing $RAW (pass REFRESH=1 to fetch the current extract)"
else
    fetch "$GEOFABRIK" "$RAW" "the Berlin OSM extract (~70 MB)"
fi

# Integrity: Geofabrik publishes an .md5 next to every extract. Verify the
# download instead of trusting the pipe blindly.
if [[ "${SKIP_CHECKSUM:-0}" == "1" ]]; then
    echo "!! SKIP_CHECKSUM=1 -- using $RAW UNVERIFIED."
elif [[ -n "${EXTRACT_URL:-}" ]]; then
    echo "Custom EXTRACT_URL set; skipping the Geofabrik checksum."
    echo "   Verify your source yourself -- see SECURITY.md."
else
    md5file="$RAW.md5"

    # Reuse a previously fetched checksum. The extract is only downloaded when
    # absent, so a cached checksum still describes the file on disk -- and a
    # re-run after an interrupted build should not depend on the network at
    # all. Anything that is not a bare 32-hex-digit line is treated as a failed
    # or partial download and refetched.
    if [[ -f "$md5file" ]] \
        && grep -qE '^[0-9a-fA-F]{32}([[:space:]]|$)' "$md5file"; then
        echo "Verifying extract checksum (using cached $(basename "$md5file"))..."
    else
        echo "Fetching published checksum..."
        # --max-time bounds the whole request: this file is ~50 bytes, so any
        # transfer still running after a minute is a stalled connection, not a
        # slow one. Without it curl waits indefinitely on a server that accepts
        # the connection and then goes quiet, which looks exactly like a hang.
        if ! curl -fsSL --retry "$RETRIES" --retry-delay 3 \
            --connect-timeout 20 --max-time 60 \
            "$GEOFABRIK.md5" -o "$md5file"; then
            rm -f "$md5file"
            die "downloaded the extract but could not fetch its checksum from
  $GEOFABRIK.md5
The extract itself is fine and is kept, so a re-run is quick. If that host
stays unresponsive you have two honest options:
  * verify by hand:  md5_hex is just md5sum/md5 -- compare
      md5sum $RAW
    against the checksum published beside the extract, then
      SKIP_CHECKSUM=1 $0
  * skip verification knowing what that means:  SKIP_CHECKSUM=1 $0"
        fi
        echo "Verifying extract checksum..."
    fi

    # The published file is "<md5>  berlin-latest.osm.pbf"; we only want the hex.
    published="$(awk '{print $1}' "$md5file")"
    actual="$(md5_hex "$RAW")"
    if [[ -z "$published" || "$published" != "$actual" ]]; then
        die "checksum mismatch for $RAW
  published: ${published:-<none found>}
  actual:    $actual
The download is corrupt or was tampered with. Delete both and re-run:
  rm $RAW $md5file && $0"
    fi
    echo "  OK ($actual)"
fi

# --- 2. Routing + camera snapshot -------------------------------------------
# Buildings (w/building, r/building) are kept because a wall hides the street
# behind it from a camera: the core clips each camera's coverage by them (see
# core/src/occluders.rs). tags-filter pulls in the nodes of every kept way and
# the member ways of every kept relation, which is what multipolygon buildings
# need. The core prunes the buildings to those near a camera at load time and
# the cache stores only that subset, but this file carries all of them. It is no
# longer bundled (see the top of this file), so its size only costs build time.
echo "Filtering to walkable streets + buildings + surveillance nodes..."
osmium tags-filter --overwrite "$RAW" \
    w/highway w/building r/building n/man_made=surveillance \
    -o "$ROUTING" \
    || die "osmium could not filter $RAW (is the file complete?)"
osmium fileinfo -e "$ROUTING" | sed -n 's/^  Number of/  /p' || true
echo "-> $ROUTING"

# --- 2a. Live cameras (optional) ---------------------------------------------
# The Geofabrik extract is cut once a day, so its cameras can be a day old. With
# LIVE_CAMERAS=1 the surveillance nodes are taken from Overpass instead and
# swapped into the routing snapshot: roads and buildings stay from the extract.
# This must never break a build, so every failure path below falls back to the
# extract's own cameras and says so.
camera_source="geofabrik"
camera_snapshot=""

# Count <node> elements of an Overpass XML file (Overpass writes one per line).
count_xml_nodes() { grep -c '^[[:space:]]*<node ' "$1" || true; }

# Fetch Berlin's surveillance nodes into $1 (OSM XML). One query, retried with
# exponential backoff on failure/overload; prints the data timestamp (Overpass'
# osm_base, i.e. how fresh its database is) to stdout on success.
fetch_live_cameras() {
    local out="$1" attempt=1 delay=10 code rc base
    local query='[out:xml][timeout:180];
area["boundary"="administrative"]["admin_level"="4"]["name"="Berlin"]->.b;
node["man_made"="surveillance"](area.b);
out meta;'
    while (( attempt <= 4 )); do
        rc=0
        code=$(curl -sS --max-time 240 --connect-timeout 20 \
            -A "$USER_AGENT" --data-urlencode "data=$query" \
            -w '%{http_code}' -o "$out" "$OVERPASS_URL") || rc=$?
        # A 200 can still carry a runtime error (timeout, out of memory) as a
        # <remark>; a truncated body lacks the closing tag.
        if (( rc == 0 )) && [[ "$code" == "200" ]] \
            && grep -q '</osm>' "$out" \
            && ! grep -qi '<remark>.*error' "$out"; then
            base="$(sed -nE 's/.*<meta osm_base="([^"]+)".*/\1/p' "$out" | head -1)"
            if [[ -n "$base" ]]; then
                echo "$base"
                return 0
            fi
            echo "  Overpass answered without a data timestamp" >&2
        else
            echo "  Overpass attempt $attempt failed (curl $rc, HTTP ${code:-none})" >&2
        fi
        if (( attempt < 4 )); then
            echo "  retrying in ${delay}s" >&2
            sleep "$delay"
            delay=$((delay * 3))
        fi
        attempt=$((attempt + 1))
    done
    return 1
}

if [[ "${LIVE_CAMERAS:-0}" == "1" ]]; then
    echo "LIVE_CAMERAS=1 -- fetching current cameras from Overpass (one query)..."
    mkdir -p "$DATA/tmp"
    live_xml="$DATA/tmp/live-cameras.osm"
    live_pbf="$DATA/tmp/live-cameras.osm.pbf"
    nocam_pbf="$DATA/tmp/routing-nocam.osm.pbf"
    merged_pbf="$DATA/tmp/routing-merged.osm.pbf"
    rm -f "$live_xml" "$live_pbf" "$nocam_pbf" "$merged_pbf"

    extract_cams=""
    if osmium tags-filter --overwrite "$ROUTING" n/man_made=surveillance \
        -o "$DATA/tmp/extract-cams.osm.pbf" 2>/dev/null; then
        extract_cams="$(osmium fileinfo -e -g data.count.nodes \
            "$DATA/tmp/extract-cams.osm.pbf" 2>/dev/null | head -1 || true)"
    fi
    rm -f "$DATA/tmp/extract-cams.osm.pbf"

    if live_ts="$(fetch_live_cameras "$live_xml")"; then
        live_cams="$(count_xml_nodes "$live_xml")"
        echo "  Overpass returned $live_cams surveillance nodes (data as of $live_ts);" \
            "the extract has ${extract_cams:-unknown}"
        # Sanity floor: Berlin has thousands of cameras and they don't vanish
        # overnight. A tiny answer means a broken response, not a real edit.
        floor=1
        [[ "$extract_cams" =~ ^[0-9]+$ ]] && floor=$(( extract_cams / 2 ))
        if (( live_cams < floor || live_cams == 0 )); then
            echo "WARNING: live camera count $live_cams is under half the extract's" \
                "(${extract_cams:-?}); distrusting it, keeping the extract's cameras"
        elif osmium tags-filter --overwrite --invert-match "$ROUTING" \
                n/man_made=surveillance -o "$nocam_pbf" \
            && osmium sort --overwrite "$live_xml" -o "$live_pbf" \
            && osmium merge --overwrite "$nocam_pbf" "$live_pbf" -o "$merged_pbf" \
            && [[ -s "$merged_pbf" ]]; then
            mv "$merged_pbf" "$ROUTING"
            camera_source="overpass"
            camera_snapshot="$live_ts"
            echo "-> swapped $live_cams live cameras into $ROUTING"
        else
            echo "WARNING: could not merge the live cameras; keeping the extract's"
        fi
    else
        echo "WARNING: Overpass unavailable after retries; the build uses the" \
            "extract's own cameras (as old as the extract)"
    fi
    rm -f "$live_xml" "$live_pbf" "$nocam_pbf" "$merged_pbf"
fi

# Pre-build the scored-graph cache with the same core code the app runs. It IS
# the app's routing data: the cameras, places, street graph (already scored) and
# the building outlines near cameras are all in it, so the app opens it without
# the extract (Router::open_cache). It is checked for the format version and
# the zone table, not for the extract's contents, because the extract is not
# shipped. SKIP_CACHE=1 skips this step (and then the extract is bundled).
CACHE_ASSET="$ASSETS/berlin-routing.graphcache"
ROUTING_ASSET="$ASSETS/berlin-routing.osm.pbf"
rm -f "$CACHE_ASSET" "$ROUTING_ASSET"
cache_built=0
# The cameras the app actually routes around (after dropping indoor, ALPR,
# guards), as counted by the core: build_cache prints "...: N cameras, ...".
routed_cameras=""
if [[ "${SKIP_CACHE:-0}" != "1" ]]; then
    if command -v cargo >/dev/null 2>&1; then
        if cache_out="$(cargo run --quiet --release --manifest-path "$ROOT/core/Cargo.toml" \
            --example build_cache -- "$ROUTING" "$CACHE_ASSET")"; then
            echo "$cache_out"
            routed_cameras="$(sed -nE 's/.*: ([0-9]+) cameras,.*/\1/p' <<<"$cache_out" | head -1)"
            cache_built=1
        else
            echo "WARNING: cache pre-build failed"
            rm -f "$CACHE_ASSET"
        fi
    else
        echo "cargo not found -- cannot pre-build the routing cache"
    fi
fi
if [[ "$cache_built" == "1" ]]; then
    echo "-> $CACHE_ASSET (bundled into app assets; the extract is not)"
else
    # No cache, so the app has to build the graph itself from the extract.
    cp "$ROUTING" "$ROUTING_ASSET"
    echo "-> $ROUTING_ASSET (bundled instead: the app scores on first launch)"
fi

# Provenance: record what this snapshot actually contains so a build can state
# how current its cameras are. The OSM snapshot date is the extract's own
# replication timestamp (when Geofabrik cut it, not when we downloaded it); the
# camera count is the surveillance nodes in the snapshot. With LIVE_CAMERAS the
# snapshot date is Overpass' data timestamp and the extract date is kept apart. Both are best-effort
# -- a failure here must not fail the build -- and are written to build-info.txt
# for the release workflow to fold into its notes.
BUILD_INFO="$DATA/build-info.txt"
# `|| true` on each substitution because the script runs under `set -e`: a
# failed osmium probe here must degrade to "unknown", never abort the build.
extract_snapshot="$(osmium fileinfo -e -g header.option.osmosis_replication_timestamp \
    "$RAW" 2>/dev/null | head -1 || true)"
[[ -z "$extract_snapshot" ]] && extract_snapshot="unknown"
# The camera data's own timestamp: Overpass' when the cameras were pulled live,
# the extract's otherwise. This is what the app and the release notes report.
osm_snapshot="${camera_snapshot:-$extract_snapshot}"
cam_probe="$DATA/tmp/cameras-probe.osm.pbf"
mkdir -p "$DATA/tmp"
cameras=""
if osmium tags-filter --overwrite "$ROUTING" n/man_made=surveillance \
    -o "$cam_probe" 2>/dev/null; then
    cameras="$(osmium fileinfo -e -g data.count.nodes "$cam_probe" 2>/dev/null | head -1 || true)"
    rm -f "$cam_probe"
fi
[[ -z "$cameras" ]] && cameras="unknown"
{
    echo "built_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "osm_snapshot=$osm_snapshot"
    echo "camera_source=$camera_source"
    echo "extract_snapshot=$extract_snapshot"
    echo "surveillance_nodes=$cameras"
    echo "cameras=${routed_cameras:-unknown}"
    echo "extract_source=$GEOFABRIK"
} > "$BUILD_INFO"
# The app shows how fresh its data is (About panel), so the two dates travel
# inside the APK too. Dates only: nothing else from build-info.txt is shipped.
grep -E '^(osm_snapshot|built_utc)=' "$BUILD_INFO" > "$ASSETS/data-info.txt"
echo "   cameras: ${routed_cameras:-unknown} routed ($cameras surveillance nodes)  |  camera data: $osm_snapshot ($camera_source)  |  extract: $extract_snapshot"
echo "   provenance written to $BUILD_INFO"

# --- 3. Label glyphs ---------------------------------------------------------
if [[ "${SKIP_TILES:-0}" == "1" ]]; then
    echo "SKIP_TILES=1 -- skipping glyphs and basemap."
    echo "The app builds and routes without them; the map draws on a plain"
    echo "background with no labels until you re-run without SKIP_TILES."
    exit 0
fi

GLYPH_OUT="$ASSETS/glyphs/$FONTSTACK"
if [[ -d "$GLYPH_OUT" ]] && [[ -n "$(ls -A "$GLYPH_OUT" 2>/dev/null)" ]]; then
    echo "Using existing glyphs in $GLYPH_OUT"
else
    if [[ ! -f "$FONTS_ZIP" ]]; then
        fetch "$FONTS_URL" "$FONTS_ZIP" "the Noto Sans glyph pack (~60 MB)"
    fi
    # The openmaptiles/fonts v2.0 release publishes no checksum for
    # noto-sans.zip (only noto-open-sans.zip carries one), so this hash is
    # pinned from the vetted copy this project's releases were built from
    # (trust-on-first-use). It makes a later tamper or a silently changed asset
    # detectable; bump it deliberately if FONTS_URL is ever repointed.
    FONTS_SHA256="d117316544b43a5dde7ee761b36e17701e9f85574e181d76a74814240fdbaf34"
    actual_fonts="$(sha256_hex "$FONTS_ZIP")"
    if [[ "$actual_fonts" != "$FONTS_SHA256" ]]; then
        die "checksum mismatch for $FONTS_ZIP
  pinned: $FONTS_SHA256
  actual: $actual_fonts
The font pack differs from the pinned copy -- corrupt, tampered, or the
upstream asset changed. Delete it and re-run, and if the change is legitimate
update FONTS_SHA256 in this script:
  rm $FONTS_ZIP && $0"
    fi
    echo "  glyph pack checksum OK ($actual_fonts)"
    echo "Extracting Latin glyph ranges for $FONTSTACK..."
    mkdir -p "$GLYPH_OUT"
    for start in "${GLYPH_RANGES[@]}"; do
        name="$start-$((start + 255)).pbf"
        # -j junks the archive path; -o overwrites; land it in the flat output.
        if unzip -o -j "$FONTS_ZIP" "$FONTSTACK/$name" -d "$GLYPH_OUT" \
            >/dev/null 2>&1; then
            :
        else
            echo "  (range $name absent in pack, skipping)"
        fi
    done
    count=$(find "$GLYPH_OUT" -name "*.pbf" | wc -l | tr -d " ")
    if [[ "$count" -eq 0 ]]; then
        rm -rf "$ASSETS/glyphs"
        die "extracted no glyphs -- the font pack layout may have changed.
Expected files like '$FONTSTACK/0-255.pbf' inside $FONTS_ZIP."
    fi
    size=$(du -sh "$ASSETS/glyphs" | cut -f1)
    echo "-> $ASSETS/glyphs ($count ranges, $size)"
fi

# --- 4. Offline basemap tiles ------------------------------------------------
if [[ ! -f "$PLANETILER_JAR" ]]; then
    fetch "$PLANETILER_URL" "$PLANETILER_JAR" "Planetiler $PLANETILER_VERSION"
fi
# The jar is executed with the same JVM that renders the tiles, so verify it
# against the SHA-256 the Planetiler release publishes next to it before running.
verify_sha256 "$PLANETILER_JAR" "$PLANETILER_URL.sha256" "Planetiler $PLANETILER_VERSION"

# Leave out the OpenMapTiles layers app/src/main/assets/style_template.json never
# draws. Measured on the v0.2.0 tiles (zoom 14-15 hold 92% of the file): poi was
# 42% of the tile bytes and housenumber 10%, so ~half the 82 MB was data no
# style layer reads. The style's source-layers are boundary, building,
# landcover, landuse, park, place, transportation, transportation_name, water
# and waterway: if a layer is ever added to the style, take it off this list.
EXCLUDE_LAYERS="poi,housenumber,water_name,aeroway,aerodrome_label,mountain_peak"

echo "Rendering Berlin vector tiles (this takes a few minutes)..."
java -Xmx3g -jar "$PLANETILER_JAR" \
    --osm-path="$RAW" \
    --output="$TILES" \
    --maxzoom=15 \
    --exclude-layers="$EXCLUDE_LAYERS" \
    --download \
    --download-dir="$DATA/planetiler-sources" \
    --force \
    || die "Planetiler failed. If it ran out of memory, lower --maxzoom or
raise -Xmx. To get a working app without tiles for now:  SKIP_TILES=1 $0"

cp "$TILES" "$ASSETS/berlin.pmtiles"
echo "-> $TILES (bundled into app assets)"

echo
echo "Done. Rebuild the app with:  ./gradlew :app:assembleDebug"
