#!/usr/bin/env python3
"""Generate core/tests/fixtures/mini_berlin.osm.pbf — a tiny synthetic town.

Layout (all in central-Berlin coordinates so real-world defaults apply):

    11 --- 12 --- 13 --- 14 --- 15     lat 52.5210   (row 1, residential)
     |      |      |      |      |                   (footway connectors)
     1 ---- 2 --●- 3 ---- 4 ---- 5     lat 52.5200   (row 0, residential)

● = dome camera sitting on the row-0 street between nodes 2 and 3, so the
short way from 1 to 5 is watched and the row-1 detour is clean. Extra
surveillance nodes that must be DROPPED by ingest: a guard and an ALPR.
A directional fixed camera at node 15 points north, away from every street.

Needs `osmium` (osmium-tool) on PATH to convert XML → PBF.
"""

import subprocess
import sys
import tempfile
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "core/tests/fixtures/mini_berlin.osm.pbf"

LONS = ["13.4000", "13.4010", "13.4020", "13.4030", "13.4040"]

nodes = []
for col, lon in enumerate(LONS):
    nodes.append((1 + col, "52.5200", lon, []))  # row 0: ids 1..5
    nodes.append((11 + col, "52.5210", lon, []))  # row 1: ids 11..15

nodes += [
    # The dome camera on the row-0 street, mid-segment between nodes 2 and 3.
    (100, "52.5200", "13.4015", [("man_made", "surveillance"),
                                 ("surveillance:type", "camera"),
                                 ("camera:type", "dome")]),
    # Fixed camera at the NE corner, pointing north away from all streets.
    (101, "52.5210", "13.4040", [("man_made", "surveillance"),
                                 ("surveillance:type", "camera"),
                                 ("camera:type", "fixed"),
                                 ("camera:direction", "0")]),
    # Non-camera surveillance: ingest must drop both.
    (102, "52.5205", "13.3990", [("man_made", "surveillance"),
                                 ("surveillance:type", "guard")]),
    (103, "52.5205", "13.4050", [("man_made", "surveillance"),
                                 ("surveillance:type", "ALPR")]),
]

ways = [
    (200, [1, 2, 3, 4, 5], [("highway", "residential")]),
    (201, [11, 12, 13, 14, 15], [("highway", "residential")]),
]
for col in range(5):
    ways.append((210 + col, [1 + col, 11 + col], [("highway", "footway")]))

xml = ['<?xml version="1.0" encoding="UTF-8"?>',
       '<osm version="0.6" generator="schattenweg-fixture">']
for nid, lat, lon, tags in sorted(nodes):
    if tags:
        xml.append(f'  <node id="{nid}" version="1" lat="{lat}" lon="{lon}">')
        xml.extend(f'    <tag k="{k}" v="{v}"/>' for k, v in tags)
        xml.append('  </node>')
    else:
        xml.append(f'  <node id="{nid}" version="1" lat="{lat}" lon="{lon}"/>')
for wid, refs, tags in ways:
    xml.append(f'  <way id="{wid}" version="1">')
    xml.extend(f'    <nd ref="{r}"/>' for r in refs)
    xml.extend(f'    <tag k="{k}" v="{v}"/>' for k, v in tags)
    xml.append('  </way>')
xml.append('</osm>')

OUT.parent.mkdir(parents=True, exist_ok=True)
with tempfile.NamedTemporaryFile("w", suffix=".osm", delete=False) as f:
    f.write("\n".join(xml))
    tmp = f.name

subprocess.run(["osmium", "cat", "--overwrite", tmp, "-o", str(OUT)], check=True)
print(f"wrote {OUT} ({OUT.stat().st_size} bytes)", file=sys.stderr)
