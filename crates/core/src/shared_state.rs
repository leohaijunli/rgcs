//! Cross-window single source of truth (plan §3 **A11**, phase 1): the
//! "map-and-planning shared state" — layer toggles, selection, camera —
//! lives here in core, keyed and **versioned**, so two webviews project it
//! instead of owning it. Every write carries the previous version the writer
//! saw; a stale write is rejected and the authoritative state re-broadcast.
//!
//! Phase 1 ships the store, the Tauri commands and the change event. The
//! frontend migrates one slice (map layer toggles) onto it; the remaining
//! slices (selection, camera, drape result) move the same way — `set` with
//! the version you read, project the event, never keep local truth.
//!
//! Not in phase 1 (kept open by design): persistence across restarts (the
//! store is process state; a restart re-hydrates from each window's own
//! boot), and the mission plan itself (that flow is already command-driven
//! with its own hash gate).

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One shared key with its content and the write counter that serialises
/// writers (plan A11: "所有写操作带版本号,冲突时以 core 为准").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedEntry {
    /// Bumped on every accepted write; 1 after the first.
    pub version: u64,
    /// The value — an arbitrary JSON payload (the callers own the shape).
    pub value: Value,
    /// Which window wrote it last (diagnostics; `main` bootstraps).
    pub written_by: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SharedStateError {
    #[error("version conflict for `{key}`: expected {expected}, current is {current}")]
    Conflict {
        key: String,
        expected: u64,
        current: u64,
    },
    #[error("unknown key `{0}`")]
    Unknown(String),
}

/// The store: `key → (version, value, writer)`. Small by design — a few
/// dozen keys of UI state, cloned on read.
#[derive(Default)]
pub struct SharedState {
    entries: Mutex<BTreeMap<String, SharedEntry>>,
}

/// One changed key, broadcast to every window after an accepted write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedUpdate {
    pub key: String,
    pub entry: SharedEntry,
}

impl SharedState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read one key.
    ///
    /// # Errors
    ///
    /// [`SharedStateError::Unknown`] when nothing was ever written under it.
    pub fn get(&self, key: &str) -> Result<SharedEntry, SharedStateError> {
        self.entries
            .lock()
            .expect("shared state lock")
            .get(key)
            .cloned()
            .ok_or_else(|| SharedStateError::Unknown(key.to_string()))
    }

    /// Read everything (a fresh/late-joining window hydrates from this).
    #[must_use]
    pub fn snapshot(&self) -> BTreeMap<String, SharedEntry> {
        self.entries.lock().expect("shared state lock").clone()
    }

    /// Write a key. With `expected_version: None` the write always lands
    /// (the bootstrap); with `Some(v)` it must match the current version —
    /// a stale writer gets [`SharedStateError::Conflict`] and re-reads.
    ///
    /// # Errors
    ///
    /// [`SharedStateError::Conflict`] on a stale expected version.
    pub fn set(
        &self,
        key: &str,
        value: Value,
        expected_version: Option<u64>,
        writer: &str,
    ) -> Result<SharedUpdate, SharedStateError> {
        let mut entries = self.entries.lock().expect("shared state lock");
        let entry = entries.get_mut(key);
        match (entry, expected_version) {
            (None, None) | (None, Some(0)) => {
                let e = SharedEntry {
                    version: 1,
                    value,
                    written_by: writer.to_string(),
                };
                entries.insert(key.to_string(), e.clone());
                Ok(SharedUpdate {
                    key: key.to_string(),
                    entry: e,
                })
            }
            (Some(existing), expected) => {
                if let Some(v) = expected {
                    if v != existing.version {
                        return Err(SharedStateError::Conflict {
                            key: key.to_string(),
                            expected: v,
                            current: existing.version,
                        });
                    }
                }
                existing.version += 1;
                existing.value = value;
                existing.written_by = writer.to_string();
                Ok(SharedUpdate {
                    key: key.to_string(),
                    entry: existing.clone(),
                })
            }
            (None, Some(expected)) => Err(SharedStateError::Conflict {
                key: key.to_string(),
                expected,
                current: 0,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn first_write_versions_at_one_and_reads_back() {
        let st = SharedState::new();
        let up = st
            .set("layers.mesh", json!(true), None, "main")
            .expect("bootstrap write");
        assert_eq!(up.entry.version, 1);
        assert_eq!(st.get("layers.mesh").expect("read").value, json!(true));
    }

    #[test]
    fn every_accepted_write_bumps_the_version() {
        let st = SharedState::new();
        st.set("selected", json!(null), None, "main")
            .expect("first");
        let up = st
            .set("selected", json!(3), Some(1), "main")
            .expect("second");
        assert_eq!(up.entry.version, 2);
        assert_eq!(up.entry.value, json!(3));
    }

    /// A11's conflict rule: a stale writer is rejected with the current
    /// version, and the stored value keeps the winner's content.
    #[test]
    fn a_stale_write_conflicts_and_keeps_the_winner() {
        let st = SharedState::new();
        st.set("k", json!("a"), None, "w1").expect("v1");
        st.set("k", json!("b"), Some(1), "w2").expect("v2");
        let err = st
            .set("k", json!("stale"), Some(1), "w1")
            .expect_err("stale");
        assert!(
            matches!(
                err,
                SharedStateError::Conflict {
                    expected: 1,
                    current: 2,
                    ..
                }
            ),
            "got {err}"
        );
        assert_eq!(st.get("k").expect("read").value, json!("b"));
    }

    #[test]
    fn the_snapshot_hydrates_a_late_window() {
        let st = SharedState::new();
        st.set("a", json!(1), None, "main").expect("a");
        st.set("b", json!(2), None, "main").expect("b");
        let snap = st.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap["b"].value, json!(2));
    }

    #[test]
    fn an_unknown_key_is_an_error_not_a_default() {
        let st = SharedState::new();
        assert!(matches!(
            st.get("nope"),
            Err(SharedStateError::Unknown(k)) if k == "nope"
        ));
    }
}
