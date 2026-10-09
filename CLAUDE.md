# CLAUDE.md — project context & decisions

Context for anyone (human or Claude) picking this repo up. It captures **what
was decided and why**, so future work doesn't re-litigate settled questions.

---

## 1. What we're building

**Schattenweg** ("shadow path") — a privacy- and security-first Android app that:

1. Maps CCTV cameras in **Berlin** (only, for now) from OpenStreetMap.
2. Lets the user **plan a walking route that avoids cameras**, trading detour
   length against surveillance exposure.
3. Leaks nothing: location and routing stay **on-device**.

Package id: `de.schattenweg.app`. Rust crate: `schattenweg-core`
(UniFFI namespace `schattenweg_core`).

The original prompt pointed at <https://osmcamera.dihe.de/> as the data source.

---

## 2. Data source — decided

**Use OpenStreetMap directly. Do NOT build on osmcamera.dihe.de.**

That site is a PHP/MySQL scrape of OSM planet files with a Leaflet frontend and
**no clean API**, and its data was **stale (last camera update 2024-04-19)**. The
real data underneath is plain OSM: cameras are nodes tagged `man_made=surveillance`
(~219k worldwide at last count). Original code: github.com/khris78/osmcamera.

Our ingest paths (see `scripts/` and `core/src/osm.rs`):

- **Live/dev:** Overpass API, `node["man_made"="surveillance"]` in a Berlin bbox
  (`scripts/fetch_cameras.sh`, GeoJSON preview only).
- **Shipped/offline:** Geofabrik `berlin-latest.osm.pbf`, filtered with `osmium`
  to streets + surveillance nodes (`scripts/build_map_assets.sh`), read
  on-device by the Rust core and bundled in the APK.

### OSM tag → model mapping (the part that's easy to get wrong)

| Tag | Use |
|-----|-----|
| `man_made=surveillance` | qualifies the node |
| `surveillance:type=camera` | keep; drop `guard` / `ALPR` (not lenses to dodge) |
| `camera:type=fixed\|dome\|panning` | cone vs disc coverage |
| `camera:direction=<deg or compass>` | cone centre bearing (0=N, 90=E) |
| `surveillance=indoor` | **dropped**: it watches the inside of a building, not the street (a disc would also cut through walls) |
| `surveillance=public\|outdoor\|traffic` (or untagged) | kept; context only, not required |
| `building=*` (not `no`) | closed way, or `type=multipolygon` relation with closed outer ways: **blocks a camera's view** if tall enough (below) |
| `height` → `building:height` → `building:levels`×3.2 m → 9 m | the one height chain (`osm::building_height_m`); `building=shed\|garage\|garages\|carport\|roof\|hut\|kiosk\|greenhouse\|cabin` default to 3 m instead |
| `min_height` ≥ mount height | dropped: raised parts (skybridges) don't block a sight line |

Decided: routes avoid **cameras only** — ALPR reads plates, guards aren't
lenses. Coverage is uneven — OSM has only a fraction of real cameras. **The UI
must say so** (see §5).

---

## 3. Architecture — decided

Driving principle: **a surveillance-avoidance tool that phones home is
self-defeating.** So everything sensitive is on-device.

- **Rust core (`core/`, crate `schattenweg-core`) via UniFFI** — OSM parsing,
  exposure scoring, routing. Memory-safe language handling untrusted OSM input
  across a narrow FFI surface. Keep the FFI boundary small and value-typed;
  heavy state lives behind one `Router` object (never marshal the whole graph).
- **Kotlin + Jetpack Compose UI (`app/`)** — presentation only. minSdk 29.
- **MapLibre GL Native** for the map — **never** the Google Maps SDK (it drags in
  Play Services and beacons). **Offline bundled Berlin vector tiles** — no tile
  server sees the viewport.
- **Positioning via platform `LocationManager` (GNSS)** — **never** the Fused
  Location Provider (routes through Google).
- **No Google Play Services / Firebase / analytics.** GrapheneOS is a
  first-class target. A Play Services fallback was explicitly deferred.
- **All map data bundled in the APK** (pre-filtered snapshot); the app declares
  no `INTERNET` permission. Label glyphs (Latin ranges of Noto Sans only, ~1 MB)
  are bundled too and copied to `filesDir` beside the tiles — the style's
  `glyphs` URL is a local `file://` path, so labels render offline.
