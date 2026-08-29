# Schattenweg

A privacy-first Android app that maps Berlin's CCTV cameras and plans walking
routes that **avoid** them. Rust core, Kotlin/Compose UI, no Google, no network, no permissions, fully offline.

> *Schattenweg* — the shadow path.

## What it does

- Plots `man_made=surveillance` cameras (from OpenStreetMap) on an offline map,
  drawing each camera's modelled field of view; tap one for its details.
- Plans a route from A to B that trades detour length against camera exposure,
  set with a three-way **Low / Medium / High** avoidance control. When a
  camera-free route exists, a freshly dropped A→B pair defaults to it.
- Search streets and places, toggle map layers, and zoom — all offline.
- Runs entirely on-device: no server sees your location or your routes. The
  place search reads a bundled index, so even typing a destination leaks
  nothing.

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
│       ├── MapScreen.kt       # map + camera layer + avoidance control
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

## Download

Pre-built APKs are attached to each release:
**[github.com/phrag/SchattenWeg/releases/latest](https://github.com/phrag/SchattenWeg/releases/latest)**
(the in-app credit at the bottom of the map opens the same page).

The release workflow generates the offline Berlin data during the build (see
`.github/workflows/release.yml`), so those APKs **map and route out of the
box**. They are **debug**-signed with the `.debug` application-id suffix,
though — installable and usable, but not signed with a release key. For a
release-key-signed APK, build from source with the keystore in place (below).

(The separate `CI` workflow that runs on every push builds an asset-free debug
APK — a fast compile check — which renders on a plain background. Releases are
the builds meant for installing.)

## Build

**Prerequisites:** `rustup`, `cargo-ndk` (`cargo install cargo-ndk`), the
Android SDK/NDK, JDK 17+, and — for the data pipeline — `osmium` plus Java 21+
for Planetiler.

The Rust toolchain version and both Android targets are pinned in
`rust-toolchain.toml`, so rustup installs them for you on the first build; you
do not need `rustup target add`. A floating toolchain is what this pin
prevents: the lockfile's dependencies impose their own minimum rustc, so
"whatever stable you have" is not a build specification.

1. **Build map data + tiles**
   ```bash
   ./scripts/build_map_assets.sh     # → data/berlin-routing.osm.pbf + offline tiles
   ```
   It downloads ~70 MB from Geofabrik, verifies it against the published MD5,
   filters it, and renders tiles with Planetiler. Finished files are kept, so
   re-running after a failure only fetches what's missing.

   If a download fails, the error names the step, the URL and the HTTP status.
   A `5xx` is the download host having a bad day — wait and re-run. Useful
   knobs:

   | Variable | Effect |
   |---|---|
   | `EXTRACT_URL=<url>` | Fetch the extract from a mirror instead |
   | `SKIP_TILES=1` | Stop after the routing snapshot — the app still routes, on a plain background |
   | `PLANETILER_VERSION=vX.Y.Z` | Pin a different Planetiler release |
   | `RETRIES=<n>` | Download attempts per file (default 5) |

2. **Core tests** (pure Rust, no Android needed)
   ```bash
   cd core && cargo test
   ```

3. **Try routing from the terminal** — sweeps λ so you can see the trade-off
   ```bash
   cd core && cargo run --release --example plan_route ../data/berlin-routing.osm.pbf
   ```
   No Berlin extract yet? The bundled test fixture works too:
   ```bash
   cd core && cargo run --release --example plan_route -- \
       tests/fixtures/mini_berlin.osm.pbf 52.5200,13.4000 52.5200,13.4040
   #     λ     length    exposure
   #     0      271 m       15.0%     ← straight down the watched street
   #     8      493 m        0.0%     ← detours around the camera
   ```

4. **Build the app** (compiles the Rust core for Android and generates the
   UniFFI Kotlin bindings automatically)
   ```bash
   ./gradlew :app:assembleDebug
   ```

### Blank map?

MapLibre 13.x renders with **Vulkan**, which many emulators do not usefully
support — the map then draws nothing at all, basemap *and* overlays, which
looks like missing data but is not. Build the OpenGL ES variant instead:

```bash
./gradlew :app:assembleDebug -PmaplibreBackend=opengl
```

The app logs the basemap it found and any MapLibre load failure. Watch it with:

```bash
adb logcat -s Schattenweg:V Mbgl:V vulkan:V
```

## Contributing

Enable the repo's hooks once per clone, so machine-local paths, email
addresses and credentials cannot be committed:

```bash
git config core.hooksPath .githooks
```

CI enforces the same rules on every push (the `hygiene` job).

Kotlin style is checked with [ktlint](https://pinterest.github.io/ktlint/),
configured for Compose in `.editorconfig`. Format before pushing — CI's
`ktlint` job fails otherwise:

```bash
ktlint -F "app/src/main/java/de/schattenweg/**/*.kt" "*.gradle.kts" "app/*.gradle.kts"
```

## Privacy posture

- **No Google Play Services, no Firebase, no analytics.** Targets de-Googled
  devices (GrapheneOS) as a first-class case. minSdk 29 (Android 10).
- **MapLibre** with bundled offline vector tiles — not the Google Maps SDK, and
  no tile server sees your viewport. (The MapLibre AAR was audited: no
  telemetry or analytics classes.)
- Routing and scoring are **fully offline**; the app ships with a bundled Berlin
  data snapshot and **declares no `INTERNET` permission** — MapLibre's own
  `INTERNET`, `ACCESS_WIFI_STATE` and location permissions are stripped in the
  manifest merge, so the app requests no dangerous permission at all.
- Positioning (if/when used) via the platform **LocationManager (GNSS)**, never
  the Fused Location Provider.
- Signing keys never enter this repo; release builds are left unsigned rather
  than falling back to the forgeable debug key.

Full threat model, including what this app explicitly does **not** protect
against: [SECURITY.md](SECURITY.md).

## Honesty

The map shows only cameras **mapped in OpenStreetMap** — real coverage is higher.
Avoiding mapped cameras reduces exposure; it is **not** anonymity. Both caveats
are surfaced in the UI on purpose.

## Licence

GPL-3.0-or-later. Map data © OpenStreetMap contributors, available under the
[Open Database Licence](https://www.openstreetmap.org/copyright).
