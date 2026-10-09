#!/usr/bin/env python3
"""Generate core/tests/fixtures/mini_berlin_buildings.osm.pbf.

Four small scenes in central-Berlin coordinates, ~700 m apart so they never
interact. Each scene (except the last) is the same 80 m east-west footway
(nodes S0-S1-S2) with a dome camera 12 m north of its middle. A dome's 20 m
range covers about 32 m of the street (x within ±16 m of the camera), so with
nothing in the way the route S0→S2 is ~45 % watched.

    camera  ●            y = +12 m
    ┌───────┐
    │ block │            y =  +4 .. +8 m, x = -8 .. +8 m
    └───────┘
    S0 ---- S1 ---- S2   y =   0 m, x = -40 / 0 / +40 m

  scene 0  "wall"      the block is a 5-storey building       -> street hidden
  scene 1  "garage"    the block is building=garage            -> street watched
  scene 2  "relation"  the block is an untagged closed way that is the outer
                       ring of a building multipolygon (height=15) -> hidden
  scene 3  "far"       a tall building kilometres from any camera; ingest must
                       prune it, so the fixture yields exactly two rings

Cameras are node ids 100/200/300 (one per scene 0-2). Needs either `osmium`
(osmium-tool) on PATH or the `osmium` Python package (pyosmium) to write the
PBF; the XML is generated here either way.
"""

import math
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

OUT = (Path(__file__).resolve().parent.parent
       / "core/tests/fixtures/mini_berlin_buildings.osm.pbf")

LAT0, LON0 = 52.5200, 13.4000
M_PER_DEG_LAT = 111_195.0
M_PER_DEG_LON = M_PER_DEG_LAT * math.cos(math.radians(LAT0))
SCENE_SPACING_DEG = 0.01  # ~680 m of longitude


def at(scene, east_m, north_m):
    lat = LAT0 + north_m / M_PER_DEG_LAT
    lon = LON0 + scene * SCENE_SPACING_DEG + east_m / M_PER_DEG_LON
    return f"{lat:.7f}", f"{lon:.7f}"


nodes = {}   # id -> (lat, lon, tags)
ways = []    # (id, [node ids], tags)
rels = []    # (id, [(type, ref, role)], tags)

CAMERA = [("man_made", "surveillance"), ("surveillance:type", "camera"),
          ("camera:type", "dome")]


def block_nodes(base, scene):
    corners = [(-8, 4), (-8, 8), (8, 8), (8, 4)]
    ids = []
    for i, (e, n) in enumerate(corners):
        nodes[base + i] = (*at(scene, e, n), [])
        ids.append(base + i)
    return ids + [ids[0]]


def scene_street(scene, base, camera_id):
    for i, e in enumerate((-40, 0, 40)):
        nodes[base + i] = (*at(scene, e, 0), [])
    nodes[camera_id] = (*at(scene, 0, 12), CAMERA)
    ways.append((base, [base, base + 1, base + 2],
                 [("highway", "footway"), ("name", f"Scene {scene} Weg")]))


# scene 0: tall building way
scene_street(0, 1, 100)
ways.append((1000, block_nodes(1000, 0),
             [("building", "apartments"), ("building:levels", "5")]))

# scene 1: a garage must not hide anything
scene_street(1, 11, 200)
ways.append((1100, block_nodes(1100, 1), [("building", "garage")]))

# scene 2: multipolygon building, outer way carries no tags of its own
scene_street(2, 21, 300)
ways.append((1200, block_nodes(1200, 2), []))
rels.append((5000, [("w", 1200, "outer")],
             [("type", "multipolygon"), ("building", "yes"), ("height", "15")]))

# scene 3: a tall building nowhere near a camera
for i, (e, n) in enumerate([(-8, 4), (-8, 8), (8, 8), (8, 4)]):
    nodes[1300 + i] = (*at(3, e + 1500, n), [])
ways.append((1300, [1300, 1301, 1302, 1303, 1300],
             [("building", "yes"), ("height", "30")]))

xml = ['<?xml version="1.0" encoding="UTF-8"?>',
       '<osm version="0.6" generator="schattenweg-fixture">']
for nid in sorted(nodes):
    lat, lon, tags = nodes[nid]
    if tags:
        xml.append(f'  <node id="{nid}" version="1" lat="{lat}" lon="{lon}">')
        xml.extend(f'    <tag k="{k}" v="{v}"/>' for k, v in tags)
        xml.append('  </node>')
    else:
        xml.append(f'  <node id="{nid}" version="1" lat="{lat}" lon="{lon}"/>')
for wid, refs, tags in sorted(ways):
    xml.append(f'  <way id="{wid}" version="1">')
    xml.extend(f'    <nd ref="{r}"/>' for r in refs)
    xml.extend(f'    <tag k="{k}" v="{v}"/>' for k, v in tags)
    xml.append('  </way>')
for rid, members, tags in rels:
    xml.append(f'  <relation id="{rid}" version="1">')
    xml.extend(f'    <member type="{t}" ref="{r}" role="{role}"/>'
               for t, r, role in members)
    xml.extend(f'    <tag k="{k}" v="{v}"/>' for k, v in tags)
    xml.append('  </relation>')
xml.append('</osm>')

OUT.parent.mkdir(parents=True, exist_ok=True)
with tempfile.NamedTemporaryFile("w", suffix=".osm", delete=False) as f:
    f.write("\n".join(xml))
    tmp = f.name

if shutil.which("osmium"):
    subprocess.run(["osmium", "cat", "--overwrite", tmp, "-o", str(OUT)],
                   check=True)
else:
    import osmium  # pyosmium: pip install osmium

    OUT.unlink(missing_ok=True)
    writer = osmium.SimpleWriter(str(OUT))
    for obj in osmium.FileProcessor(tmp):
        writer.add(obj)
    writer.close()
print(f"wrote {OUT} ({OUT.stat().st_size} bytes)", file=sys.stderr)
