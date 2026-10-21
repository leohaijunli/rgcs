//! A11 phase 1 bridge: the `shared_state_*` commands over
//! `core::shared_state`, plus the `shared_state_changed` broadcast every
//! window listens to. The conflict rule is core's: a write carries the
//! version the caller read, and a stale write is rejected with the current
//! version so the caller re-reads instead of clobbering.

use std::collections::BTreeMap;

use serde_json::Value;
use tauri::Emitter;

use crate::state::AppState;

/// Read one key (the caller projects it; there is no local truth).
///
/// # Errors
///
/// A stringified error when the key was never written.
#[tauri::command]
pub fn shared_state_get(
    state: tauri::State<'_, AppState>,
    key: String,
) -> Result<maggcs_core::shared_state::SharedEntry, String> {
    state.shared.get(&key).map_err(|e| e.to_string())
}

/// Every key — a fresh or late-joining window hydrates from this.
#[tauri::command]
pub fn shared_state_snapshot(
    state: tauri::State<'_, AppState>,
) -> BTreeMap<String, maggcs_core::shared_state::SharedEntry> {
    state.shared.snapshot()
}

/// Write a key. `expectedVersion: null` bootstraps (always lands);
/// otherwise it must match the version the caller read. On success the new
/// entry is broadcast as `shared_state_changed` to every window (including
/// the writer — the writer projects the event too, so it never keeps truth).
///
/// # Errors
///
/// A stringified [`maggcs_core::shared_state::SharedStateError`] — notably
/// `Conflict { expected, current }` for a stale write.
#[tauri::command]
pub fn shared_state_set(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    key: String,
    value: Value,
    expected_version: Option<u64>,
    writer: String,
) -> Result<maggcs_core::shared_state::SharedUpdate, String> {
    let update = state
        .shared
        .set(&key, value, expected_version, &writer)
        .map_err(|e| e.to_string())?;
    let _ = app.emit("shared_state_changed", update.clone());
    Ok(update)
}
