#!/usr/bin/env bash
#
# fetch_cameras.sh — pull Berlin surveillance cameras from OpenStreetMap.
#
# Two paths:
#   (A) LIVE via Overpass  — quick, current, good for dev. Default.
#   (B) OFFLINE via PBF    — reproducible, ships in the app. See notes at bottom.
#
# Output: data/berlin-cameras.geojson  (man_made=surveillance nodes)
#
# NOTE ON THE ORIGINAL SOURCE: the site that kicked this project off
# (osmcamera.dihe.de) is just a stale scrape of this same OSM data (last
# refreshed 2024-04). Go to OSM directly — that's what this does.

set -euo pipefail

OUT_DIR="$(cd "$(dirname "$0")/.." && pwd)/data"
mkdir -p "$OUT_DIR"
OUT="$OUT_DIR/berlin-cameras.geojson"

# Berlin bounding box (south,west,north,east). Trim/replace with the Berlin
# admin-area query for exact borders if you prefer.
BBOX="52.3383,13.0884,52.6755,13.7612"

OVERPASS_URL="https://overpass-api.de/api/interpreter"

echo "Querying Overpass for man_made=surveillance in Berlin bbox…"
QUERY="[out:json][timeout:120];
node[\"man_made\"=\"surveillance\"](${BBOX});
out body;"

# Fetch, then convert the Overpass JSON to GeoJSON with jq.
curl -sS --data-urlencode "data=${QUERY}" "$OVERPASS_URL" \
  | jq '{
      type: "FeatureCollection",
      features: [
        .elements[]
        | select(.type == "node")
        | {
            type: "Feature",
            geometry: { type: "Point", coordinates: [.lon, .lat] },
            properties: (.tags + { osm_id: .id })
          }
      ]
    }' > "$OUT"

COUNT=$(jq '.features | length' "$OUT")
echo "Wrote $COUNT cameras → $OUT"

# -----------------------------------------------------------------------------
# (B) OFFLINE / REPRODUCIBLE ALTERNATIVE
#
# For the shipped app you want a fixed snapshot, not a live Overpass call.
# Download the Geofabrik Berlin extract and filter it with osmium:
#
#   curl -L -o data/berlin.osm.pbf \
#     https://download.geofabrik.de/europe/germany/berlin-latest.osm.pbf
#
#   osmium tags-filter data/berlin.osm.pbf \
#     n/man_made=surveillance -o data/berlin-cameras.osm.pbf
#
# The Rust core (src/osm.rs::load_cameras) reads that .pbf directly, so you can
# skip GeoJSON entirely for the on-device path and keep this GeoJSON output
# just for quick map previews / sanity checks.
# -----------------------------------------------------------------------------
