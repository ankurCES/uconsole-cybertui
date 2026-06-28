# Photos

Drop your numbered photos of cyberdeck running on the uconsole into
[`docs/photos/`](../photos/) and reference them here. Filenames follow
the pattern `photo-NN.jpg` (or `.png`) so the references below keep
working without edits.

## Numbered photo index

### `photo-01.jpg` — the launcher / sidebar

A clean shot of the TUI showing the sidebar with all 13 screens, the
live header (clock, CPU, mem, BAT, ssid), and a status screen on the
right (System is a good default).

Caption suggestion: *"cyberdeck on uconsole · 13 screens, one keystroke each."*

### `photo-02.jpg` — split WM in action

A split workspace: a status screen on one side, a live `$SHELL` pane
on the other. Type something that produces ANSI colours before the
shot (e.g. `ls -la /usr/bin | head -20` or `htop`) so the terminal
pane shows colour codes.

Caption suggestion: *"split tree · Ctrl-W v/s to split, Ctrl-W 1..9 to jump."*

### `photo-03.jpg` — a modal in flight

Pick whichever modal you like — Secret (Wi-Fi password), Choice
(service picker), Wizard (new Wi-Fi connection), Progress
(`apt upgrade`), or AuthFailure (sudo prompt). The modal should be
visible over the underlying screen with the dim layer showing.

Caption suggestion: *"modal system · Secret / Choice / Wizard / Progress / AuthFailure."*

### `photo-04.jpg` — the web UI in a browser tab

The web UI rendered in a browser tab on a laptop. The bearer token
overlay or the unauthenticated landing page is fine. The JSON
snapshot at the top of the page (visible in the browser's network
panel) is the same data the TUI shows in the live header.

Caption suggestion: *"axum bridge · same data, different surface."*

## How to add a photo

1. Take the photo. If it's a phone photo, crop to the screen so the
   uconsole device frame isn't dominating the frame.
2. Save it into [`docs/photos/`](../photos/) as `photo-NN.jpg`
   (or `.png`).
3. Open a PR titled `photos: add photo-NN (description)`.
4. The README and this page already reference these slots, so no other
   file changes are needed.

## Where these photos are referenced

- [`../README.md`](../README.md) — top-level "Screenshots" section.
- This page — full numbered index with captions.
- The relevant wiki pages:
  - [Architecture](./Architecture.md) — `photo-04.jpg`
    (web UI in browser).
  - [Phase 3 — WM](./Phase-3-WM.md) — `photo-02.jpg`
    (split tree).
  - [Phase 5 — Modals](./Phase-5-Modals.md) — `photo-03.jpg`
    (modal in flight).
  - [Phase 1 — TUI](./Phase-1-TUI.md) — `photo-01.jpg`
    (launcher / sidebar).

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

By dropping a photo into [`docs/photos/`](../photos/) you agree to
license it under the same MIT terms as the rest of the project (or,
if you prefer, open a PR adding a `docs/photos/LICENSE-photos.md`
with your preferred license — most contributors just use MIT).
