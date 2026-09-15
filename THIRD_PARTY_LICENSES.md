# Third-party licences

Schattenweg itself is **GPL-3.0-or-later** (see [`LICENSE`](LICENSE)). The app
and its data bundle the third-party work listed below. Each entry links to the
upstream project, where the full licence text lives; the licences are all
compatible with distributing Schattenweg under the GPL.

The **in-app About section** (☰ → About) links here, and to the OpenStreetMap
and OpenMapTiles licences directly. The two licences that require their text to
be distributed with the binary — **MapLibre GL Native (BSD-2-Clause)** and
**Noto Sans (SIL OFL 1.1)** — are bundled in the app and shown verbatim, fully
offline, on the **Open-source licences** screen (☰ → About → Open-source
licences).

## Bundled in the APK — code

| Component | Used for | Licence |
|-----------|----------|---------|
| [MapLibre GL Native — Android SDK](https://github.com/maplibre/maplibre-gl-native) (`org.maplibre.gl:android-sdk` / `-opengl`) | Rendering the offline map | BSD-2-Clause |
| [JNA](https://github.com/java-native-access/jna) (`net.java.dev.jna:jna`) | Loading the Rust core from the UniFFI bindings | Apache-2.0 **or** LGPL-2.1 (dual) |
| [AndroidX Compose, Activity, Lifecycle](https://developer.android.com/jetpack/androidx) | UI toolkit | Apache-2.0 |
| [Kotlin standard library](https://github.com/JetBrains/kotlin) | Language runtime | Apache-2.0 |

## Bundled in the APK — the Rust core (`schattenweg-core`)

Compiled into the bundled native library. Direct dependencies:

| Crate | Used for | Licence |
|-------|----------|---------|
| [uniffi](https://github.com/mozilla/uniffi-rs) | The Kotlin ⇄ Rust FFI | MPL-2.0 |
| [osmpbf](https://github.com/b-r-u/osmpbf) | Reading the OSM `.pbf` extract | MIT **or** Apache-2.0 |
| [thiserror](https://github.com/dtolnay/thiserror) | Error types | MIT **or** Apache-2.0 |

(Plus their transitive dependencies, each carrying its own permissive licence —
`cargo tree` in `core/` lists the full set.)

## Bundled in the APK — data & assets

| Asset | Source | Licence |
|-------|--------|---------|
| Camera, street & place data; the derived routing snapshot | [OpenStreetMap](https://www.openstreetmap.org/copyright) | **ODbL 1.0** |
| Offline basemap (vector tiles) | Generated with [Planetiler](https://github.com/onthegomap/planetiler) from the [OpenMapTiles](https://openmaptiles.org/) schema | Schema **CC-BY 4.0**; data **ODbL 1.0** |
| Label glyphs | [Noto Sans](https://github.com/notofonts/noto-fonts) (Latin ranges only) | SIL OFL 1.1 |

### About the OpenStreetMap data

The bundled Berlin extract (`data/berlin-routing.osm.pbf`) is a **derived
database** produced from OpenStreetMap. Under the ODbL's share-alike terms it is
offered under the **[ODbL 1.0](https://opendatacommons.org/licenses/odbl/1-0/)**.
You can regenerate it from current OpenStreetMap data with
[`scripts/build_map_assets.sh`](scripts/build_map_assets.sh), or take it from the
assets attached to a [GitHub release](https://github.com/phrag/SchattenWeg/releases).

## Build-time tools (not shipped in the APK)

[Planetiler](https://github.com/onthegomap/planetiler) (Apache-2.0) generates the
offline tiles, and [osmium](https://osmcode.org/osmium-tool/) (Boost Software
Licence) filters the extract. Both run only during `build_map_assets.sh`.
