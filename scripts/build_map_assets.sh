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
        --connect-timeout 20 \
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
if [[ -f "$RAW" ]]; then
    echo "Using existing $RAW"
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
    echo "Verifying extract checksum..."
    if ! curl -fsSL --retry "$RETRIES" --retry-delay 3 --connect-timeout 20 \
        "$GEOFABRIK.md5" -o "$DATA/berlin-latest.osm.pbf.md5"; then
        die "downloaded the extract but could not fetch its checksum from
  $GEOFABRIK.md5
Re-run in a few minutes (the extract is kept, so this is quick). To verify by
hand:  md5sum $RAW   and compare against the .md5 published next to the file.
To proceed without verification (understand the risk):  SKIP_CHECKSUM=1 $0"
    fi
    # The published file is "<md5>  berlin-latest.osm.pbf"; we only want the hex.
    published="$(awk '{print $1}' "$DATA/berlin-latest.osm.pbf.md5")"
    actual="$(md5_hex "$RAW")"
    if [[ -z "$published" || "$published" != "$actual" ]]; then
        die "checksum mismatch for $RAW
  published: ${published:-<none found>}
  actual:    $actual
The download is corrupt or was tampered with. Delete it and re-run:
  rm $RAW && $0"
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

# --- 3. Offline basemap tiles ------------------------------------------------
if [[ "${SKIP_TILES:-0}" == "1" ]]; then
    echo "SKIP_TILES=1 -- skipping basemap generation."
    echo "The app builds and routes without tiles; the map draws on a plain"
    echo "background until you re-run without SKIP_TILES."
    exit 0
fi

if [[ ! -f "$PLANETILER_JAR" ]]; then
    fetch "$PLANETILER_URL" "$PLANETILER_JAR" "Planetiler $PLANETILER_VERSION"
fi

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
