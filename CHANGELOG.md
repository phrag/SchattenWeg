# Changelog

Each `## <version>` section is used as the top of that release's notes by
`.github/workflows/release.yml` (the tag `v0.2.2` picks up `## 0.2.2`).
Add the section before pushing the tag.

## 0.2.3

- **Camera range is now labelled as an assumption, and two Axis models use their datasheets.** The card says the range is roughly face-recognition distance. Cameras tagged AXIS Q3839-PVE (180° panorama, 35 m) or Q1809-LE (90°, 60 m) use those figures instead of the generic 20–30 m and 60° cone; this changes routes around Kottbusser Tor.
- **AI camera coverage is violet and has its own switch.** The wedges and discs of
  cameras tagged as running AI analysis now match their violet dots, and a new
  "AI camera coverage" layer toggle shows or hides them (on by default). Routing
  is unchanged.
- **AI cameras are marked on the map.** Cameras OpenStreetMap tags as running
  AI analysis (`camera:features`) are drawn in violet, and their card says
  so. This covers the 30 police AI cameras now mapped at Kottbusser Tor and the
  face-detection cameras from the Südkreuz pilot. Routing avoids them exactly
  like any other camera.
- Camera directions written as German compass points (`O`, `NO`, `SO`) are now
  understood, so those cameras are drawn as wedges instead of full circles.
- Fresh OpenStreetMap camera data.
- The **Nightly** build (the rolling `latest` download) is now rebuilt every
  night instead of weekly, so its cameras are at most a day behind
  OpenStreetMap. The README offers Stable and Nightly side by side, each with
  its build date and camera count.

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
