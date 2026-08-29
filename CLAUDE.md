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
| `surveillance=public\|outdoor\|traffic` | context/filtering, not required |

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
- A **freshly dropped A→B pair defaults to the camera-free route when one
  exists**: if the chosen level still leaves exposure > 0, the planner retries
  at the strongest level and adopts that route if it is 0% exposure, raising the
  displayed level to match (`plan(preferClean = true)`).
- A **manual** level change is honoured exactly (`preferClean = false`), so
  Low/Medium still buy a shorter, more-exposed route even when a longer
  camera-free one exists. Without that, the lower presets would be dead controls
  whenever a clean route was reachable.

Multiplicative-on-length keeps units in metres-equivalent, which keeps the A*
straight-line heuristic **admissible** for `λ ≥ 0`.

**Exposure scoring** (`core/src/exposure.rs`): walk each edge in ~5 m steps; at
each sample point ask whether any camera covers it; score = covered fraction.
Done **once** at load time and baked onto edges.

**Field-of-view geometry** (`core/src/camera.rs`):
- Directional/`fixed` camera with a known `camera:direction` → **cone**
  (bearing ± half-FOV, within range).
- `dome` / `panning` / unknown → **disc** (range only).
- Default range/FOV live in `camera::defaults` — deliberately conservative
  guesses; **tune against ground truth**, they are not from OSM.

**Modes:** walking only (decided; cycling deferred).

The map draws that same geometry: `coverageGeoJson` in `MapScreen.kt` renders
a wedge for a fixed camera with a bearing and a disc for everything else,
deliberately mirroring `camera.rs`. It is there so the user can see what the
exposure score was actually computed from. **If the coverage rule in
`camera.rs` changes, change the drawing with it** — a picture that disagrees
with the model is worse than no picture, because it looks authoritative.

Alternatives considered and **not** chosen: GraphHopper custom model, Valhalla
`avoid_polygons`. Rejected in favour of the hand-rolled Rust pass because it
keeps everything on-device, dependency-free, and fully under our control. Revisit
only if the custom router can't keep up.

---

## 5. Caveats to keep in the UI (non-negotiable)

Both live in `MapScreen.kt`:

1. "Shows only cameras **mapped in OpenStreetMap**." (Real coverage is higher.)
2. "Avoiding cameras is **not anonymity**." (And a route that conspicuously weaves
   around every lens can itself be a signal.)
3. **"© OpenMapTiles © OpenStreetMap contributors"** — an attribution
   obligation, not a caveat: OSM data is ODbL and the OpenMapTiles schema the
   basemap is generated with is CC-BY, both of which require a visible credit.
   Planetiler prints this requirement at the end of every tile build. Bundling
   the tiles offline does not exempt us. If the basemap is ever regenerated
   from a different schema, update the credit to match rather than dropping it.

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

**Known perf follow-up:** the router is ready ~6 s after launch on the real
Berlin extract (style loads in ~80 ms by comparison) — that gap is the one-off
exposure pass over every edge, and it is paid on every cold start. Caching the
scored graph rather than re-deriving it is the fix; `rstar` in place of the
grids is the smaller lever.

**Deliberately deferred:** cycling profile, location puck ("centre on me"),
search/geocoding, camera FOV tuning against ground truth, Play Services
fallback build.
