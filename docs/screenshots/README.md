# Screenshots

Home for real screenshots of the app. The main [`README.md`](../../README.md)
deliberately has **no gallery right now**: it linked images that were never
committed, which render as broken icons on GitHub. Once real captures exist,
add them here with the names below **and** put the gallery back in the README
(`<img src="docs/screenshots/01-map-cameras.png" …>`) in the same commit, so
the page never points at a file that isn't there.

Currently committed and shown in the README: `01-route.png`,
`02-ai-zone-kottbusser-tor.png`, `03-ai-zone-goerlitzer-park.png`.

Still wanted:

| File | What to capture |
|------|-----------------|
| `01-map-cameras.png` | The map zoomed into central Berlin, camera dots visible with their coverage wedges/discs drawn. |
| `02-route.png` | A planned walking route (the coloured line) steering around cameras, with the **Low / Medium / High** avoidance control visible. |
| `03-search.png` | The **"Search a street or place"** box in use, with a result or two showing. |
| `04-layers-about.png` | The layers (☰) panel open, showing the Cameras / Coverage / Labels toggles and the **About** section with the OpenStreetMap credit. |

## How to take them

Framed phone screenshots read best. From a device or emulator running the
app:

```bash
# capture whatever is on screen to this folder
adb exec-out screencap -p > docs/screenshots/01-map-cameras.png
```

Tips for good screenshots:

- Use a **portrait** phone screen — the gallery is laid out for tall images.
- Zoom the map so individual cameras and their coverage shapes are clearly
  visible, not a city-wide blur.
- Keep them a similar size to each other so the gallery row lines up.
- PNG is preferred; keep each file under ~1 MB if you can (resize to ~1080 px
  wide is plenty).

Only reference a screenshot from the README once the file is committed.
