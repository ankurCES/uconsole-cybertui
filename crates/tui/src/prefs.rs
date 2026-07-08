//! Persistent user preferences — theme, units, last-known city, toggles.
//!
//! Stored as a single JSON file under `dirs::config_dir() / cyberdeck /
//! prefs.json`. Written atomically (tmp file + rename) so a crash mid-save
//! never produces a half-written prefs file. Load is tolerant of missing
//! files (returns `Default::default()`) and corrupt files (logs a warning,
//! returns defaults).
//!
//! The shape of `Prefs` is the contract: any field rename is a breaking
//! change for existing user files. New fields should be `Option<T>` or
//! have a sensible `Default` so older files still load.
//!
//! All mutation sites in the TUI that flip a persisted field call
//! `Prefs::save(&current)` (typically via `App::save_prefs`) which
//! delegates to `Prefs::save_to` under the hood. Saves are best-effort —
//! a failure logs a warning toast rather than crashing the renderer.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::keymap::Keymap;
use crate::theme::ThemeName;

/// Imperial vs Metric for weather display. Stored as a kebab-case string
/// in the prefs file so future formats (e.g. `"scientific"`) can be added
/// without a versioned migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Units {
    Metric,
    Imperial,
}

impl Default for Units {
    fn default() -> Self {
        Units::Metric
    }
}

/// All persisted user preferences. `Default::default()` must always
/// produce a fully-usable prefs value so a fresh install (or a deleted
/// file) renders sensibly without further user input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prefs {
    /// Active theme. Cycles via Settings → Theme. Persisted as the
    /// kebab-case string from `ThemeName::as_str` so renames inside the
    /// Rust enum don't corrupt older files (we fall back to `Dark` on
    /// unknown strings, see `ThemeName::from_str`).
    #[serde(default)]
    pub theme: ThemeName,

    /// Whether mouse capture is enabled at startup.
    #[serde(default)]
    pub mouse: bool,

    /// Whether Nerd Font glyphs are preferred over ASCII fallbacks.
    #[serde(default = "default_true")]
    pub nerd_font: bool,

    /// Whether to start the embedded web server on launch.
    #[serde(default)]
    pub web_server_on_start: bool,

    /// Bind address for the web server. `None` ⇒ runtime default
    /// (`0.0.0.0:7878`). Stored as a string so users can hand-edit
    /// `prefs.json` to override without recompiling.
    #[serde(default)]
    pub web_bind: Option<String>,

    /// Last-known city name (manual override of IP geolocation). `None`
    /// ⇒ use IP-based lookup on next launch. Stored as the user typed
    /// it so casing/spacing are preserved.
    #[serde(default)]
    pub city: Option<String>,

    /// Imperial vs Metric for weather (the City screen is the only
    /// consumer right now, but other screens may pick this up later).
    #[serde(default)]
    pub units: Units,

    /// Whether to render the synthetic traffic overlay on the City map.
    /// Documented in the City footer as "synthetic" until real keys land.
    #[serde(default = "default_true")]
    pub traffic_overlay: bool,

    /// Whether the right-hand weather panel is visible on the City
    /// screen. Toggled with `w`.
    #[serde(default = "default_true")]
    pub show_weather_panel: bool,

    /// User-editable key remapping (Settings → Keys). `Keymap::default()`
    /// is empty (= identity: every action uses its built-in binding).
    /// Older prefs files without this field load as empty.
    #[serde(default)]
    pub keymap: Keymap,
}

fn default_true() -> bool {
    true
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: ThemeName::Dark,
            mouse: false,
            nerd_font: true,
            web_server_on_start: false,
            web_bind: None,
            city: None,
            units: Units::Metric,
            traffic_overlay: true,
            show_weather_panel: true,
            keymap: Keymap::default(),
        }
    }
}

