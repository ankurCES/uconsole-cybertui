# cyberdeck — wiki

Welcome to the cyberdeck wiki. This directory mirrors the GitHub wiki
structure so the docs are browsable on disk and from the GitHub wiki
after sync.

## Index

| Page | What it covers |
| --- | --- |
| [Home](./Home.md) | Index + "where to start" |
| [Architecture](./Architecture.md) | Crate map, action flow, single-source-of-truth model |
| [Phase 1 — TUI](./Phase-1-TUI.md) | Screens, sidebar, command palette, toast log |
| [Phase 2 — PTY / ANSI](./Phase-2-PTY-ANSI.md) | `wm/pty.rs`, `wm/ansi.rs`, `wm/broadcaster.rs` |
| [Phase 3 — Window manager](./Phase-3-WM.md) | Split tree, focus, jumps, terminal panes |
| [Phase 4 — Polish](./Phase-4-Polish.md) | Pane-number badges, scrollback, persistence |
| [Phase 5 — Modal upgrade](./Phase-5-Modals.md) | Secret / Choice / Wizard / Progress / AuthFailure |
| [Hardening](./Hardening.md) | No-hang PTY tests (Patterns A + B) |
| [Keymaps](./Keymaps.md) | Global, WM, per-screen, uconsole-specific |
| [Hardware / Setup](./Hardware-Setup.md) | ClockworkPi uconsole on Debian 13 trixie |
| [Photos](./Photos.md) | Numbered photo index (drop into `docs/photos/`) |
| [Roadmap](./Roadmap.md) | Phases, in-progress, follow-ups |

## Where to start

- **I just want to install it.** → [README § Install](../README.md#install-one-liner)
- **I want the architecture in one diagram.** → [Architecture](./Architecture.md)
- **I want to see screenshots / photos.** → [Photos](./Photos.md)
- **I'm contributing a new screen.** → [Phase 1 — TUI](./Phase-1-TUI.md) + [Architecture](./Architecture.md)
- **I'm fixing a flaky PTY test.** → [Hardening](./Hardening.md)
- **I'm trying to get it running on a non-uconsole box.** → [Hardware / Setup](./Hardware-Setup.md)

## Conventions

- Wiki pages are Markdown (GitHub-flavoured).
- Code blocks default to `rust`, `sh`, `toml`, `json` — pick the right one.
- Cross-links between wiki pages use relative paths (`./Phase-3-WM.md`).
- Cross-links to the repo use `../README.md` / `../crates/...` paths.
- All wiki content lives in `docs/wiki/` and is shipped with the repo.
