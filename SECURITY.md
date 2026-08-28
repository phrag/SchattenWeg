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
| Another app on the device | No exported components except the launcher activity. No content providers, no exported services or receivers. Map data lives in app-private `filesDir`. |
| Someone with the unlocked device | Out of scope — see below. |
| Someone extracting an ADB backup | `android:allowBackup="false"`; the app's data is excluded from backup and from `adb backup`. |
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
- The Planetiler JAR is downloaded from its GitHub release without checksum
  verification — a known gap. Pin and verify it if your threat model includes
  a compromised release asset.
- The map data pipeline is the only network step in the project, and it runs
  on a build machine — never on the phone.
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
users at risk if public, contact the maintainer privately first.
