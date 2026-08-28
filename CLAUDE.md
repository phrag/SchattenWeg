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
  no `INTERNET` permission.

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
- [ ] `core/src/osm.rs::load_cameras` / `load_graph` — PBF ingest via `osmpbf`.
- [ ] Spatial grid for `CameraIndex::any_covers` and `Graph::nearest_node`
      (required at Berlin scale — the linear scans don't survive ~10⁶ edge
      samples × 10³ cameras).
- [ ] `core/examples/plan_route.rs` CLI demo on the real Berlin snapshot.
- [ ] `scripts/build_map_assets.sh`: Geofabrik → filtered snapshot + Planetiler
      offline tiles.
- [ ] Android app: Gradle + cargo-ndk + UniFFI bindings + MapLibre map screen
      (offline tiles, camera layer, tap-to-route, paranoia slider).
- [ ] CI: Rust fmt/clippy/test + Android assembleDebug.