- **Search is on-device.** `Router::search_places` scans a name index built
  during ingest (streets, localities, stations — not arbitrary POIs). There is
  no geocoder, so searching for a destination reveals nothing to anyone. The
  index costs nothing extra to build: it is collected in the graph-build passes,
  not a separate scan.
- **MapLibre 13.x renders with Vulkan** (its manifest marks Vulkan 1.0
  required). Fine for the Pixel-class hardware GrapheneOS runs on, but
  emulators commonly lack a usable Vulkan driver and then render *nothing* —
  no basemap and no GeoJSON overlays — which is easily misread as a tile or
  data fault. `-PmaplibreBackend=opengl` selects
  `org.maplibre.gl:android-sdk-opengl` for those; Vulkan stays the default
  for real devices.
- **PMTiles must be read from `filesDir`, not assets** — Android's asset
  manager can't serve the byte-range reads PMTiles needs. `MapAssets` copies
  both the tiles and the routing snapshot out on first launch.
- **3D buildings are a view layer only, off by default.** `building-3d` in
  `style_template.json` is a `fill-extrusion` of the basemap's own `building`
  layer (heights from the OpenMapTiles `render_height` / `render_min_height`
  properties, so no extra data), toggled from the layers panel, which also
  tilts the camera (`TILT_3D_DEG`). It needs the flat buildings layer on. Its
  heights come from the tiles' own rule, **not** the core's
  (`osm::building_height_m`, the 6 m blocking threshold), so never colour or
  filter it by "blocks cameras": that picture would disagree with the model.

### Security decisions (see SECURITY.md for the full threat model)

- MapLibre's library manifest brings `INTERNET`, `ACCESS_WIFI_STATE`,
  `ACCESS_FINE_LOCATION` and `ACCESS_COARSE_LOCATION`. All four are **stripped
  in the merge** (`tools:node="remove"`); only `ACCESS_NETWORK_STATE` stays,
  because MapLibre's `ConnectivityReceiver` calls `ConnectivityManager` and
  would otherwise throw. Verified against the 13.6.0 AAR: no `WifiManager`
  use, and no telemetry/analytics classes at all.
- `allowBackup=false`, cleartext traffic disabled, only the launcher activity
  exported, R8 + resource shrinking on release with keep rules for JNA/UniFFI.
- **Signing keys never enter the repo.** Credentials come from an untracked
  `keystore.properties` or from env vars; absent them the release build is
  left **unsigned** rather than silently using the public debug key.
- Dependencies are pinned (no dynamic versions); CI validates the Gradle
  wrapper and fails on any Play Services/Firebase artifact.

---

## 4. The routing model — decided

Ordinary routing minimises distance; we add a per-edge **exposure score** and let
the user trade it off:

```
edge weight = length_m * (1 + λ * exposure)          exposure ∈ [0,1]
```

- `λ = 0` → shortest path, cameras ignored.
- `λ ≈ 1–3` → sensible avoidance.
- `λ > 5` → big detours to dodge lenses.

The core takes λ as a continuous f64 and always will — but the **UI no longer
exposes a raw slider**. It offers three presets, Low/Medium/High → λ 1/3/6
(`AvoidanceLevel` in `RouteViewModel.kt`); three named choices are easier to
reason about than a bare number. Two behaviours ride on top, both decided:
- **Every new route starts at High**: the control launches on High, and a
  freshly dropped A→B pair (second map tap, or a search start/end completing the
  pair) resets the level to High whatever was picked for the previous route
  (`plan(freshPair = true)`). High is the strongest preset, so this also gives
  the camera-free route whenever one exists. (This replaced an earlier "start at
  Medium, then retry at High if the route is still watched" rule.)
- A **manual** level change is honoured exactly for that route
  (`plan()`, `freshPair = false`), so Low/Medium still buy a shorter,
  more-exposed route even when a longer camera-free one exists. Without that,
  the lower presets would be dead controls. It does not stick: the next new
  route is back on High.

Multiplicative-on-length keeps units in metres-equivalent, which keeps the A*
straight-line heuristic **admissible** for `λ ≥ 0`.

**Exposure scoring** (`core/src/exposure.rs`): walk each edge in ~5 m steps; at
each sample point ask whether any camera covers it; score = covered fraction.
Done **once** at load time and baked onto edges. A point is "watched" if a
camera covers it **or** it lies inside a police AI-video zone (below).

**Field-of-view geometry** (`core/src/camera.rs`):
- Directional/`fixed` camera with a known `camera:direction` → **cone**
  (bearing ± half-FOV, within range).
- `dome` / `panning` / unknown → **disc** (range only).
- Default range/FOV live in `camera::defaults` — deliberately conservative
  guesses; **tune against ground truth**, they are not from OSM.

**Buildings block the view — decided** (`core/src/occluders.rs`). A point is
covered by a camera only if it is in range and in the cone **and** the sight
line from the camera does not cross the outline of a building tall enough to
hide it (`defaults::occluder_min_height_m()`, 6 m — a guess, tuned on the high
side because wrongly treating a building as transparent only *over*-reports
exposure, the safe direction). Every exemption errs the same way:
- the camera's **own building** (inside the footprint or within 3 m of its
  wall — cameras sit on facades) is ignored for that camera;
