//! Persistent tag overlay: MAC → {label, icon, color}.
//!
//! The radar shows known devices as named icons and unknown ones as generic
//! dots. The mapping from MAC to "who is this" lives in a JSON file
//! (`data/tags.json` by default) so the user can edit it without rebuilding.
//!
//! Format (matches the example in the plan):
//!
//! ```json
//! {
//!   "aa:bb:cc:dd:ee:ff": {
//!     "label": "Ankur's phone",
//!     "icon": "person",
//!     "color": "#7fdcff"
//!   }
//! }
//! ```
//!
//! MACs are stored as the user typed them; lookups normalise to lowercase
//! so a tag for `AA:BB:CC:DD:EE:FF` matches a device event for
//! `aa:bb:cc:dd:ee:ff`.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// The full set of icons the UI knows how to render. Anything else falls
/// back to `Generic` so a future schema bump doesn't crash old builds.
pub const KNOWN_ICONS: &[&str] = &[
    "person", "phone", "laptop", "tablet", "speaker", "tv", "watch", "generic",
];

/// One tag row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tag {
    pub label: String,
    pub icon: String,
    /// CSS hex color (`#rrggbb`).
    pub color: String,
}

/// The on-disk shape of `data/tags.json`: a flat MAC → Tag map.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TagFile {
    #[serde(flatten)]
    pub tags: HashMap<String, Tag>,
}

/// Process-wide tag store. Loads on construction, saves on every write.
///
/// Behind an `RwLock` so axum handlers can read it cheaply. The write path
/// is "rare" (user edits a tag) so a write lock is fine.
#[derive(Debug)]
pub struct TagDb {
    path: PathBuf,
    inner: RwLock<TagFile>,
}

impl TagDb {
    /// Load from disk. If the file doesn't exist yet, start empty (and the
    /// first save will create it).
    pub fn load(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let file = match fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str::<TagFile>(&s).unwrap_or_default(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => TagFile::default(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path,
            inner: RwLock::new(file),
        })
    }

    /// File this DB was loaded from / will save to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Cheap snapshot for `/api/tags`.
    pub fn snapshot(&self) -> TagFile {
        self.inner.read().expect("tag db poisoned").clone()
    }

    /// Insert or update. Persists to disk. Returns the previous value if
    /// there was one.
    pub fn upsert(&self, mac: &str, tag: Tag) -> io::Result<Option<Tag>> {
        let key = normalise_mac(mac);
        let prev;
        {
            let mut guard = self.inner.write().expect("tag db poisoned");
            prev = guard.tags.insert(key, tag);
            save(&self.path, &guard)?;
        }
        Ok(prev)
    }

    /// Delete by MAC. Persists to disk. Returns whether anything was removed.
    pub fn delete(&self, mac: &str) -> io::Result<bool> {
        let key = normalise_mac(mac);
        let removed;
        {
            let mut mut_guard = self.inner.write().expect("tag db poisoned");
            removed = mut_guard.tags.remove(&key).is_some();
            if removed {
                save(&self.path, &mut_guard)?;
            }
        }
        Ok(removed)
    }

    /// Look up a single MAC. Returns the tag if any (case-insensitive).
    pub fn get(&self, mac: &str) -> Option<Tag> {
        let key = normalise_mac(mac);
        self.inner.read().expect("tag db poisoned").tags.get(&key).cloned()
    }

    /// Apply the overlay to a list of MACs and return the resolved set.
    /// Used by `/api/devices` so the browser doesn't have to.
    pub fn overlay<'a, I: IntoIterator<Item = &'a str>>(
        &self,
        macs: I,
    ) -> HashMap<String, Tag> {
        let guard = self.inner.read().expect("tag db poisoned");
        macs.into_iter()
            .filter_map(|m| {
                let k = normalise_mac(m);
                guard.tags.get(&k).cloned().map(|t| (m.to_string(), t))
            })
            .collect()
    }
}

