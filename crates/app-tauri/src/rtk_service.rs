//! Independent base receiver and link-owned RTCM forwarder adapters.

use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Runtime, State};

use maggcs_core::config::serial::{AutoConnectIntentRef, AutoConnectRole, SerialSessionRef};
use maggcs_core::devices::{
    BaudSelection, DeviceDatabase, DeviceRole, LeaseRequest, SerialDeviceInfo, SerialOwner,
    SerialPortRef,
};
use maggcs_core::lifecycle::StopOutcome;
use maggcs_core::mavlink::connection::{ConnectionEvent, ConnectionHandle, LinkState};
use maggcs_core::mavlink::{router::MessageRoute, Endpoint};
use maggcs_core::rtk::base::{
    BaseMode, BasePhase, BasePort, BaseSerialRequest, BaseSessionId, BaseStatus,
    RtkBaseServiceHandle,
};
use maggcs_core::rtk::multi::{ForeignInjector, ForeignState, FOREIGN_WINDOW};
use maggcs_core::rtk::service::{RtkLinkStatus, RtkServiceHandle, RtkStats};

use crate::state::AppState;

#[derive(Default)]
struct LinkCache {
    owner: u64,
    active: bool,
    snapshot: RtkLinkStatus,
}

#[derive(Default)]
pub struct RtkState {
    pub base: RtkBaseServiceHandle,
    link: Mutex<LinkCache>,
    base_events: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl RtkState {
    pub fn ensure_base_events<R: Runtime>(self: &Arc<Self>, app: &AppHandle<R>) {
        let mut pump = self.base_events.lock();
        if pump.is_some() {
            return;
        }
        let state = Arc::downgrade(self);
        let mut status = self.base.subscribe();
        let app = app.clone();
        *pump = Some(tauri::async_runtime::spawn(async move {
            loop {
                let current = status.borrow_and_update().clone();
                if let Some(state) = state.upgrade() {
                    if current.phase != BasePhase::Ready {
                        state.clear_stats(&app);
                    }
                } else {
                    return;
                }
                if app.emit("rtk_base_status", &current).is_err() {
                    return;
                }
                if status.changed().await.is_err() {
                    return;
                }
            }
        }));
    }

    fn clear_stats<R: Runtime>(&self, app: &AppHandle<R>) {
        let mut cache = self.link.lock();
        if cache.snapshot.stats.take().is_some() {
            cache.snapshot.revision += 1;
            let _ = app.emit("rtk_link_status", &cache.snapshot);
        }
    }

    fn begin_link<R: Runtime>(&self, app: &AppHandle<R>) -> u64 {
        let mut cache = self.link.lock();
        cache.owner += 1;
        cache.active = true;
        cache.snapshot.stats = None;
        cache.snapshot.foreign = ForeignState::default();
        cache.snapshot.revision += 1;
        let _ = app.emit("rtk_link_status", &cache.snapshot);
        cache.owner
    }

    fn publish<R: Runtime>(
        &self,
        owner: u64,
        app: &AppHandle<R>,
        apply: impl FnOnce(&mut RtkLinkStatus),
    ) {
        let mut cache = self.link.lock();
        if cache.owner != owner || !cache.active {
            return;
        }
        apply(&mut cache.snapshot);
        cache.snapshot.revision += 1;
        // Serialise publication with ownership changes, including the final
        // empty snapshot. Delayed frontend delivery is rejected by revision.
        let _ = app.emit("rtk_link_status", &cache.snapshot);
    }

    fn retire<R: Runtime>(&self, owner: u64, app: &AppHandle<R>) {
        let mut cache = self.link.lock();
        if cache.owner != owner || !cache.active {
            return;
        }
        cache.active = false;
        cache.snapshot.stats = None;
        cache.snapshot.foreign = ForeignState::default();
        cache.snapshot.revision += 1;
        let _ = app.emit("rtk_link_status", &cache.snapshot);
    }
}

impl Drop for RtkState {
    fn drop(&mut self) {
        if let Some(pump) = self.base_events.get_mut().take() {
            pump.abort();
        }
    }
}

pub struct RtkRuntime {
    service: RtkServiceHandle,
    service_task: Option<tokio::task::JoinHandle<()>>,
    observers: Vec<tauri::async_runtime::JoinHandle<()>>,
    retire: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl RtkRuntime {
    pub fn spawn<R: Runtime>(
        state: Arc<RtkState>,
        app: AppHandle<R>,
        conn: ConnectionHandle,
    ) -> Self {
        use ::mavlink::MessageData;
        let owner = state.begin_link(&app);
        let (service, mut stats_rx, task) =
            maggcs_core::rtk::service::spawn_for_base(state.base.clone(), conn.clone());
        let stats_state = state.clone();
        let stats_app = app.clone();
        let stats_conn = conn.clone();
        let stats_task = tauri::async_runtime::spawn(async move {
            while let Some(stats) = stats_rx.recv().await {
                let base = stats_state.base.snapshot();
                if stats.source.is_some() {
                    if base.phase != BasePhase::Ready
                        || stats.base_session_id != base.session_id
                        || stats.connection_generation != stats_conn.generation()
                        || stats_conn.state().await != LinkState::Connected
                    {
                        continue;
                    }
                } else if base.phase == BasePhase::Ready
                    && stats_conn.state().await == LinkState::Connected
                {
                    // Empty snapshots from before this base/transport became ready.
                    continue;
                }
                stats_state.publish(owner, &stats_app, |snapshot| {
                    let current = stats_state.base.snapshot();
                    if stats.source.is_some() {
                        if current.phase == BasePhase::Ready
                            && current.session_id == stats.base_session_id
                            && stats_conn.generation() == stats.connection_generation
                        {
                            snapshot.stats = Some(stats.clone());
                        }
                    } else if current.phase != BasePhase::Ready {
                        snapshot.stats = None;
                    }
                });
            }
        });
        let foreign_state = state.clone();
        let foreign_app = app.clone();
        let mut foreign_events = conn.subscribe_route(MessageRoute::messages(&[
            ::mavlink::common::GPS_RTCM_DATA_DATA::ID,
        ]));
        let foreign_task = tauri::async_runtime::spawn(async move {
            let own_ids = (conn.config().system_id, conn.config().component_id);
            let mut injector = ForeignInjector::new();
            let mut last = ForeignState::default();
            let mut generation = conn.generation();
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    event = foreign_events.recv() => match event {
                        Ok(ConnectionEvent::Message(message)) if is_foreign_injector(
                            (message.header.system_id, message.header.component_id), own_ids,
                        ) => injector.observe(Instant::now()),
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                        _ => {},
                    },
                    _ = tick.tick() => {},
                }
                if generation != conn.generation() || conn.state().await != LinkState::Connected {
                    generation = conn.generation();
                    injector.reset();
                    foreign_state.publish(owner, &foreign_app, |snapshot| snapshot.stats = None);
                }
                let current = ForeignState {
                    active: injector.active(Instant::now(), FOREIGN_WINDOW),
                    packets: injector.packets(),
                };
                if current != last {
                    last = current;
                    foreign_state
                        .publish(owner, &foreign_app, |snapshot| snapshot.foreign = current);
                }
            }
        });
        Self {
            service,
            service_task: Some(task),
            observers: vec![stats_task, foreign_task],
            retire: Some(Box::new(move || state.retire(owner, &app))),
        }
    }

