# Signing a release build

The release build is **unsigned unless you supply a key** (see `SECURITY.md`);
it never falls back to the public debug key. The key is yours alone: generate
it on your own machine, never in a cloud/CI session, and never commit it.

## 1. Generate the key (once)

```sh
keytool -genkeypair -v \
  -keystore ~/.android-keys/schattenweg-release.jks \
  -alias schattenweg \
  -keyalg RSA -keysize 4096 -validity 10000
```

Pick strong passwords. Then **back the `.jks` file and both passwords up
offline** (password manager or encrypted drive). If the key is lost, nobody
who installed a build signed with it can receive updates.

## 2. Sign locally

Copy `keystore.properties.example` to `keystore.properties` (gitignored) in the
repo root, point `storeFile` at the `.jks` outside the repo, fill in the
passwords and alias, then:

```sh
./gradlew :app:assembleRelease
```

The APK lands in `app/build/outputs/apk/release/`. Check it with
`apksigner verify --print-certs <apk>`.

## 3. Sign in GitHub Actions (optional)

`release.yml` builds an extra signed APK when these repository secrets exist
(Settings → Secrets and variables → Actions). Without them it skips the step
and publishes only the debug-signed APK, as before.

| Secret | Value |
|---|---|
| `SCHATTENWEG_KEYSTORE_B64` | `base64 -w0 schattenweg-release.jks` |
| `SCHATTENWEG_STORE_PASSWORD` | keystore password |
| `SCHATTENWEG_KEY_ALIAS` | `schattenweg` |
| `SCHATTENWEG_KEY_PASSWORD` | key password |

The keystore is decoded to the runner's temp directory for one Gradle call and
removed afterwards. Secrets are not exposed to runs from forks. The trade-off:
GitHub now holds your signing key. If you would rather not, skip this section
and sign locally, then attach the APK to the release yourself.

The signed APK has no `.debug` application-id suffix, so it installs alongside
the debug build. Because release uses R8 and resource shrinking, test a signed
build on a device before relying on it.
