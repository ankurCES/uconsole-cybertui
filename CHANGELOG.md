# Changelog

All notable changes to this project will be documented in this file.
Format follows [Keep a Changelog](https://keepachangelog.com/).

## [0.4.0] — 2026-07-10

### Added
- City map: braille-rendered roads from OpenStreetMap via Overpass API for any city worldwide
- Colored POI layer: parks (green), forests (dark green), water (cyan), emergency services (red/blue/magenta)
- Live blinking GPS location marker derived from IP geolocation
- Weather ASCII art icons with day/night sun/moon indicators
- Dynamic city data fetch — no longer limited to bundled city slugs
- City data disk cache (`~/.cyberdeck/cities/`) with 24h TTL
- Local AI chat via llama-server sidecar with MiniCPM5-1B GGUF
- install.sh: automatic llama-server build from source (llama.cpp)
- install.sh: automatic MiniCPM5-1B GGUF model download

### Changed
- AI: `--jinja` flag for native chat template support (MiniCPM5)
- AI: 90s health timeout (was 30s), stderr ring buffer capture
- AI: `reasoning_content` SSE field for native thinking output
- README restructured — detail pages moved to `docs/`

### Fixed
- AI screen stuck on "loading" forever after model failure — now shows actual error
- install.sh: correct MiniCPM5 GGUF download URL (was 404)

## [0.3.0] — 2026-06-XX

### Added
- AI agent harness, top menu bar, AI logs screen
- CyberDeck Native theme
- OSM city map with braille rendering, traffic overlay, weather pane
- LoRa/Meshtastic node screen
- Screensaver mode
- TUI v2: clean UI/interaction layer rewrite
- Two-pane window manager with live PTY terminals
- Phase 5 modals (Secret/Choice/Wizard/Progress/AuthFailure)
- Wi-Fi radar: BLE RSSI distance + bearing
- City screen: click-to-pan, palette search, zoom
- CSI human sensing (breathing/heartbeat/presence) via nexmon

### Changed
- Flat D-pad keymap (a=Enter, b=Esc) for uconsole ergonomics
- User-configurable keymap (Settings screen)
- Ctrl+M global menu shortcut

## [0.2.0] — 2026-05-XX

### Added
- Cyberdeck console layout optimized for 80x24
- Tab/Shift-Tab WM pane sync
- City screen (14th sidebar screen) — IP geo, Open-Meteo, braille road map

## [0.1.0] — 2026-04-XX

### Added
- Initial release: 13-screen TUI + axum web bridge
- System, Network, Bluetooth, Power, Display, Audio, Storage, Services, Packages, Processes, Logs, Files, Settings screens
- Command palette, help modal, toast log
- Bearer-token auth for web API
- One-line curl installer with presets (--tui, --web, --full, --radar)