/// Write the tag file atomically: write to `tags.json.tmp` then rename.
fn save(path: &Path, file: &TagFile) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let s = serde_json::to_string_pretty(file)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&tmp, s)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Lowercase + trim, so case-insensitive lookups work.
fn normalise_mac(mac: &str) -> String {
    mac.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wifi-radar-test-{}-{}",
            std::process::id(),
            name
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.join("tags.json")
    }

    #[test]
    fn load_missing_file_yields_empty_db() {
        let p = temp_path("missing");
        let _ = fs::remove_file(&p);
        let db = TagDb::load(&p).unwrap();
        assert!(db.snapshot().tags.is_empty());
    }

    #[test]
    fn upsert_persists_and_overwrites() {
        let p = temp_path("upsert");
        let _ = fs::remove_file(&p);
        let db = TagDb::load(&p).unwrap();
        db.upsert(
            "AA:BB:CC:DD:EE:FF",
            Tag {
                label: "Ankur's phone".into(),
                icon: "phone".into(),
                color: "#7fdcff".into(),
            },
        )
        .unwrap();
        assert_eq!(db.get("aa:bb:cc:dd:ee:ff").unwrap().label, "Ankur's phone");

        // Reload from disk to confirm persistence.
        let db2 = TagDb::load(&p).unwrap();
        assert_eq!(
            db2.get("AA:BB:CC:DD:EE:FF").unwrap().label,
            "Ankur's phone"
        );
    }

    #[test]
    fn upsert_returns_previous_value() {
        let p = temp_path("prev");
        let _ = fs::remove_file(&p);
        let db = TagDb::load(&p).unwrap();
        let first = Tag {
            label: "first".into(),
            icon: "person".into(),
            color: "#fff".into(),
        };
        let second = Tag {
            label: "second".into(),
            icon: "person".into(),
            color: "#fff".into(),
        };
        assert!(db.upsert("aa:aa:aa:aa:aa:aa", first.clone()).unwrap().is_none());
        let prev = db.upsert("aa:aa:aa:aa:aa:aa", second.clone()).unwrap();
        assert_eq!(prev, Some(first));
        assert_eq!(db.get("aa:aa:aa:aa:aa:aa").unwrap().label, "second");
    }

    #[test]
    fn delete_removes_tag_and_persists() {
        let p = temp_path("delete");
        let _ = fs::remove_file(&p);
        let db = TagDb::load(&p).unwrap();
        db.upsert(
            "aa:bb:cc:dd:ee:01",
            Tag {
                label: "x".into(),
                icon: "person".into(),
                color: "#fff".into(),
            },
        )
        .unwrap();
        assert!(db.delete("aa:bb:cc:dd:ee:01").unwrap());
        assert!(db.get("aa:bb:cc:dd:ee:01").is_none());

        let db2 = TagDb::load(&p).unwrap();
        assert!(db2.get("aa:bb:cc:dd:ee:01").is_none());
    }

    #[test]
    fn delete_returns_false_when_absent() {
        let p = temp_path("delete-miss");
        let _ = fs::remove_file(&p);
        let db = TagDb::load(&p).unwrap();
        assert!(!db.delete("aa:bb:cc:dd:ee:ff").unwrap());
    }

    #[test]
    fn overlay_returns_only_macs_that_have_tags() {
        let p = temp_path("overlay");
        let _ = fs::remove_file(&p);
        let db = TagDb::load(&p).unwrap();
        db.upsert(
            "aa:bb:cc:dd:ee:01",
            Tag {
                label: "known".into(),
                icon: "phone".into(),
                color: "#fff".into(),
            },
        )
        .unwrap();
        let macs = vec![
            "aa:bb:cc:dd:ee:01",
            "aa:bb:cc:dd:ee:02",
            "AA:BB:CC:DD:EE:01", // same as #1, different case
        ];
        let out = db.overlay(macs);
        assert_eq!(out.len(), 2);
        assert_eq!(out["aa:bb:cc:dd:ee:01"].label, "known");
        assert_eq!(out["AA:BB:CC:DD:EE:01"].label, "known");
    }

    #[test]
    fn corrupt_file_yields_empty_db_without_panicking() {
        let p = temp_path("corrupt");
        fs::write(&p, "this is not json").unwrap();
        let db = TagDb::load(&p).unwrap();
        assert!(db.snapshot().tags.is_empty());
    }

    #[test]
    fn normalise_mac_lowercases_and_trims() {
        assert_eq!(normalise_mac("  AA:BB:CC:DD:EE:FF  "), "aa:bb:cc:dd:ee:ff");
        assert_eq!(normalise_mac("aa:bb:cc:dd:ee:ff"), "aa:bb:cc:dd:ee:ff");
    }
}