#!/usr/bin/env bash
#
# build_map_assets.sh — produce everything the app bundles for offline Berlin.
#
#   1. data/berlin-latest.osm.pbf     full Geofabrik extract (download, ~70 MB)
#   2. data/berlin-routing.osm.pbf    streets + surveillance nodes only —
#                                     what the Rust core ingests on-device
#   3. data/berlin.pmtiles            vector basemap tiles (Planetiler)
#
# 2 and 3 are copied into app/src/main/assets/map/ for the APK build.
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
echo "Filtering to walkable streets + surveillance nodes..."
osmium tags-filter --overwrite "$RAW" \
    w/highway n/man_made=surveillance \
    -o "$ROUTING" \
    || die "osmium could not filter $RAW (is the file complete?)"
osmium fileinfo -e "$ROUTING" | sed -n 's/^  Number of/  /p' || true
cp "$ROUTING" "$ASSETS/berlin-routing.osm.pbf"
echo "-> $ROUTING (bundled into app assets)"

# Provenance: record what this snapshot actually contains so a build can state
# how current its cameras are. The OSM snapshot date is the extract's own
# replication timestamp (when Geofabrik cut it, not when we downloaded it); the
# camera count is the surveillance nodes in the snapshot. Both are best-effort
# -- a failure here must not fail the build -- and are written to build-info.txt
# for the release workflow to fold into its notes.
BUILD_INFO="$DATA/build-info.txt"
# `|| true` on each substitution because the script runs under `set -e`: a
# failed osmium probe here must degrade to "unknown", never abort the build.
osm_snapshot="$(osmium fileinfo -e -g header.option.osmosis_replication_timestamp \
    "$RAW" 2>/dev/null | head -1 || true)"
[[ -z "$osm_snapshot" ]] && osm_snapshot="unknown"
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
    echo "surveillance_nodes=$cameras"
    echo "extract_source=$GEOFABRIK"
} > "$BUILD_INFO"
echo "   cameras: $cameras surveillance nodes  |  OSM snapshot: $osm_snapshot"
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

echo "Rendering Berlin vector tiles (this takes a few minutes)..."
java -Xmx3g -jar "$PLANETILER_JAR" \
    --osm-path="$RAW" \
    --output="$TILES" \
    --maxzoom=15 \
    --download \
    --download-dir="$DATA/planetiler-sources" \
    --force \
    || die "Planetiler failed. If it ran out of memory, lower --maxzoom or
raise -Xmx. To get a working app without tiles for now:  SKIP_TILES=1 $0"

cp "$TILES" "$ASSETS/berlin.pmtiles"
echo "-> $TILES (bundled into app assets)"

echo
echo "Done. Rebuild the app with:  ./gradlew :app:assembleDebug"
