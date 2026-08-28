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

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DATA="$ROOT/data"
ASSETS="$ROOT/app/src/main/assets/map"
mkdir -p "$DATA" "$ASSETS"

GEOFABRIK="https://download.geofabrik.de/europe/germany/berlin-latest.osm.pbf"
RAW="$DATA/berlin-latest.osm.pbf"
ROUTING="$DATA/berlin-routing.osm.pbf"
TILES="$DATA/berlin.pmtiles"

PLANETILER_VERSION="v0.9.1"
PLANETILER_JAR="$DATA/planetiler-$PLANETILER_VERSION.jar"
PLANETILER_URL="https://github.com/onthegomap/planetiler/releases/download/$PLANETILER_VERSION/planetiler.jar"

# --- 1. Geofabrik extract ----------------------------------------------------
if [[ ! -f "$RAW" ]]; then
    echo "Downloading Berlin extract from Geofabrik…"
    curl -fL --retry 3 -o "$RAW" "$GEOFABRIK"
else
    echo "Using existing $RAW"
fi

# Integrity: Geofabrik publishes an .md5 next to every extract. Verify the
# download instead of trusting the pipe blindly.
echo "Verifying extract checksum…"
curl -fsL "$GEOFABRIK.md5" | awk -v f="$RAW" '{print $1 "  " f}' | md5sum -c -

# --- 2. Routing + camera snapshot -------------------------------------------
echo "Filtering to walkable streets + surveillance nodes…"
osmium tags-filter --overwrite "$RAW" \
    w/highway n/man_made=surveillance \
    -o "$ROUTING"
osmium fileinfo -e "$ROUTING" | sed -n 's/^  Number of/  /p' || true
cp "$ROUTING" "$ASSETS/berlin-routing.osm.pbf"
echo "→ $ROUTING (bundled into app assets)"

# --- 3. Offline basemap tiles ------------------------------------------------
if [[ "${SKIP_TILES:-0}" == "1" ]]; then
    echo "SKIP_TILES=1 — skipping basemap generation."
    exit 0
fi

if [[ ! -f "$PLANETILER_JAR" ]]; then
    echo "Downloading Planetiler $PLANETILER_VERSION…"
    curl -fL --retry 3 -o "$PLANETILER_JAR" "$PLANETILER_URL"
fi

echo "Rendering Berlin vector tiles (this takes a few minutes)…"
java -Xmx3g -jar "$PLANETILER_JAR" \
    --osm-path="$RAW" \
    --output="$TILES" \
    --maxzoom=15 \
    --download \
    --download-dir="$DATA/planetiler-sources" \
    --force

cp "$TILES" "$ASSETS/berlin.pmtiles"
echo "→ $TILES (bundled into app assets)"

echo
echo "Done. Rebuild the app with:  ./gradlew :app:assembleDebug"
