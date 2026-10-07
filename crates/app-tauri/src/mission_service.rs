//! Mission service adapter: drives `core::mission::service::MissionService` on
//! the live MAVLink link and forwards its events to the React UI (issue #18).
//!
//! Events (`"mission"`): `{ op, kind, sent, total, seq, message? }` with
//! `kind = progress | completed | current_changed | failed`. On a completed
//! download the adapter also emits `"mission_plan"` with the downloaded items.
//!
//! The state machine, retransmission loop, and download assembly all live in
//! `core`; this module only maps events to the UI payload and emits them.

use maggcs_core::mavlink::connection::ConnectionHandle;
use maggcs_core::mission::protocol::{MissionEvent, MissionOperation};
use maggcs_core::mission::service::{MissionIds, MissionService, MissionServiceEvent};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Payload forwarded to the webview on every `"mission"` event.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct MissionEventPayload {
    pub op: &'static str,
    pub kind: &'static str,
    pub sent: u16,
    pub total: u16,
    pub seq: u16,
    pub message: Option<String>,
}

/// Serialize a core event into the UI payload.
fn payload(e: &MissionEvent) -> MissionEventPayload {
    match e {
        MissionEvent::Progress {
            operation,
            sent,
            total,
        } => MissionEventPayload {
            op: op_name(*operation),
            kind: "progress",
            sent: *sent,
            total: *total,
            seq: 0,
            message: None,
        },
        MissionEvent::Completed(op) => MissionEventPayload {
            op: op_name(*op),
            kind: "completed",
            sent: 0,
            total: 0,
            seq: 0,
            message: None,
        },
        MissionEvent::CurrentChanged { seq } => MissionEventPayload {
            op: "set_current",
            kind: "current_changed",
            sent: 0,
            total: 0,
            seq: *seq,
            message: None,
        },
        MissionEvent::Failed(err) => MissionEventPayload {
            op: "mission",
            kind: "failed",
            sent: 0,
            total: 0,
            seq: 0,
            message: Some(err.to_string()),
        },
    }
}

fn op_name(op: MissionOperation) -> &'static str {
    match op {
        MissionOperation::Upload => "upload",
        MissionOperation::Download => "download",
        MissionOperation::ClearAll => "clear",
        MissionOperation::SetCurrent(_) => "set_current",
    }
}

/// Start the mission service on the given link and forward its events to the UI.
pub fn spawn(app: AppHandle, handle: ConnectionHandle, ids: MissionIds) -> MissionService {
    let (service, mut events_rx) = MissionService::spawn(handle, ids);
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events_rx.recv().await {
            match event {
                MissionServiceEvent::Protocol(e) => {
                    let _ = app.emit("mission", payload(&e));
                }
                MissionServiceEvent::PlanDownloaded(items) => {
                    let _ = app.emit("mission_plan", items);
                }
            }
        }
    });
    service
}