    pub async fn stop(mut self, deadline: Duration) -> StopOutcome {
        let started = Instant::now();
        if let Some(retire) = self.retire.take() {
            retire();
        }
        for observer in self.observers.drain(..) {
            observer.abort();
            let _ = observer.await;
        }
        let result = tokio::time::timeout(deadline.saturating_sub(started.elapsed()), async {
            self.service
                .shutdown()
                .await
                .map_err(|error| error.to_string())?;
            if let Some(task) = self.service_task.as_mut() {
                task.await.map_err(|error| error.to_string())?;
            }
            Ok::<(), String>(())
        })
        .await;
        match result {
            Ok(Ok(())) => {
                self.service_task.take();
                StopOutcome::Stopped {
                    elapsed_ms: elapsed_ms(started),
                }
            }
            Ok(Err(message)) => StopOutcome::Failed { message },
            Err(_) => StopOutcome::TimedOut {
                elapsed_ms: elapsed_ms(started),
            },
        }
    }
}

impl Drop for RtkRuntime {
    fn drop(&mut self) {
        if let Some(retire) = self.retire.take() {
            retire();
        }
        if let Some(task) = self.service_task.take() {
            task.abort();
        }
        for observer in self.observers.drain(..) {
            observer.abort();
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn is_foreign_injector(sender: (u8, u8), own: (u8, u8)) -> bool {
    sender != own
}

pub(crate) fn same_port(left: &Path, right: &Path) -> bool {
    let left = std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

/// Call while holding AppState::ops, before opening any FC serial transport.
pub(crate) fn ensure_port_not_base(state: &AppState, endpoint: &Endpoint) -> Result<(), String> {
    let Endpoint::Serial { port, .. } = endpoint else {
        return Ok(());
    };
    let base = state.rtk.base.snapshot();
    if base.phase.running()
        && base
            .serial
            .as_ref()
            .is_some_and(|s| same_port(port, Path::new(&s.port_name)))
    {
        return Err(
            "This serial port is owned by the RTK base receiver. Disconnect the base first.".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
fn classify_ports(devices: Vec<SerialDeviceInfo>, occupied: &[PathBuf]) -> Vec<BasePort> {
    let db = DeviceDatabase::builtin();
    devices
        .into_iter()
        .map(|device| {
            let suggested_role = db.suggested_role(&device);
            let unavailable_reason = if occupied
                .iter()
                .any(|port| same_port(port, Path::new(&device.port_name)))
            {
                Some("In use by a vehicle connection".into())
            } else if suggested_role == DeviceRole::FlightController {
                Some("Flight controller — not a base receiver".into())
            } else {
                None
            };
            BasePort {
                device,
                port_ref: None,
                suggested_role,
                unavailable_reason,
            }
        })
        .collect()
}

#[tauri::command]
pub async fn rtk_serial_ports(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<BasePort>, String> {
    let service = crate::serial_service::ensure(&app, &state).await?;
    let inventory = service.discovery.snapshot();
    Ok(inventory
        .ports
        .into_iter()
        .map(|port| {
            let unavailable_reason = if let Some(lease) = &port.lease {
                Some(format!("In use by {:?}", lease.owner))
            } else if port.suggested_role == DeviceRole::FlightController {
                Some("Flight controller — not a base receiver".into())
            } else {
                None
            };
            BasePort {
                device: port.port.device.clone(),
                port_ref: Some(port.port),
                suggested_role: port.suggested_role,
                unavailable_reason,
            }
        })
        .collect())
}

#[tauri::command]
pub async fn rtk_start_serial(
    app: AppHandle,
    device: SerialDeviceInfo,
    port: SerialPortRef,
    baud_rate: u32,
    mode: BaseMode,
    state: State<'_, AppState>,
) -> Result<BaseSessionId, String> {
    let service = crate::serial_service::ensure(&app, &state).await?;
    let manual = service.begin_manual(AutoConnectRole::RtkBase).await?;
    if port.device != device {
        return Err("The selected serial device changed. Select it again.".into());
    }
    if DeviceDatabase::builtin().suggested_role(&device) == DeviceRole::FlightController {
        return Err("Flight controller — not a base receiver".into());
    }
    let lease = service
        .discovery
        .reserve(LeaseRequest {
            expected: port.clone(),
            owner: SerialOwner::RtkBase,
        })
        .await
        .map_err(|e| e.to_string())?;
    manual.check()?;
    let _ops = state
        .ops()
        .try_lock()
        .map_err(|_| "A vehicle connection change is in progress; retry base connection.")?;
    state.rtk.ensure_base_events(&app);
    let request = BaseSerialRequest {
        baud: if baud_rate == 0 {
            BaudSelection::Auto
        } else {
            BaudSelection::Fixed(baud_rate)
        },
        mode,
    };
    let id = state
        .rtk
        .base
        .start_leased_serial(lease, request, manual.cancel.clone())
        .await
        .map_err(|error| error.to_string())?;
    if let Err(error) = manual.check() {
        let _ = state.rtk.base.stop(id).await;
        return Err(error);
    }
    drop(_ops);
    manual
        .commit(
            SerialSessionRef::Base { id },
            Some(port),
            None,
            Some(request),
            None,
        )
        .await;
    Ok(id)
}

#[cfg(test)]
fn validate_selection(
    selected: &SerialDeviceInfo,
    current: Vec<SerialDeviceInfo>,
    occupied: &[PathBuf],
) -> Result<(), String> {
    let port = classify_ports(current, occupied)
        .into_iter()
        .find(|port| port.device == *selected)
        .ok_or("The selected serial device changed or was unplugged. Select it again.")?;
    if let Some(reason) = port.unavailable_reason {
        return Err(reason);
    }
    Ok(())
}

#[tauri::command]
pub async fn rtk_stop(
    app: AppHandle,
    session_id: BaseSessionId,
    expected_intent: Option<AutoConnectIntentRef>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if state.rtk.base.snapshot().session_id != Some(session_id) {
        return Err("base session has changed".into());
    }
    let service = state.serial.get().cloned();
    let revision = if let Some(service) = &service {
        let snapshot = service.snapshot();
        if expected_intent.is_some()
            || snapshot.rtk.session == Some(SerialSessionRef::Base { id: session_id })
        {
            Some(service.pause_memory(AutoConnectRole::RtkBase, expected_intent.as_ref())?)
        } else {
            None
        }
    } else {
        None
    };
    state
        .rtk
        .base
        .stop(session_id)
        .await
        .map_err(|error| error.to_string())?;
    state.rtk.clear_stats(&app);
    if let (Some(service), Some(revision)) = (service, revision) {
        service.persist(&revision).await?;
    }
    Ok(())
}

#[tauri::command]
pub fn rtk_base_status(state: State<'_, AppState>) -> BaseStatus {
    state.rtk.base.snapshot()
}

#[tauri::command]
pub fn rtk_link_status(state: State<'_, AppState>) -> RtkLinkStatus {
    state.rtk.link.lock().snapshot.clone()
}

#[tauri::command]
pub fn rtk_last_stats(state: State<'_, AppState>) -> Option<RtkStats> {
    state.rtk.link.lock().snapshot.stats.clone()
}

#[tauri::command]
pub fn rtk_foreign_state(state: State<'_, AppState>) -> Option<ForeignState> {
    Some(state.rtk.link.lock().snapshot.foreign)
}

#[cfg(test)]
mod tests;
