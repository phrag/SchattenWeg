# Schattenweg

A privacy-first Android app that maps Berlin's CCTV cameras and plans walking
routes that **avoid** them. Rust core, Kotlin/Compose UI, no Google.

> *Schattenweg* — the shadow path.

## What it does

- Plots `man_made=surveillance` cameras (from OpenStreetMap) on an offline map.
- Plans a route from A to B that trades detour length against camera exposure,
  controlled by a single "paranoia" slider (λ).
- Runs entirely on-device: no server sees your location or your routes.

## Architecture at a glance

```
SchattenWeg/
├── core/                     # Rust: OSM ingest, exposure scoring, routing (UniFFI)
│   └── src/
│       ├── camera.rs         # camera model + field-of-view geometry
│       ├── exposure.rs       # per-edge surveillance-exposure scoring
│       ├── routing.rs        # camera-aware A*
│       ├── osm.rs            # OSM tag → model mapping (ingest)
│       └── lib.rs            # the small UniFFI surface Kotlin calls
├── app/                      # Android: Kotlin + Jetpack Compose + MapLibre
│   └── src/main/java/de/schattenweg/app/
│       ├── MainActivity.kt
│       ├── MapScreen.kt       # map + camera layer + paranoia slider
│       └── RouteViewModel.kt  # bridges Compose ⇄ Rust core
├── scripts/
│   ├── fetch_cameras.sh       # Overpass camera fetch (GeoJSON preview)
│   └── build_map_assets.sh    # Geofabrik → routing snapshot + offline tiles
├── CLAUDE.md                  # project context + decisions (read this first)
└── settings.gradle.kts
```

The routing idea in one line:

```
edge weight = length_m * (1 + λ * exposure)
```

where `exposure ∈ [0,1]` is the fraction of a road segment inside any camera's
modelled field of view. `λ=0` is a normal shortest path; larger `λ` buys quieter
routes with longer detours.

## Build

**Prerequisites:** Rust (stable) with Android targets, `cargo-ndk`, the Android
SDK/NDK, JDK 17+, and (for the data pipeline) `osmium` + Java for Planetiler.

1. **Build map data + tiles**
   ```bash
   ./scripts/build_map_assets.sh     # → data/berlin-routing.osm.pbf + offline tiles
   ```

2. **Core tests** (pure Rust, no Android needed)
   ```bash
   cd core && cargo test
   ```

3. **Try routing from the terminal**
   ```bash
   cd core && cargo run --release --example plan_route ../data/berlin-routing.osm.pbf
   ```

4. **Build the app** (compiles the Rust core for Android and generates the
   UniFFI Kotlin bindings automatically)
   ```bash
   ./gradlew :app:assembleDebug
   ```

## Privacy posture

- **No Google Play Services, no Firebase, no analytics.** Targets de-Googled
  devices (GrapheneOS) as a first-class case. minSdk 29 (Android 10).
- **MapLibre** with bundled offline vector tiles — not the Google Maps SDK, and
  no tile server sees your viewport.
- Positioning (if/when used) via the platform **LocationManager (GNSS)**, never
  the Fused Location Provider.
- Routing and scoring are **fully offline**; the app ships with a bundled Berlin
  data snapshot and declares no `INTERNET` permission.

## Honesty

The map shows only cameras **mapped in OpenStreetMap** — real coverage is higher.
Avoiding mapped cameras reduces exposure; it is **not** anonymity. Both caveats
are surfaced in the UI on purpose.

## Licence

GPL-3.0-or-later. Map data © OpenStreetMap contributors, available under the
[Open Database Licence](https://www.openstreetmap.org/copyright).
