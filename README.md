# Schattenweg

A privacy-first Android app that maps Berlin's CCTV cameras and plans walking
routes that **avoid** them — trading a little extra walking for less time in
view of a lens.

> *Schattenweg* — the shadow path.

It works **completely offline**. The map, the camera data, the search box and
the routing all live on your phone. Nothing you do — where you are, where you're
going, the route you take — ever leaves the device. The app doesn't even hold
the permission to reach the internet.

---

## Download & install

1. Open the **[Releases page](https://github.com/phrag/SchattenWeg/releases)** and
   download the most recent `.apk`.
   - The rolling **`latest`** build tracks the newest tested commit; tagged
     builds like `v0.1.0` are fixed versions. Either is fine.
2. Copy it to your phone and tap it. Android will ask you to allow installing
   from this source — that's the normal sideloading prompt for an app that
   isn't from a store.
3. Open **Schattenweg**. No sign-in, no setup, no network — it's ready.

**Requirements:** Android 10 (API 29) or newer. Works on de-Googled devices such
as **GrapheneOS** — there are no Google Play Services to miss.

> The APKs in Releases are **debug-signed**: they install and run, and they're
> the easiest way to try the app. They aren't a store-grade signed build. If you
> prefer, you can [build from source](#build-from-source) instead.

Right now the data covers **Berlin only**.

---

## What it does

- **Shows the cameras.** Every `man_made=surveillance` camera OpenStreetMap
  knows about in Berlin, drawn on an offline map with its modelled field of
  view. Tap one to see its details.
- **Routes around them.** Pick a start and a destination and Schattenweg finds a
  walking route that stays out of camera view where it reasonably can. One
  **"paranoia" slider** sets how much extra walking you'll accept to dodge a
  lens: all the way down is the normal shortest path; higher values take quieter
  detours.
- **Finds places offline.** Search streets, neighbourhoods and stations from a
  bundled index — so even typing a destination reveals nothing to anyone.
- **Everything above happens on your phone**, with no connection.

---

## Your privacy

This is the whole point of the app, so it's worth being explicit.

- **No internet permission at all.** The app doesn't declare `INTERNET`. It
  *can't* phone home, send analytics, or fetch a map tile mid-route, because
  Android won't let it open a socket. A surveillance-avoidance tool that quietly
  talked to a server would defeat its own purpose.
- **Fully offline.** The Berlin map, camera data, place-search index and the
  routing engine are all bundled in the app and run locally.
- **Your location stays on the device.** Positioning uses the platform's own GPS
  (`LocationManager`), never Google's Fused Location Provider — which routes
  through Google's servers.
- **No Google, no Firebase, no analytics, no ad SDKs.** The map is drawn with
  MapLibre from bundled vector tiles, not the Google Maps SDK, so no tile server
  ever sees where you're looking. (The MapLibre library was audited: no
  telemetry classes, and its bundled `INTERNET`/Wi‑Fi/location permissions are
  stripped out during the build.)
- **Nothing is backed up off-device.** Android cloud backup is disabled for the
  app.

The full threat model — including what this app deliberately does **not**
protect against — is in **[SECURITY.md](SECURITY.md)**.

---

## Honesty about the limits

- The map shows only cameras **mapped in OpenStreetMap**. Real-world coverage is
  higher — treat an empty street as "unknown", not "unwatched".
- Avoiding mapped cameras **reduces exposure; it is not anonymity.** A route that
  conspicuously weaves around every lens can itself draw attention.

Both of these are shown inside the app, on purpose.

---

## Build from source

Prefer to build it yourself, or want to hack on it? Everything you need is here.

**Prerequisites:** `rustup`, `cargo-ndk` (`cargo install cargo-ndk`), the Android
SDK/NDK, JDK 17+, and — for the offline map data — `osmium` plus Java 21+ (for
Planetiler).

The Rust toolchain and both Android targets are pinned in `rust-toolchain.toml`,
so rustup installs them on the first build; you don't need `rustup target add`.

1. **Build the map data + tiles**
   ```bash
   ./scripts/build_map_assets.sh     # → data/berlin-routing.osm.pbf + offline tiles
   ```
   It downloads ~70 MB from Geofabrik, verifies it against the published MD5,
   filters it to streets + cameras, and renders offline tiles with Planetiler.
   Finished files are kept, so re-running after a failure only fetches what's
   missing. Useful knobs:

   | Variable | Effect |
   |---|---|
   | `REFRESH=1` | Discard a cached extract and fetch the **current** one, so the bundled cameras are the latest OSM has |
   | `EXTRACT_URL=<url>` | Fetch the extract from a mirror instead |
   | `SKIP_TILES=1` | Stop after the routing snapshot — the app still routes, on a plain background |
   | `PLANETILER_VERSION=vX.Y.Z` | Pin a different Planetiler release |
   | `RETRIES=<n>` | Download attempts per file (default 5) |

   It ends by writing `data/build-info.txt` (OSM snapshot date + camera count) so
   a build can state exactly how current its cameras are.

2. **Run the core tests** (pure Rust, no Android needed)
   ```bash
   cd core && cargo test
   ```

3. **Try routing from the terminal** — sweeps the paranoia dial (λ) so you can
   see the trade-off
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

MapLibre 13.x renders with **Vulkan**, which many emulators don't usefully
support — the map then draws nothing at all, basemap *and* overlays, which looks
like missing data but isn't. Build the OpenGL ES variant instead:

```bash
./gradlew :app:assembleDebug -PmaplibreBackend=opengl
```

The app logs the basemap it found and any MapLibre load failure:

```bash
adb logcat -s Schattenweg:V Mbgl:V vulkan:V
```

### How routing works, in one line

```
edge weight = length_m * (1 + λ * exposure)
```

where `exposure ∈ [0,1]` is the fraction of a road segment inside any camera's
modelled field of view, and λ is the paranoia slider. `λ=0` is a normal shortest
path; larger λ buys quieter routes with longer detours.

### Layout

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

### Releases

Pushes to `main` publish a rolling **`latest`** debug APK; pushing a `v*` tag
publishes a versioned one. Both regenerate the map assets from a freshly
downloaded OSM extract, so a released APK always carries the latest cameras. See
[`.github/workflows/release.yml`](.github/workflows/release.yml).

## Contributing

Enable the repo's hooks once per clone, so machine-local paths, email addresses
and credentials can't be committed:

```bash
git config core.hooksPath .githooks
```

CI enforces the same rules on every push (the `hygiene` job).

## Licence

GPL-3.0-or-later. Map data © OpenStreetMap contributors, available under the
[Open Database Licence](https://www.openstreetmap.org/copyright).
