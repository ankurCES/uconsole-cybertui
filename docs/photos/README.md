# docs/photos

Drop your numbered photos of cyberdeck running on the uconsole into this
directory. Filenames follow the pattern `photo-NN.jpg` (or `.png`) so the
references in [`docs/wiki/Photos.md`](../wiki/Photos.md) and the README
keep working without edits.

## Numbered photo slots

| Slot | What to capture | Reference |
| ---- | --------------- | --------- |
| `photo-01.jpg` | The TUI on the uconsole — sidebar + a status screen | README "Screenshots" section, `docs/wiki/Photos.md` |
| `photo-02.jpg` | A split WM: a status screen on one side, a live terminal pane on the other (different colours so ANSI parsing is visible) | README "Screenshots", `docs/wiki/Photos.md`, `docs/wiki/Phase-3-WM.md` |
| `photo-03.jpg` | A modal in flight — pick whichever one (`Secret`, `Choice`, `Wizard`, `Progress`, or `AuthFailure`) | README "Screenshots", `docs/wiki/Phase-5-Modals.md` |
| `photo-04.jpg` | The web UI in a browser tab (with bearer token overlay or the unauthenticated landing) | README "Screenshots", `docs/wiki/Architecture.md` |

## How to add a photo

1. Take the photo. If it's a phone photo, crop to the screen so the
   uconsole device frame isn't dominating the frame.
2. Save it into this directory as `photo-NN.jpg` (or `.png`).
3. Optional: open a PR titled `photos: add photo-NN (description)`.
4. The README and `docs/wiki/Photos.md` already reference these slots,
   so no other file changes are needed.

## Where these photos are referenced

- [`../README.md`](../README.md) — top-level "Screenshots" section.
- [`../wiki/Photos.md`](../wiki/Photos.md) — full numbered index with
  captions.

## Tips for good photos

- **Light.** The TUI is dark by default; either take the photo in a
  dimmer room or lower the screen brightness a notch.
- **Crop.** Show the screen, not the keyboard (unless the keyboard is
  the story — `Ctrl-W` and the `^P` palette are nice to capture).
- **Real state.** A photo of "live" data is more useful than a photo
  of an empty launcher. `nmcli dev wifi`, `pactl list sinks`, and
  `top` are all good subjects.
- **Panes.** For the WM shot, type something that produces ANSI
  colours (e.g. `ls -la /usr/bin | head -20` or `htop`). Plain
  black-on-white terminals are boring.

## File-size sanity

Keep each photo under ~1 MB so the repo stays small. The README and
wiki embed them via relative paths, so they're served from the repo
itself when the GitHub wiki is synced.

## License for photos

By dropping a photo into this directory you agree to license it under
the same MIT terms as the rest of the project (or, if you prefer, open
a PR adding a `docs/photos/LICENSE-photos.md` with your preferred
license — most contributors just use MIT).
