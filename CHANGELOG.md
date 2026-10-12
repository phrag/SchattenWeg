# Changelog

Each `## <version>` section is used as the top of that release's notes by
`.github/workflows/release.yml` (the tag `v0.2.2` picks up `## 0.2.2`).
Add the section before pushing the tag.

## 0.2.2

- **Much smaller download: about 111 MB, down from 308 MB at 0.2.0.**
  The scored route cache is compressed, the release build is arm64-only,
  the offline map tiles drop layers the style never draws, and the 59 MB
  OpenStreetMap extract is no longer bundled (the pre-built cache is the
  routing data). Updating over an older install deletes the old extract, so
  expect roughly 59 MB less storage.
- The rolling `latest` build is now rebuilt every Monday so its camera data
  stays current, and is signed with the release key.

## 0.2.1

- The keyboard is asked not to suggest or learn from what you search for.
- The compass is now a button in the right-hand controls (tap to face north)
  instead of drawing over the status bar.
- The About panel shows how fresh the camera data is ("Camera data:
  OpenStreetMap, <date>").
- Compressed route cache and arm64-only release build (first size cut).
- Simpler install instructions in the README, with a direct download link.

## 0.2.0

First build signed with the release key (certificate fingerprint in
SECURITY.md).

- Buildings now block camera sight lines, in both the exposure score and the
  coverage drawn on the map; indoor cameras are ignored.
- Routes avoid Berlin's police AI-video zones.
- 3D buildings layer, on by default.
- Every new route starts at High avoidance.
- On-device place search and a pre-built route cache for fast first launch.
