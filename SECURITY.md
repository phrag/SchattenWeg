# Security & privacy posture

Schattenweg is a surveillance-avoidance tool. A tool like that which phones
home, leaks a route, or ships a forgeable build is worse than useless — it is
actively dangerous to the person trusting it. This file records what the app
does about that, and what it deliberately does not promise.

## Threat model

**What we defend against**

| Adversary | Defence |
|---|---|
| Network observer (ISP, Wi-Fi operator, hostile AP) | The app makes no network requests at all — it holds no `INTERNET` permission. There is no traffic to observe. |
| A backend operator (including us) | There is no backend. Routing, scoring and map rendering are entirely on-device. We never learn a start, a destination, or that you opened the app. |
| Tile provider correlating viewports | Basemap tiles are bundled offline; no tile server sees where you pan. |
| Google / Play Services telemetry | No Play Services, no Firebase, no analytics, no crash reporter. The MapLibre AAR was audited: it contains no telemetry or analytics classes. |
| Another app on the device | The only exported activity is the launcher. The APK also carries two AndroidX components: `InitializationProvider` (not exported) and `ProfileInstallReceiver` (exported, but only the system shell holding the `DUMP` permission can trigger it). No other providers, services or receivers. Map data lives in app-private `filesDir`. |
| Someone with the unlocked device | Out of scope — see below. |
| Someone extracting a cloud or ADB backup | `android:allowBackup="false"`; the app's data is excluded from cloud backup and from `adb backup`. (On Android 12+ this flag does not govern device-to-device transfer, which is controlled by `dataExtractionRules`; the app does not set them.) |
| A tampered or forged build | Release builds are signed with a key that never touches this repo; unsigned is the failure mode if the key is absent (see below). |

**What we explicitly do NOT defend against**

- **This is not anonymity.** Avoiding mapped cameras lowers exposure to *those
  cameras*. It does nothing about your phone's radios, your face, ALPR, or a
  human tail. A route that conspicuously weaves around every lens can itself
  be a signal.
- **Coverage is partial.** OpenStreetMap has a fraction of the real cameras.
  An unmarked street is not a safe street. Both caveats are on screen in the
  app on purpose, and must stay there.
- **Device compromise.** A rooted/malware-infected device, or an unlocked
  phone in someone else's hands, defeats everything here. Use full-disk
  encryption and a strong lockscreen.
- **Camera FOV numbers are modelling assumptions**, not ground truth from OSM
  (see `core/src/camera.rs::defaults`). They are conservative guesses.

## Signing keys — never in this repository

`.gitignore` refuses `*.jks`, `*.keystore`, `*.p12`, `*.pem`, `*.key`,
`keystore.properties`, `key.properties`, `signing.properties` and
`local.properties`.

Release signing credentials are read from an untracked `keystore.properties`
in the repo root, or from the environment in CI:

```properties
# keystore.properties — NEVER commit this file
storeFile=/absolute/path/outside/the/repo/schattenweg-release.jks
storePassword=…
keyAlias=schattenweg
keyPassword=…
```

or `SCHATTENWEG_STORE_FILE`, `SCHATTENWEG_STORE_PASSWORD`,
`SCHATTENWEG_KEY_ALIAS`, `SCHATTENWEG_KEY_PASSWORD`.

Keep the keystore itself **outside the working tree**. If credentials are
missing the release build produces an *unsigned* APK — deliberately, rather
than falling back to the public debug key, which would look signed while being
trivially forgeable.

**CI release signing.** A `v*` tag build decodes the keystore from the repo
secrets `SCHATTENWEG_KEYSTORE_BASE64`, `SCHATTENWEG_STORE_PASSWORD`,
`SCHATTENWEG_KEY_ALIAS` and `SCHATTENWEG_KEY_PASSWORD` into the runner's temp
dir, builds `assembleRelease`, verifies the signature with `apksigner` and
deletes the key. If any secret is missing the tag build fails instead of
publishing a debug-signed "release". The same applies to pushes to `main`,
whose rolling `latest` build is signed with the release key too (anyone who can
push to `main` can therefore get code signed: protect the branch). Manual runs
from other branches stay debug-signed and never receive the secrets. Keep an offline backup of the
keystore and its passwords: Android will not accept updates signed by a
different key, so losing it means a new app identity.

Release certificate SHA-256 fingerprint (compare against `apksigner verify
--print-certs` on any release APK):

```
0B:E2:F0:AD:D9:1F:66:94:8C:65:A6:5B:01:56:BA:46:17:F6:4E:17:4B:7C:92:93:7B:BA:1A:FE:CE:E2:2A:FC
```

If a key is ever committed: rotate it. Rewriting history is not enough — treat
any pushed key as burned.

## Build & supply chain

- All dependency versions are pinned in `gradle/libs.versions.toml` and
  `core/Cargo.lock`; no dynamic (`+`, `latest.release`) versions.
- The Gradle wrapper JAR is validated in CI by
  `gradle/actions/wrapper-validation`.
- CI fails the build if any Play Services / Firebase artifact appears in the
  dependency tree.
- `scripts/build_map_assets.sh` verifies the Geofabrik download against the
  published MD5 before using it. Two escape hatches deliberately weaken that
  and both say so loudly when used: `SKIP_CHECKSUM=1`, and `EXTRACT_URL=<url>`
  (a mirror has no Geofabrik checksum to compare against — verify such a
  source yourself).
- The Planetiler JAR is checked against the `.sha256` file published beside it
  in the same GitHub release. That catches a corrupted download or a poisoned
  cache, but not a compromised release, since the hash comes from the same
  place. Pin the hash in the script if your threat model includes that.
- Nightly and `main` builds also pull the camera nodes live from Overpass
  (`LIVE_CAMERAS=1`). That data has no integrity check beyond a plausibility
  test (fewer than half the extract's cameras falls back to the extract).
  Tagged (Stable) builds keep the extract's cameras.
- The map data pipeline (Geofabrik, Overpass, the Planetiler download) is the
  only part of the project that talks to data hosts, and it runs on a build
  machine — never on the phone. Dependency resolution (Gradle, Cargo) also
  needs the network at build time.
- Maintainers with network access to Gradle's CDN should add
  `distributionSha256Sum` to `gradle/wrapper/gradle-wrapper.properties`
  (published at `<distributionUrl>.sha256`).

## Memory safety

All parsing of untrusted input — the OSM PBF extract — happens in Rust
(`core/`), behind a narrow, value-typed UniFFI boundary. The crate contains no
`unsafe` blocks of our own; the only unsafe code is UniFFI's generated
scaffolding.

## Reporting

Open a GitHub issue for non-sensitive matters. For anything that would put
users at risk if public, use GitHub's private vulnerability reporting (the
repository's Security tab → "Report a vulnerability") rather than a public
issue.