impl Prefs {
    /// Path to the prefs file under the user's config dir. Uses
    /// `dirs::config_dir()` which respects `XDG_CONFIG_HOME` on Linux
    /// and the platform-native location elsewhere. The `cyberdeck/`
    /// subdirectory is created lazily by `save_to`.
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("cyberdeck").join("prefs.json"))
    }

    /// Load prefs from `path`. Missing file → `Default::default()`.
    /// Corrupt file → `Default::default()` + `tracing::warn!`. Never
    /// returns `Err` — prefs are best-effort and a corrupt prefs file
    /// must not block the TUI from launching.
    pub fn load_from(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(s) => match serde_json::from_str::<Prefs>(&s) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "prefs file is corrupt — falling back to defaults",
                    );
                    Self::default()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "could not read prefs file — falling back to defaults",
                );
                Self::default()
            }
        }
    }

    /// Convenience: load from `Prefs::path()`. Returns defaults if
    /// `dirs::config_dir()` is unavailable (unusual, but possible on
    /// stripped-down containers).
    pub fn load() -> Self {
        match Self::path() {
            Some(p) => Self::load_from(&p),
            None => Self::default(),
        }
    }

    /// Save to `path`. Writes to `<path>.tmp` first, then renames
    /// atomically. Creates parent directories as needed. Returns the
    /// underlying I/O error so callers can toast/log it.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&tmp, json)?;
        // Atomic on POSIX, best-effort on Windows. The cyberdeck-tui
        // target is Linux/macOS so this is fine.
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Convenience: save to `Prefs::path()`. Logs a warning on failure
    /// (best-effort — prefs loss shouldn't crash the renderer).
    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Err(e) = self.save_to(&p) {
                tracing::warn!(
                    path = %p.display(),
                    error = %e,
                    "could not persist prefs",
                );
            }
        }
    }

    /// Toggle units and persist. Called from the City screen's `u`
    /// handler so the user's unit preference survives a restart.
    /// The caller updates `app.units` directly; this writes the new
    /// value to disk.
    pub fn save_units(units: Units) {
        let mut p = Self::load();
        p.units = units;
        p.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeName, ALL_THEME_NAMES};
    use tempfile::tempdir;

    #[test]
    fn round_trip_preserves_all_fields() {
        use crate::keymap::Keymap;
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("prefs.json");

        let original = Prefs {
            theme: ThemeName::Cyberpunk,
            mouse: true,
            nerd_font: false,
            web_server_on_start: true,
            web_bind: Some("127.0.0.1:9000".to_string()),
            city: Some("Tokyo".to_string()),
            units: Units::Imperial,
            traffic_overlay: false,
            show_weather_panel: false,
            keymap: Keymap::default(),
        };
        original.save_to(&path).expect("save");
        let loaded = Prefs::load_from(&path);
        assert_eq!(loaded.theme, original.theme);
        assert_eq!(loaded.mouse, original.mouse);
        assert_eq!(loaded.nerd_font, original.nerd_font);
        assert_eq!(loaded.web_server_on_start, original.web_server_on_start);
        assert_eq!(loaded.web_bind, original.web_bind);
        assert_eq!(loaded.city, original.city);
        assert_eq!(loaded.units, original.units);
        assert_eq!(loaded.traffic_overlay, original.traffic_overlay);
        assert_eq!(loaded.show_weather_panel, original.show_weather_panel);
        assert_eq!(loaded.keymap, original.keymap);
    }

    #[test]
    fn round_trip_preserves_keymap_bindings() {
        use crate::keymap::{Keymap, NavAction};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("prefs.json");
        let mut km = Keymap::default();
        km.bind(NavAction::Down, KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        km.bind(NavAction::Up,   KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        let original = Prefs {
            theme: ThemeName::Dark,
            mouse: false,
            nerd_font: true,
            web_server_on_start: false,
            web_bind: None,
            city: None,
            units: Units::Metric,
            traffic_overlay: true,
            show_weather_panel: true,
            keymap: km,
        };
        original.save_to(&path).expect("save");
        let loaded = Prefs::load_from(&path);
        assert_eq!(loaded.keymap.get(NavAction::Down),
                   Some(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)));
        assert_eq!(loaded.keymap.get(NavAction::Up),
                   Some(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)));
    }

    #[test]
    fn partial_file_fills_default_keymap() {
        // An older prefs file (pre-keymap) has no `keymap` field. It must
        // load as an empty Keymap — not fail the whole parse.
        use crate::keymap::Keymap;
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("prefs.json");
        fs::write(&path, r#"{ "theme": "dark" }"#).unwrap();
        let loaded = Prefs::load_from(&path);
        assert!(loaded.keymap.is_empty(), "missing keymap field should default to empty");
    }

    #[test]
    fn missing_file_returns_defaults() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("does-not-exist.json");
        let loaded = Prefs::load_from(&path);
        let defaults = Prefs::default();
        assert_eq!(loaded.theme, defaults.theme);
        assert_eq!(loaded.mouse, defaults.mouse);
        assert!(loaded.nerd_font, "default nerd_font is true");
    }

    #[test]
    fn corrupt_file_returns_defaults() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("prefs.json");
        fs::write(&path, "{ this is not json").unwrap();
        let loaded = Prefs::load_from(&path);
        // Should not panic, should return defaults.
        assert_eq!(loaded.theme, ThemeName::Dark);
    }

    #[test]
    fn partial_file_fills_defaults_for_missing_fields() {
        // Older prefs files (pre-City) won't have `traffic_overlay` etc.
        // The #[serde(default)] attributes must make those load as
        // their default values rather than failing the whole parse.
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("prefs.json");
        fs::write(
            &path,
            r#"{ "theme": "cyberpunk", "mouse": true, "nerd_font": false }"#,
        )
        .unwrap();
        let loaded = Prefs::load_from(&path);
        assert_eq!(loaded.theme, ThemeName::Cyberpunk);
        assert!(loaded.mouse);
        assert!(!loaded.nerd_font);
        // Missing fields fall back to Prefs::default().
        assert!(
            loaded.traffic_overlay,
            "missing field should default to true"
        );
        assert!(loaded.show_weather_panel);
        assert_eq!(loaded.units, Units::Metric);
    }

    #[test]
    fn unknown_theme_string_falls_back_to_dark() {
        // Forward-compat: a future build adds a new theme that this
        // build doesn't know about. The string won't parse as a known
        // ThemeName so we should silently use Dark rather than 500-ing.
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("prefs.json");
        fs::write(
            &path,
            r#"{ "theme": "plasma-storm-from-the-future" }"#,
        )
        .unwrap();
        let loaded = Prefs::load_from(&path);
        assert_eq!(loaded.theme, ThemeName::Dark);
    }

    #[test]
    fn theme_cycle_wraps_around() {
        // Sanity: cycling through all themes visits each one exactly
        // once before returning to the start.
        let mut t = ThemeName::Dark;
        let mut seen = vec![t];
        for _ in 0..ALL_THEME_NAMES.len() {
            t = t.next();
            seen.push(t);
        }
        // After N+1 steps we must be back at Dark.
        assert_eq!(seen[0], seen[ALL_THEME_NAMES.len()]);
        // Every intermediate value must be unique.
        let mut sorted = seen[..ALL_THEME_NAMES.len()].to_vec();
        sorted.sort_by_key(|n| n.as_str());
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            ALL_THEME_NAMES.len(),
            "cycle must visit every theme exactly once"
        );
    }

    #[test]
    fn theme_str_round_trips() {
        for name in ALL_THEME_NAMES {
            assert_eq!(ThemeName::from_str(name.as_str()), *name);
        }
    }
}