- a building **containing the sample point** is ignored, so passages and
  `highway=corridor` edges stay watched;
- a sight line that merely grazes a vertex isn't blocked;
- inner rings are ignored (courtyards are solid; a camera inside one is "in its
  own building");
- **AI-video zones are not occluded** — they are policy circles, not lines of
  sight.

The ingest (`osm::load_buildings`) keeps only blocking buildings within camera
range of some camera, which is lossless for scoring and keeps the cache small.
Multipolygon relations are supported only where the outer rings are closed
ways; outers made of several open ways are skipped. The routing snapshot must
therefore contain buildings: `build_map_assets.sh` filters
`w/highway w/building r/building n/man_made=surveillance`. **Measure the
snapshot size when regenerating** — it carries every Berlin building even
though the core only keeps those near cameras. Changing the 6 m threshold, the
height chain or the exemptions changes every cached score: bump `cache::VERSION`.

**AI-video zones (`core/src/zones.rs`) — routed around, decided.** Berlin police
run (Kottbusser Tor) or have announced (Warschauer Brücke, Alexanderplatz,
Görlitzer Park, plus three building pilots) AI behaviour detection on CCTV. No
camera positions or boundaries are published — the Senate refused (Drucksache
19/26970) — so each site is an **approximate circle** around a landmark.
- **One table, in the core.** `zones::table()` feeds the exposure pass *and* is
  exported to the UI (`ai_zones()`), which draws and describes exactly those
  records. There is no second copy in Kotlin to drift. Edit zones there.
- **How they affect routes:** a sample point inside a zone counts as watched,
  exactly like a point inside a camera's field of view, so a zone raises an
  edge's exposure by the share of it inside the circle and the usual
  `length * (1 + λ * exposure)` weight does the rest. This is a deliberate
  choice to treat "AI analysis happens here" as full exposure — a policy
  judgement, not a measurement. A zone says nothing about lens count.
- **Planned zones count too** (Alexanderplatz, Görlitzer Park, the buildings),
  not only Kottbusser Tor. If that should change, filter on `ZoneStatus` where
  `build_parts_from_pbf` builds the `ZoneIndex`.
- **Cache:** the zone *geometry* is hashed into the cache fingerprint
  (`zones::signature`), so editing a zone rebuilds the cache automatically;
  reworded notes do not. Changing how zones are *scored* still needs a
  `cache::VERSION` bump like any other scoring change. This also covers the
  cache pre-built into the APK by `build_map_assets.sh`: it is built by the
  same code, so it matches the zone table of the commit it was built from. A
  stale asset set built before a zone edit is rejected on-device and the app
  scores on first launch instead — slower, never wrong.
- **UI:** violet dashed circles under "AI-monitored zones", long-press for
  status and what is detected. Hiding the layer does not change routing.
  Re-check `AS_OF` and the status table as pilots start; replace the circles
  with surveyed outlines once the promised entrance signage exists.

**Modes:** walking only (decided; cycling deferred).

The map draws that same geometry, and the **core is the only place it is
computed**: `Router::coverage_near` (→ `OccluderIndex::coverage_ring`) returns
each camera's wedge (fixed camera with a bearing) or disc (everything else),
cut short where a building blocks the view, and `coverageGeoJson` in
`MapScreen.kt` just serialises those rings. It is there so the user can see what
the exposure score was actually computed from. The ring is traced with a fixed
number of rays (48 across a cone, 72 round a disc), so it can differ from the
scoring test by a metre or two along a shadow edge. There used to be a
Kotlin copy of the wedge/disc rule; it was removed because a picture that
disagrees with the model is worse than no picture — it looks authoritative.
Don't reintroduce one.

Alternatives considered and **not** chosen: GraphHopper custom model, Valhalla
`avoid_polygons`. Rejected in favour of the hand-rolled Rust pass because it
keeps everything on-device, dependency-free, and fully under our control. Revisit
only if the custom router can't keep up.

---

## 5. Caveats to keep in the UI (non-negotiable)

Both live in `MapScreen.kt`:

1. "Shows only cameras **mapped in OpenStreetMap**." (Real coverage is higher.)
   Only *mapped* buildings are assumed to block a camera's view; trees, fences,
   vehicles and unmapped buildings are not modelled, so real exposure can be
   higher still. The AI-video zones are approximate circles, not surveyed
   outlines, and the panel says so.
2. "Avoiding cameras is **not anonymity**." (And a route that conspicuously weaves
   around every lens can itself be a signal.)
3. **"© OpenMapTiles © OpenStreetMap contributors"** — an attribution
   obligation, not a caveat: OSM data is ODbL and the OpenMapTiles schema the
   basemap is generated with is CC-BY, both of which require a visible credit.
   Planetiler prints this requirement at the end of every tile build. Bundling
   the tiles offline does not exempt us. If the basemap is ever regenerated
   from a different schema, update the credit to match rather than dropping it.
   The full credit lives in the layers (burger ☰) panel's **About** section,
   alongside the app **version** with a **"Get the latest version"** link that
   downloads the newest APK directly from the rolling `latest` release
   (`LATEST_APK_URL`; it and the README's direct APK link both rely on
   `release.yml` keeping the asset name `schattenweg-latest-debug.apk`),
   a **"Rendered with MapLibre"** renderer
   credit, an **"Open-source licences"** link (to `THIRD_PARTY_LICENSES.md`),
   and the project GitHub / "Report a problem" links (there is deliberately no
   maintainer-name credit). The "© OpenStreetMap
   contributors" and "© OpenMapTiles" lines in About are **links** to their
   licences (ODbL / CC-BY), as both ask attribution to point at the terms. The
   **Open-source licences** entry opens a full-screen, fully offline view
   (`LicensesScreen`) that shows the bundled BSD-2-Clause (MapLibre) and OFL-1.1
   (Noto Sans) texts verbatim from `app/src/main/assets/licenses/` — those two
   licences require their text to ship with the binary. Keep the asset files and
   that screen in step; the rest of the dependency list lives in
   `THIRD_PARTY_LICENSES.md`.

   MapLibre's own bottom-left badge (logo + ⓘ attribution) is **disabled**
   (`uiSettings.isLogoEnabled = false`, `isAttributionEnabled = false`) so the
   credits aren't split between a MapLibre control and the panel. But ODbL wants
   attribution *visible on the map*, not only in a menu — so a small always-on
   **"© OpenStreetMap"** line sits at the top of the bottom control stack and
   taps through to the About panel. Keep that on-map line (the ODbL affordance)
   and the full About credit both present; if the credits move again, move the
   version, the MapLibre credit and the licences link with them.

---

## 6. Conventions

- Rust: keep the FFI surface in `lib.rs` minimal and value-typed. Business logic
  stays in the private modules and is unit-tested without UniFFI.
- No new dependency that pulls in Google/Play Services, ever.
- Distances in metres, bearings in degrees (0=N, 90=E), coordinates as
  `(lat, lon)` f64.
- Licence: GPL-3.0-or-later; preserve OSM/ODbL attribution.

---

## 7. State of the repo

Track progress against this list when picking the project back up:

- [x] Rust core: camera/FOV geometry, exposure scoring, camera-aware A* — unit-tested.
- [x] UniFFI surface (`Router::from_pbf`, `plan`, `cameras_near`, `camera_count`).
- [x] `core/src/osm.rs::load_cameras` / `load_graph` — PBF ingest via `osmpbf`.
- [x] Spatial grids for `CameraIndex::any_covers` (3×3 cell query) and
      `Graph::nearest_node` (expanding ring). Both tested against brute force.
- [x] `core/examples/plan_route.rs` CLI demo; `core/tests/pbf_ingest.rs`
      exercises the whole pipeline on a synthetic PBF fixture.
- [x] `scripts/build_map_assets.sh`: Geofabrik (md5-verified) → filtered
      snapshot + Planetiler offline tiles.
- [x] Buildings occlude camera coverage (`core/src/occluders.rs`,
      `osm::load_buildings`, cache v4, `Router::coverage_near`; map draws the
      core's clipped outlines). Unit- and fixture-tested
      (`scripts/make_building_fixture.py` → `mini_berlin_buildings.osm.pbf`;
      regenerate with `osmium` on PATH or `pip install osmium`).
      **Not yet run against the real Berlin extract or on a device:** snapshot
      size growth, build/launch time and the before/after exposure on the 4251
      cameras are unmeasured, and the Kotlin change has not been compiled
      outside CI.
- [x] Android app: Gradle + cargo-ndk + UniFFI bindings + MapLibre map screen
      (offline tiles, camera layer, tap-to-route, Low/Medium/High avoidance
      control, in-app GitHub credit).
- [x] CI: Rust fmt/clippy/test + Android assembleDebug + no-Play-Services gate.
- [x] Release workflow (`.github/workflows/release.yml`): a version tag (or a
      manual run) generates the offline Berlin assets (`build_map_assets.sh` —
      needs osmium + Java 21 for Planetiler, so the job sets up both 17 and 21),
      builds the APK, and publishes it to GitHub Releases. Outputs are named
      `schattenweg-<variant>.apk` (`base.archivesName`), not `app-<variant>.apk`.
      Released APKs are offline-complete but **debug-signed** — release-key
      signing needs the keystore, which never enters the repo. The everyday
      `ci.yml` still builds an **asset-free** debug APK as a compile check only.

**Verified end to end on an emulator** (2026-08-29, arm64, OpenGL backend):
the bundled 81 MB PMTiles basemap renders offline from `filesDir`, ingest
reports 4251 Berlin cameras with 641 drawn within a 2 km radius, tap-to-route
plans between two taps, and moving λ re-plans. A λ=8 route across the centre
came out 3093 m at 0% mean exposure. **Not yet run on real hardware** — the
emulator has no usable Vulkan driver, so that path is still unexercised (see
§3); a Pixel is the next real test.

Three bugs that only appear on-device were found in that first run, all worth
remembering because none could fail a unit test:
- MapLibre 13.x defaults to Vulkan; on the emulator it rendered *nothing*, not
  even GeoJSON overlays, while logging no error.
- `refreshCameras` returned early while the router was still loading, so the
  camera layer stayed empty until something moved the map.
- The GeoJSON overlays were written from `AndroidView`'s `update` block with
  the state reads inside `getMapAsync`'s callback. Compose does not record
  reads made in an async callback, so the block ran once against empty data
  and never again. Overlay data is now pushed from a keyed `LaunchedEffect`;
  keep it that way.

**Known perf follow-up:** the router was ready ~6 s after launch on the real
Berlin extract (style loads in ~80 ms by comparison) — that gap is the one-off
exposure pass over every edge. That pass is now **cached**: `Router::open`
(`core/src/cache.rs`) reloads the scored graph from a small binary snapshot
next to the extract in `filesDir` instead of re-deriving it, so every cold
start *after the first* skips the pass. The cache is only the five flat inputs
the router is assembled from (nodes, scored edges, cameras, places, and the
blocking-building outlines near cameras); its header
carries a format/logic `VERSION` and a fingerprint of the source extract, and
any mismatch or malformed byte makes it fall back to the PBF. **Pre-built at asset-build time:**
`build_map_assets.sh` now runs `core/examples/build_cache.rs` (same `Router::open`
path) and bundles `berlin-routing.graphcache` beside the snapshot; `MapAssets`
copies it to `filesDir`, so even the first launch skips the pass. The cache
fingerprint is a **content hash** of the extract (not size+mtime, which would
change when the asset is copied); a missing/stale cache still falls back to the
PBF. `SKIP_CACHE=1` skips the step. Remaining lever: stop bundling the `.pbf`,
and `rstar` in place of the grids.
**When the scoring/geometry logic changes** (`camera.rs` coverage, the
`defaults` table, `exposure.rs` sampling, `occluders.rs`, the building height
chain in `osm.rs`), bump `cache::VERSION` so stale
scores can't outlive the code that produced them.

**Deliberately deferred:** cycling profile, location puck ("centre on me"),
camera FOV tuning against ground truth, Play Services fallback build.
(On-device place search — `Router::search_places`, `core/src/places.rs` — is
**done** and wired into the UI, so it is no longer on this list.)
