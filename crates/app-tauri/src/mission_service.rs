//! Target/operation-bound observation adapter for the existing Mission worker.

use maggcs_core::mavlink::connection::ConnectionHandle;
use maggcs_core::mission::operation_types::{MissionCurrentSnapshot, MissionProgressEnvelope};
use maggcs_core::mission::service::{MissionIds, MissionService, MissionServiceEvent};
use maggcs_core::runtime::operations::OperationPhase;
use maggcs_core::runtime::LinkId;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::runtime_facade::{RuntimeFacade, TargetRuntime};
use crate::state::AppState;

pub(crate) fn forward_event<R: Runtime>(
    app: &AppHandle<R>,
    link: LinkId,
    event: MissionServiceEvent,
) {
    let runtime = RuntimeFacade::new(app);
    match event {
        MissionServiceEvent::OwnedProgress {
            operation_id,
            generation,
            sent,
            total,
        } => {
            let state = app.state::<AppState>();
            let Some(operation) = state.missions.operation(&operation_id) else {
                return;
            };
            if operation.target.link_id != link
                || operation.target.generation != generation
                || runtime.bind(&operation.target).is_err()
            {
                return;
            }
            let Ok(status) = crate::mission_operations::retained_status(app, &operation) else {
                return;
            };
            if status.phase != OperationPhase::Running {
                return;
            }
            let _ = app.emit(
                "mission_progress",
                MissionProgressEnvelope {
                    operation,
                    request_id: status.request_id,
                    sent,
                    total,
                },
            );
        }
        MissionServiceEvent::CurrentObserved {
            seq,
            generation,
            received_at,
        } => {
            let Ok(snapshot) = runtime.snapshot(link) else {
                return;
            };
            let Some(target) = snapshot.target else {
                return;
            };
            if target.generation != generation || runtime.bind(&target).is_err() {
                return;
            }
            let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
                return;
            };
            let now_ms = now.as_millis().min(u64::MAX as u128) as u64;
            let age_ms = received_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
            let _ = app.emit(
                "mission_current",
                MissionCurrentSnapshot {
                    target,
                    seq,
                    received_at_ms: now_ms.saturating_sub(age_ms),
                },
            );
        }
        // Owned invocation replies carry terminal verdicts and downloaded items.
        MissionServiceEvent::Protocol(_) | MissionServiceEvent::PlanDownloaded(_) => {}
    }
}

pub(crate) fn spawn<R: Runtime>(
    app: AppHandle<R>,
    handle: ConnectionHandle,
    ids: MissionIds,
    link: LinkId,
) -> MissionService {
    let (service, mut events) = MissionService::spawn(handle, ids);
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            forward_event(&app, link, event);
        }
    });
    service
}
