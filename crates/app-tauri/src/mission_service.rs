//! Mission service: wires `core::mission::MissionProtocol` to the live MAVLink
//! link and forwards mission events to the React UI.
//!
//! Events (`"mission"`): `{ op, kind, sent, total, seq, message? }`
//! with `kind = progress | completed | current_changed | failed`. On a
//! completed download the service also emits `"mission_plan"` with the
//! downloaded items.

use std::collections::HashMap;

use maggcs_core::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use maggcs_core::mavlink::MavMessage;
use maggcs_core::mission::protocol::{mission_item_from_mav, MissionEvent, MissionOperation, MissionProtocol};
use maggcs_core::mission::types::MissionItem;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

/// Commands accepted by the mission service task.
pub enum MissionCommand {
    Upload(Vec<MissionItem>),
    Download,
    Clear,
    SetCurrent(u16),
}

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

/// Serialize an event into the UI payload.
fn payload(e: &MissionEvent) -> MissionEventPayload {
    match e {
        MissionEvent::Progress { operation, sent, total } => MissionEventPayload {
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

/// Handle to the running mission service task.
#[derive(Clone)]
pub struct MissionService {
    cmd_tx: mpsc::Sender<MissionCommand>,
}

impl MissionService {
    /// Start the mission service on the given link.
    pub fn spawn(
        app: AppHandle,
        handle: ConnectionHandle,
        self_sys: u8,
        self_comp: u8,
        target_sys: u8,
        target_comp: u8,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        tauri::async_runtime::spawn(run(
            app, handle, cmd_rx, self_sys, self_comp, target_sys, target_comp,
        ));
        Self { cmd_tx }
    }

    /// Start uploading the given mission.
    pub async fn upload(&self, items: Vec<MissionItem>) -> Result<(), String> {
        self.cmd_tx
            .send(MissionCommand::Upload(items))
            .await
            .map_err(|_| "mission service stopped".to_string())
    }

    /// Start downloading the FC mission.
    pub async fn download(&self) -> Result<(), String> {
        self.cmd_tx
            .send(MissionCommand::Download)
            .await
            .map_err(|_| "mission service stopped".to_string())
    }

    /// Clear the FC mission.
    pub async fn clear(&self) -> Result<(), String> {
        self.cmd_tx
            .send(MissionCommand::Clear)
            .await
            .map_err(|_| "mission service stopped".to_string())
    }

    /// Set the active waypoint.
    pub async fn set_current(&self, seq: u16) -> Result<(), String> {
        self.cmd_tx
            .send(MissionCommand::SetCurrent(seq))
            .await
            .map_err(|_| "mission service stopped".to_string())
    }
}

async fn run(
    app: AppHandle,
    handle: ConnectionHandle,
    mut cmd_rx: mpsc::Receiver<MissionCommand>,
    self_sys: u8,
    self_comp: u8,
    target_sys: u8,
    target_comp: u8,
) {
    let mut proto = MissionProtocol::new(self_sys, self_comp, target_sys, target_comp);
    let mut events_rx = handle.subscribe();
    let mut downloading: Option<HashMap<u16, MissionItem>> = None;

    loop {
        let (events, frames) = tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return; };
                match cmd {
                    MissionCommand::Upload(items) => match proto.begin_upload(items) {
                        Ok(frames) => (Vec::new(), frames),
                        Err(e) => (vec![MissionEvent::Failed(e)], Vec::new()),
                    },
                    MissionCommand::Download => {
                        downloading = Some(HashMap::new());
                        (Vec::new(), proto.begin_download())
                    }
                    MissionCommand::Clear => (Vec::new(), proto.begin_clear()),
                    MissionCommand::SetCurrent(seq) => (Vec::new(), proto.begin_set_current(seq)),
                }
            }
            ev = events_rx.recv() => {
                match ev {
                    Ok(ConnectionEvent::Message(m)) => {
                        if let MavMessage::MISSION_ITEM_INT(i) = &m.message {
                            if let Some(map) = &mut downloading {
                                map.insert(i.seq, mission_item_from_mav(i));
                            }
                        }
                        proto.handle(&m.header, &m.message)
                    }
                    _ => (Vec::new(), Vec::new()),
                }
            }
        };

        for e in &events {
            let _ = app.emit("mission", payload(e));
            if let MissionEvent::Completed(MissionOperation::Download) = e {
                if let Some(map) = downloading.take() {
                    let mut items: Vec<MissionItem> = map.into_values().collect();
                    items.sort_by_key(|i| i.seq);
                    let _ = app.emit("mission_plan", items);
                }
            }
        }
        for f in frames {
            let _ = handle.send(f).await;
        }
    }
}
