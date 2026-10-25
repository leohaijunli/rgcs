//! Shared inventory and native per-role automatic serial supervision.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use maggcs_core::config::serial::*;
use maggcs_core::devices::{
    canonical_serial_path, spawn_serial_discovery, stable_usb_id, BaudSelection, DeviceDatabase,
    DeviceManager, InventoryHealth, LeaseRequest, SerialCancellation, SerialDiscoveryConfig,
    SerialDiscoveryHandle, SerialInventory, SerialLease, SerialOwner, SerialPortRef, StableUsbId,
};
use maggcs_core::mavlink::serial::{probe_fc_serial, FcIdentity, FcProbeConfig};
use maggcs_core::mavlink::{ConnectionHandle, Endpoint, LinkState};
use maggcs_core::rtk::base::{BaseFailureKind, BasePhase, BaseSerialRequest};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::state::AppState;

const SUPERVISOR_TICK: Duration = Duration::from_millis(100);
const STOP_BUDGET: Duration = maggcs_core::rtk::base::BASE_STOP_TIMEOUT;
const SETTINGS_IO_BUDGET: Duration = Duration::from_secs(2);

#[derive(Default)]
struct RoleControl {
    cancel: SerialCancellation,
    task: Option<JoinHandle<()>>,
    manual: bool,
    pinned: Option<StableUsbId>,
    learned: Option<FcIdentity>,
    active: Option<SerialSessionRef>,
    failures: u32,
    retry_at: Option<Instant>,
    blocked: bool,
}

struct Inner {
    snapshot: SerialAutoConnectSnapshot,
    controls: [RoleControl; 2],
}

pub struct SerialService {
    pub discovery: SerialDiscoveryHandle,
    inner: Mutex<Inner>,
    path: Option<PathBuf>,
    writer: Arc<tokio::sync::Mutex<()>>,
    emit: Box<dyn Fn(&SerialAutoConnectSnapshot) + Send + Sync>,
    wake: Notify,
    closing: AtomicBool,
    closed: AtomicBool,
    unreadable_file: AtomicBool,
    needs_legacy_import: AtomicBool,
    bootstrap: tokio::sync::Mutex<()>,
}

fn index(role: AutoConnectRole) -> usize {
    match role {
        AutoConnectRole::PrimaryFc => 0,
        AutoConnectRole::RtkBase => 1,
    }
}

fn status_mut(
    snapshot: &mut SerialAutoConnectSnapshot,
    role: AutoConnectRole,
) -> &mut AutoConnectStatus {
    match role {
        AutoConnectRole::PrimaryFc => &mut snapshot.fc,
        AutoConnectRole::RtkBase => &mut snapshot.rtk,
    }
}

fn status(snapshot: &SerialAutoConnectSnapshot, role: AutoConnectRole) -> &AutoConnectStatus {
    match role {
        AutoConnectRole::PrimaryFc => &snapshot.fc,
        AutoConnectRole::RtkBase => &snapshot.rtk,
    }
}

fn revision() -> Result<String, String> {
    AutoConnectIntentRef::next(AutoConnectRole::PrimaryFc)
        .map(|id| id.id)
        .map_err(|e| e.to_string())
}

pub async fn ensure(app: &AppHandle, state: &AppState) -> Result<Arc<SerialService>, String> {
    state
        .serial
        .get_or_try_init(|| async {
            let path = app
                .path()
                .app_config_dir()
                .map(|dir| dir.join(SERIAL_SETTINGS_FILE));
            let (settings, error, missing) = match &path {
                Ok(path) => {
                    let path = path.clone();
                    let legacy_auto = state.config().auto_connect;
                    match tokio::time::timeout(
                        SETTINGS_IO_BUDGET,
                        tokio::task::spawn_blocking(move || load_settings(&path, legacy_auto)),
                    )
                    .await
                    {
                        Ok(result) => result.map_err(|e| e.to_string())?,
                        Err(_) => (
                            SerialAutoConnectSettings::default(),
                            Some("serial settings load timed out".into()),
                            false,
                        ),
                    }
                }
                Err(error) => (
                    SerialAutoConnectSettings::default(),
                    Some(error.to_string()),
                    false,
                ),
            };
            let (discovery, task) = spawn_serial_discovery(
                Arc::new(DeviceManager::new(DeviceDatabase::builtin())),
                SerialDiscoveryConfig::default(),
            )
            .map_err(|e| e.to_string())?;
            // Shutdown joins through the discovery handle; this task is process-owned.
            drop(task);
            let event_app = app.clone();
            let unreadable_file = error.is_some();
            let service = Arc::new(SerialService {
                discovery,
                inner: Mutex::new(Inner {
                    snapshot: SerialAutoConnectSnapshot {
                        revision: revision()?,
                        settings_revision: revision()?,
                        settings,
                        primary_startup_owner: PrimaryStartupOwner::SerialDiscovery,
                        fc: AutoConnectStatus::new(AutoConnectRole::PrimaryFc)
                            .map_err(|e| e.to_string())?,
                        rtk: AutoConnectStatus::new(AutoConnectRole::RtkBase)
                            .map_err(|e| e.to_string())?,
                        settings_error: error,
                    },
                    controls: [RoleControl::default(), RoleControl::default()],
                }),
                path: path.ok(),
                writer: Arc::new(tokio::sync::Mutex::new(())),
                emit: Box::new(move |snapshot| {
                    let _ = event_app.emit("serial_auto_connect_status", snapshot);
                }),
                wake: Notify::new(),
                closing: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                unreadable_file: AtomicBool::new(unreadable_file),
                needs_legacy_import: AtomicBool::new(missing),
                bootstrap: tokio::sync::Mutex::new(()),
            });
            service.update_startup_owner(&state.config().endpoint);
            let worker_service = service.clone();
            let worker_app = app.clone();
            tauri::async_runtime::spawn(async move {
                run(worker_app, worker_service).await;
            });
            Ok(service)
        })
        .await
        .cloned()
}

impl SerialService {
    pub fn snapshot(&self) -> SerialAutoConnectSnapshot {
        self.inner.lock().snapshot.clone()
    }

    async fn import_legacy(&self, receiver: Option<BaseSerialRequest>) {
        let _bootstrap = self.bootstrap.lock().await;
        if !self.needs_legacy_import.load(Ordering::Acquire) {
            return;
        }
        let revision = {
            let mut inner = self.inner.lock();
            if let Some(receiver) = receiver {
                let mut settings = inner.snapshot.settings.clone();
                settings.rtk.receiver = receiver;
                if settings.validate().is_ok() {
                    inner.snapshot.settings = settings;
                }
            }
            inner.snapshot.settings_revision =
                revision().unwrap_or_else(|_| inner.snapshot.settings_revision.clone());
            self.publish(&mut inner);
            inner.snapshot.settings_revision.clone()
        };
        let _ = self.persist(&revision).await;
        self.needs_legacy_import.store(false, Ordering::Release);
        self.wake.notify_one();
    }

    fn publish(&self, inner: &mut Inner) {
        if let Ok(revision) = revision() {
            inner.snapshot.revision = revision;
        }
        (self.emit)(&inner.snapshot);
        self.wake.notify_one();
    }

    fn update_startup_owner(&self, endpoint: &str) {
        let mut inner = self.inner.lock();
        inner.snapshot.primary_startup_owner = if inner.snapshot.settings_error.is_some() {
            PrimaryStartupOwner::BlockedSettings
        } else if inner.snapshot.settings.fc.policy.enabled {
            PrimaryStartupOwner::SerialDiscovery
        } else if endpoint.starts_with("serial:") {
            PrimaryStartupOwner::ManualSerialSelection
        } else {
            PrimaryStartupOwner::LegacyEndpoint
        };
        self.publish(&mut inner);
    }

    pub fn is_current(&self, expected: &AutoConnectIntentRef) -> bool {
        !self.closing.load(Ordering::Acquire)
            && status(&self.inner.lock().snapshot, expected.role).intent == *expected
    }

    pub(crate) fn install_primary(
        &self,
        state: &AppState,
        expected: &AutoConnectIntentRef,
        port: Option<SerialPortRef>,
        link: crate::state::ActiveLink,
    ) -> Result<Option<crate::state::ActiveLink>, String> {
        let mut inner = self.inner.lock();
        if self.closing.load(Ordering::Acquire)
            || status(&inner.snapshot, expected.role).intent != *expected
        {
            return Err("connection intent changed before installation".into());
        }
        let session = fc_session(&link.connection);
        let learned = link.connection.target_identity();
        if let Some(port) = &port {
            if !self
                .discovery
                .snapshot()
                .ports
                .iter()
                .any(|p| &p.port == port)
            {
                return Err("serial attachment changed before installation".into());
            }
        }
        let previous = state.set_primary_link(link);
        let control = &mut inner.controls[index(expected.role)];
        control.active = Some(session.clone());
        control.cancel = SerialCancellation::new();
        control.pinned = port.as_ref().and_then(|p| stable_usb_id(&p.device));
        control.learned = learned;
        let current = status_mut(&mut inner.snapshot, expected.role);
        current.session = Some(session);
        current.phase = AutoConnectPhase::Running;
        current.port = port;
        self.publish(&mut inner);
        Ok(previous)
    }

    fn renew(inner: &mut Inner, role: AutoConnectRole) -> Result<AutoConnectIntentRef, String> {
        let control = &mut inner.controls[index(role)];
        control.cancel.cancel();
        control.cancel = SerialCancellation::new();
        control.failures = 0;
        control.retry_at = None;
        control.blocked = false;
        let next = AutoConnectIntentRef::next(role).map_err(|e| e.to_string())?;
        let current = status_mut(&mut inner.snapshot, role);
        current.intent = next.clone();
        current.revision = revision()?;
        current.attempt = None;
        current.error = None;
        current.retry_in_ms = None;
        Ok(next)
    }

    pub async fn begin_manual(
        self: &Arc<Self>,
        role: AutoConnectRole,
    ) -> Result<ManualAttempt, String> {
        if self.closing.load(Ordering::Acquire) {
            return Err("application is shutting down".into());
        }
        let (intent, cancel, task) = {
            let mut inner = self.inner.lock();
            let intent = Self::renew(&mut inner, role)?;
            inner.controls[index(role)].manual = true;
            let task = inner.controls[index(role)].task.take();
            let cancel = inner.controls[index(role)].cancel.clone();
            status_mut(&mut inner.snapshot, role).phase = AutoConnectPhase::Starting;
            self.publish(&mut inner);
            (intent, cancel, task)
        };
        let attempt = ManualAttempt {
            service: self.clone(),
            intent,
            cancel,
            finished: false,
        };
        if let Some(mut task) = task {
            if tokio::time::timeout(STOP_BUDGET, &mut task).await.is_err() {
                // The retiring task/port still owns its lease and cannot be reused.
                self.inner.lock().controls[index(role)].task = Some(task);
                return Err(
                    "automatic serial probe is still stopping; retry manual connection".into(),
                );
            }
        }
        Ok(attempt)
    }

    pub async fn reserve_endpoint(
        &self,
        endpoint: &Endpoint,
        expected: Option<SerialPortRef>,
        owner: SerialOwner,
    ) -> Result<Option<SerialLease>, String> {
        let Endpoint::Serial { port, .. } = endpoint else {
            return Ok(None);
        };
        let expected = expected.ok_or("Select a current serial device before connecting.")?;
        if canonical_serial_path(port)
            != canonical_serial_path(Path::new(&expected.device.port_name))
        {
            return Err("The selected serial device does not match the endpoint.".into());
        }
        self.discovery
            .reserve(LeaseRequest { expected, owner })
            .await
            .map(Some)
            .map_err(|e| e.to_string())
    }

    fn running(
        &self,
        expected: &AutoConnectIntentRef,
        session: SerialSessionRef,
        port: Option<SerialPortRef>,
        learned: Option<FcIdentity>,
    ) {
        let mut inner = self.inner.lock();
        if self.closing.load(Ordering::Acquire)
            || status(&inner.snapshot, expected.role).intent != *expected
        {
            return;
        }
        let control = &mut inner.controls[index(expected.role)];
        control.active = Some(session.clone());
        control.cancel = SerialCancellation::new(); // active I/O stops through its session owner
        control.pinned = port.as_ref().and_then(|p| stable_usb_id(&p.device));
        control.learned = learned;
        control.retry_at = None;
        control.failures = 0;
        let current = status_mut(&mut inner.snapshot, expected.role);
        current.phase = AutoConnectPhase::Running;
        current.session = Some(session);
        current.port = port;
        current.error = None;
        current.retry_in_ms = None;
        current.revision = revision().unwrap_or_else(|_| current.revision.clone());
        self.publish(&mut inner);
    }

    fn phase(
        &self,
        role: AutoConnectRole,
        phase: AutoConnectPhase,
        error: Option<SerialFailure>,
        retry_in_ms: Option<u32>,
    ) {
        let mut inner = self.inner.lock();
        if self.closing.load(Ordering::Acquire) {
            return;
        }
        let current = status_mut(&mut inner.snapshot, role);
        if current.phase == phase && current.error == error && current.retry_in_ms == retry_in_ms {
            return;
        }
        current.phase = phase;
        current.error = error;
        current.retry_in_ms = retry_in_ms;
        current.revision = revision().unwrap_or_else(|_| current.revision.clone());
        self.publish(&mut inner);
    }

    fn failed(&self, expected: &AutoConnectIntentRef, failure: SerialFailure) {
        let mut inner = self.inner.lock();
        if self.closing.load(Ordering::Acquire)
            || status(&inner.snapshot, expected.role).intent != *expected
        {
            return;
        }
        let control = &mut inner.controls[index(expected.role)];
        control.active = None;
        control.blocked = failure.retry == RetryDisposition::UserAction;
        if failure.retry == RetryDisposition::Backoff {
            control.retry_at = Some(Instant::now() + retry_delay(control.failures));
            control.failures = control.failures.saturating_add(1);
        } else {
            control.retry_at = None;
        }
        let current = status_mut(&mut inner.snapshot, expected.role);
        current.session = None;
        current.phase = match failure.retry {
            RetryDisposition::Backoff => AutoConnectPhase::RetryWait,
            RetryDisposition::WaitForDevice => AutoConnectPhase::WaitingForDevice,
            RetryDisposition::WaitForPort => AutoConnectPhase::WaitingForPort,
            RetryDisposition::UserAction => AutoConnectPhase::Failed,
        };
        current.error = Some(failure);
        current.revision = revision().unwrap_or_else(|_| current.revision.clone());
        self.publish(&mut inner);
    }

    /// Immediate policy pause; device teardown and persistence are independent.
    pub fn pause_memory(
        &self,
        role: AutoConnectRole,
        expected: Option<&AutoConnectIntentRef>,
    ) -> Result<String, String> {
        let mut inner = self.inner.lock();
        if expected.is_some_and(|expected| status(&inner.snapshot, role).intent != *expected) {
            return Err("automatic connection intent changed; refresh status".into());
        }
        Self::renew(&mut inner, role)?;
        inner.controls[index(role)].manual = false;
        inner.controls[index(role)].active = None;
        inner.snapshot.settings.policy_mut(role).paused_by_user = true;
        inner.snapshot.settings_revision = revision()?;
        let current = status_mut(&mut inner.snapshot, role);
        current.phase = AutoConnectPhase::Paused;
        current.session = None;
        let revision = inner.snapshot.settings_revision.clone();
        self.publish(&mut inner);
        Ok(revision)
    }

    async fn write(
        &self,
        expected: &str,
        settings: &SerialAutoConnectSettings,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, String> {
        settings.validate()?;
        let deadline = tokio::time::Instant::now() + SETTINGS_IO_BUDGET;
        let writer = tokio::time::timeout_at(deadline, self.writer.clone().lock_owned())
            .await
            .map_err(|_| "serial settings writer is busy")?;
        if self.inner.lock().snapshot.settings_revision != expected {
            return Err("serial settings changed; refresh before saving".into());
        }
        let path = self
            .path
            .clone()
            .ok_or("cannot resolve serial settings directory")?;
        let settings = settings.clone();
        let mut job = tokio::task::spawn_blocking(move || {
            // Return the guard so disk replacement and the subsequent cache
            // commit are one serialized update. A timed-out caller still leaves
            // the guard with this blocking job until the write really finishes.
            write_settings(&path, &settings).map(|_| writer)
        });
        match tokio::time::timeout_at(deadline, &mut job).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => Err(error.to_string()),
            Err(_) => Err("serial settings write is still pending".into()),
        }
    }

    pub async fn persist(&self, expected: &str) -> Result<(), String> {
        if self.unreadable_file.load(Ordering::Acquire) {
            return Err(
                "unreadable serial settings retained; save settings explicitly to replace the file"
                    .into(),
            );
        }
        let settings = self.inner.lock().snapshot.settings.clone();
        let result = self.write(expected, &settings).await;
        let mut inner = self.inner.lock();
        if inner.snapshot.settings_revision == expected {
            inner.snapshot.settings_error = result.as_ref().err().cloned();
            self.publish(&mut inner);
        }
        result.map(|_writer| ())
    }

    async fn save(
        &self,
        expected: &str,
        settings: SerialAutoConnectSettings,
    ) -> Result<SerialAutoConnectSnapshot, String> {
        self.save_with_reset(expected, settings, None).await
    }

    async fn save_with_reset(
        &self,
        expected: &str,
        settings: SerialAutoConnectSettings,
        reset: Option<AutoConnectRole>,
    ) -> Result<SerialAutoConnectSnapshot, String> {
        let _writer = self.write(expected, &settings).await?;
        let mut inner = self.inner.lock();
        if inner.snapshot.settings_revision != expected {
            return Err("serial settings changed during save".into());
        }
        let changed = [
            inner.snapshot.settings.fc != settings.fc,
            inner.snapshot.settings.rtk != settings.rtk,
        ];
        let repaired = inner.snapshot.settings_error.is_some();
        inner.snapshot.settings = settings;
        inner.snapshot.settings_revision = revision()?;
        inner.snapshot.settings_error = None;
        self.unreadable_file.store(false, Ordering::Release);
        for role in [AutoConnectRole::PrimaryFc, AutoConnectRole::RtkBase] {
            if changed[index(role)] || repaired || reset == Some(role) {
                Self::renew(&mut inner, role)?;
                inner.controls[index(role)].pinned = None;
                inner.controls[index(role)].learned = None;
            }
        }
        self.publish(&mut inner);
        Ok(inner.snapshot.clone())
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        self.closing.store(true, Ordering::Release);
        self.closed.store(true, Ordering::Release);
        let tasks: Vec<_> = {
            let mut inner = self.inner.lock();
            inner
                .controls
                .iter_mut()
                .filter_map(|control| {
                    control.cancel.cancel();
                    control.task.take()
                })
                .collect()
        };
        self.wake.notify_waiters();
        for mut task in tasks {
            tokio::time::timeout(STOP_BUDGET, &mut task)
                .await
                .map_err(|_| "serial attempt is still stopping")?
                .map_err(|e| e.to_string())?;
        }
        self.discovery
            .shutdown(STOP_BUDGET)
            .await
            .map_err(|e| e.to_string())
    }

    pub fn quiesce(&self) {
        self.closing.store(true, Ordering::Release);
        let inner = self.inner.lock();
        for control in &inner.controls {
            control.cancel.cancel();
        }
        self.wake.notify_waiters();
    }

    pub fn resume_after_refusal(&self) {
        if !self.closed.load(Ordering::Acquire) {
            self.closing.store(false, Ordering::Release);
            self.wake.notify_one();
        }
    }
}

pub struct ManualAttempt {
    service: Arc<SerialService>,
    pub intent: AutoConnectIntentRef,
    pub cancel: SerialCancellation,
    finished: bool,
}

impl ManualAttempt {
    pub fn check(&self) -> Result<(), String> {
        if !self.cancel.is_cancelled() && self.service.is_current(&self.intent) {
            Ok(())
        } else {
            Err("manual connection was superseded or cancelled".into())
        }
    }

    pub async fn commit(
        mut self,
        session: SerialSessionRef,
        port: Option<SerialPortRef>,
        learned: Option<FcIdentity>,
        receiver: Option<BaseSerialRequest>,
        baud: Option<BaudSelection>,
    ) {
        if self.check().is_err() {
            return;
        }
        self.service
            .running(&self.intent, session, port.clone(), learned.clone());
        let revision = {
            let mut inner = self.service.inner.lock();
            if status(&inner.snapshot, self.intent.role).intent != self.intent {
                return;
            }
            let policy = inner.snapshot.settings.policy_mut(self.intent.role);
            policy.paused_by_user = false;
            if let Some(port) = port {
                policy.saved = Some(SavedSerialDevice::from_port(&port));
            }
            if let Some(receiver) = receiver {
                inner.snapshot.settings.rtk.receiver = receiver;
            }
            if self.intent.role == AutoConnectRole::PrimaryFc {
                inner.snapshot.settings.fc.expected_target = learned;
                if let Some(baud) = baud {
                    inner.snapshot.settings.fc.baud = baud;
                }
            }
            inner.snapshot.settings_revision =
                revision().unwrap_or_else(|_| inner.snapshot.settings_revision.clone());
            self.service.publish(&mut inner);
            inner.snapshot.settings_revision.clone()
        };
        let _ = self.service.persist(&revision).await;
        let mut inner = self.service.inner.lock();
        if status(&inner.snapshot, self.intent.role).intent == self.intent {
            inner.controls[index(self.intent.role)].manual = false;
        }
        self.finished = true;
        self.service.wake.notify_one();
    }
}

impl Drop for ManualAttempt {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let mut inner = self.service.inner.lock();
        if status(&inner.snapshot, self.intent.role).intent == self.intent {
            inner.controls[index(self.intent.role)].manual = false;
            let active = inner.controls[index(self.intent.role)].active.clone();
            inner.controls[index(self.intent.role)].blocked = active.is_none();
            let current = status_mut(&mut inner.snapshot, self.intent.role);
            current.session = active;
            current.phase = if current.session.is_some() {
                AutoConnectPhase::Running
            } else {
                AutoConnectPhase::Failed
            };
            if current.session.is_none() {
                current.error = Some(SerialFailure {
                    code: "manual_connection_incomplete".into(),
                    message:
                        "Manual connection did not complete. Choose Connect or Resume to retry."
                            .into(),
                    stage: "manual connection".into(),
                    retry: RetryDisposition::UserAction,
                });
            }
            self.service.publish(&mut inner);
        }
    }
}

async fn run(app: AppHandle, service: Arc<SerialService>) {
    let mut inventory = service.discovery.subscribe();
    let mut tick = tokio::time::interval(SUPERVISOR_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut published = None;
    loop {
        if service.closed.load(Ordering::Acquire) {
            return;
        }
        let current = inventory.borrow_and_update().clone();
        if published != Some(current.revision) {
            published = Some(current.revision);
            let _ = app.emit("serial_inventory", &current);
            let devices: Vec<_> = current
                .ports
                .iter()
                .map(|p| p.port.device.clone())
                .collect();
            let _ = app.emit("devices_changed", devices);
        }
        if !service.closing.load(Ordering::Acquire) {
            for role in [AutoConnectRole::PrimaryFc, AutoConnectRole::RtkBase] {
                drive(&app, &service, role, &current).await;
            }
        }
        tokio::select! {
            _ = tick.tick() => {},
            _ = service.wake.notified() => {},
            changed = inventory.changed() => { if changed.is_err() { return; } },
        }
    }
}

async fn drive(
    app: &AppHandle,
    service: &Arc<SerialService>,
    role: AutoConnectRole,
    inventory: &SerialInventory,
) {
    if role == AutoConnectRole::RtkBase && service.needs_legacy_import.load(Ordering::Acquire) {
        return;
    }
    let state = app.state::<AppState>();
    let (settings, intent, pinned, learned, active, blocked, retry_at) = {
        let mut inner = service.inner.lock();
        let control = &mut inner.controls[index(role)];
        if control.task.as_ref().is_some_and(|task| task.is_finished()) {
            control.task.take();
        }
        if control.manual || control.task.is_some() {
            return;
        }
        (
            inner.snapshot.settings.clone(),
            status(&inner.snapshot, role).intent.clone(),
            inner.controls[index(role)].pinned.clone(),
            inner.controls[index(role)].learned.clone(),
            inner.controls[index(role)].active.clone(),
            inner.controls[index(role)].blocked,
            inner.controls[index(role)].retry_at,
        )
    };
    let policy = settings.policy(role);
    if policy.paused_by_user {
        service.phase(role, AutoConnectPhase::Paused, None, None);
        return;
    }
    if role == AutoConnectRole::PrimaryFc {
        if let Some(connection) = state.connection() {
            let link_state = connection.state().await;
            if link_state != LinkState::Disconnected {
                let session = fc_session(&connection);
                {
                    let mut inner = service.inner.lock();
                    if status(&inner.snapshot, role).intent != intent {
                        return;
                    }
                    let control = &mut inner.controls[index(role)];
                    if control.learned.is_none() {
                        control.learned = connection.target_identity();
                    }
                    if control.active.as_ref() != Some(&session) {
                        control.active = Some(session.clone());
                        status_mut(&mut inner.snapshot, role).session = Some(session);
                        service.publish(&mut inner);
                    }
                }
                service.phase(
                    role,
                    if link_state == LinkState::Connected {
                        AutoConnectPhase::Running
                    } else {
                        AutoConnectPhase::Starting
                    },
                    None,
                    None,
                );
                return;
            }
        }
    } else {
        let base = state.rtk.base.snapshot();
        if base.phase.running() {
            let missing = inventory.health == InventoryHealth::Ready
                && service
                    .inner
                    .lock()
                    .snapshot
                    .rtk
                    .port
                    .as_ref()
                    .is_some_and(|selected| {
                        !inventory
                            .ports
                            .iter()
                            .any(|p| p.port.instance_id == selected.instance_id)
                    });
            if missing {
                if let Some(id) = base.session_id {
                    let _ = state.rtk.base.stop(id).await;
                }
            } else {
                service.phase(
                    role,
                    if matches!(base.phase, BasePhase::Identifying | BasePhase::Configuring) {
                        AutoConnectPhase::Starting
                    } else {
                        AutoConnectPhase::Running
                    },
                    None,
                    None,
                );
                return;
            }
        }
        if matches!(active, Some(SerialSessionRef::Base { id }) if Some(id) == base.session_id)
            && base.phase == BasePhase::Failed
        {
            service.failed(
                &intent,
                SerialFailure {
                    code: "base_failed".into(),
                    message: base.error.unwrap_or_else(|| "base receiver stopped".into()),
                    stage: "receiver".into(),
                    retry: if base.failure_kind == Some(BaseFailureKind::Transport) {
                        RetryDisposition::Backoff
                    } else {
                        RetryDisposition::UserAction
                    },
                },
            );
            return;
        }
    }
    if active.is_some() {
        service.failed(
            &intent,
            SerialFailure {
                code: "link_lost".into(),
                message: "serial session ended".into(),
                stage: "connection".into(),
                retry: RetryDisposition::Backoff,
            },
        );
        return;
    }
    if !policy.enabled {
        service.phase(role, AutoConnectPhase::Disabled, None, None);
        return;
    }
    if service.snapshot().settings_error.is_some() {
        service.phase(role, AutoConnectPhase::Failed, None, None);
        return;
    }
    if blocked {
        return;
    }
    if let Some(remaining) = retry_at.and_then(|at| at.checked_duration_since(Instant::now())) {
        let error = status(&service.snapshot(), role).error.clone();
        service.phase(
            role,
            AutoConnectPhase::RetryWait,
            error,
            Some(remaining.as_millis().min(u128::from(u32::MAX)) as u32),
        );
        return;
    }
    let selected = match select_candidate(inventory, &settings, role, pinned.as_ref()) {
        Ok(Some(port)) => port,
        Ok(None) => {
            service.phase(role, AutoConnectPhase::WaitingForDevice, None, None);
            return;
        }
        Err(error) => {
            let failure = SerialFailure::discovery(&error);
            let phase = match failure.retry {
                RetryDisposition::WaitForPort => AutoConnectPhase::WaitingForPort,
                RetryDisposition::WaitForDevice => AutoConnectPhase::WaitingForDevice,
                _ => AutoConnectPhase::NeedsSelection,
            };
            service.phase(role, phase, Some(failure), None);
            return;
        }
    };
    let cancel = {
        let mut inner = service.inner.lock();
        if status(&inner.snapshot, role).intent != intent || inner.controls[index(role)].manual {
            return;
        }
        let control = &mut inner.controls[index(role)];
        control.pinned = stable_usb_id(&selected.device);
        control.cancel = SerialCancellation::new();
        let cancel = control.cancel.clone();
        let current = status_mut(&mut inner.snapshot, role);
        current.phase = AutoConnectPhase::Probing;
        current.port = Some(selected.clone());
        current.attempt = revision().ok();
        current.error = None;
        service.publish(&mut inner);
        cancel
    };
    let worker_app = app.clone();
    let worker_service = service.clone();
    let expected = intent.clone();
    let task = tokio::spawn(async move {
        let result = async {
            let lease = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(()),
                result = worker_service.discovery.reserve(LeaseRequest { expected: selected.clone(), owner: role.owner() }) => result.map_err(|e| SerialFailure::discovery(&e))?,
            };
            cancel.check().map_err(|e| SerialFailure::discovery(&e))?;
            if role == AutoConnectRole::RtkBase {
                let state = worker_app.state::<AppState>();
                let id = state.rtk.base.start_leased_serial(lease, settings.rtk.receiver, cancel.clone()).await
                    .map_err(|e| SerialFailure { code: "base_start_failed".into(), message: e.to_string(), stage: "receiver".into(), retry: RetryDisposition::Backoff })?;
                if !worker_service.is_current(&expected) { let _ = state.rtk.base.stop(id).await; return Ok(()); }
                worker_service.running(&expected, SerialSessionRef::Base { id }, Some(selected), None);
            } else {
                let probed = probe_fc_serial(lease, FcProbeConfig { baud: settings.fc.baud, local_system_id: maggcs_core::mavlink::ConnectionConfig::default().system_id,
                    expected_target: settings.fc.expected_target.or(learned) }, cancel.clone()).await
                    .map_err(|e| SerialFailure::discovery(&e))?;
                let target = probed.target().clone();
                let handle = crate::commands::connect_probed_serial(worker_app.clone(), probed, expected.clone()).await
                    .map_err(|message| SerialFailure { code: "fc_attach_failed".into(), message, stage: "connection".into(), retry: RetryDisposition::Backoff })?;
                worker_service.running(&expected, fc_session(&handle), Some(selected), Some(target));
            }
            Ok::<(), SerialFailure>(())
        }.await;
        if let Err(error) = result {
            if !cancel.is_cancelled() {
                worker_service.failed(&expected, error);
            }
        }
    });
    let mut inner = service.inner.lock();
    if status(&inner.snapshot, role).intent == intent && !inner.controls[index(role)].manual {
        inner.controls[index(role)].task = Some(task);
    } else {
        task.abort();
    }
}

pub fn fc_session(handle: &ConnectionHandle) -> SerialSessionRef {
    SerialSessionRef::Fc {
        connection_id: handle.handle_id().to_string(),
        generation: handle.generation().to_string(),
    }
}

fn load_settings(
    path: &Path,
    legacy_auto: bool,
) -> (SerialAutoConnectSettings, Option<String>, bool) {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<SerialAutoConnectSettings>(&text) {
            Ok(settings) => match settings.validate() {
                Ok(()) => (settings, None, false),
                Err(error) => (SerialAutoConnectSettings::default(), Some(error), false),
            },
            Err(error) => (
                SerialAutoConnectSettings::default(),
                Some(error.to_string()),
                false,
            ),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut settings = SerialAutoConnectSettings::default();
            settings.fc.policy.enabled = legacy_auto;
            (settings, None, true)
        }
        Err(error) => (
            SerialAutoConnectSettings::default(),
            Some(error.to_string()),
            false,
        ),
    }
}

fn write_settings(path: &Path, settings: &SerialAutoConnectSettings) -> Result<(), String> {
    settings.validate()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temporary = path.with_extension(format!("json.{}.tmp", revision()?));
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    let result = (|| {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|e: std::io::Error| e.to_string())
}

#[tauri::command]
pub async fn serial_inventory(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SerialInventory, String> {
    Ok(ensure(&app, &state).await?.discovery.snapshot())
}

#[tauri::command]
pub async fn serial_resolve_endpoint(
    app: AppHandle,
    state: State<'_, AppState>,
    endpoint: String,
) -> Result<SerialPortRef, String> {
    let service = ensure(&app, &state).await?;
    let Endpoint::Serial { port, .. } =
        Endpoint::try_from(endpoint.as_str()).map_err(|e| e.to_string())?
    else {
        return Err("select a serial endpoint".into());
    };
    let inventory = service
        .discovery
        .refresh()
        .await
        .map_err(|e| e.to_string())?;
    let canonical = canonical_serial_path(&port);
    let mut matches = inventory
        .ports
        .into_iter()
        .filter(|p| canonical_serial_path(Path::new(&p.port.device.port_name)) == canonical);
    let selected = matches
        .next()
        .ok_or("The serial device is unavailable. Select it again.")?;
    if matches.next().is_some() {
        return Err("serial endpoint is ambiguous".into());
    }
    Ok(selected.port)
}

#[tauri::command]
pub async fn serial_auto_connect_status(
    app: AppHandle,
    state: State<'_, AppState>,
    legacy_rtk: Option<BaseSerialRequest>,
) -> Result<SerialAutoConnectSnapshot, String> {
    let service = ensure(&app, &state).await?;
    service.import_legacy(legacy_rtk).await;
    service.update_startup_owner(&state.config().endpoint);
    Ok(service.snapshot())
}

#[tauri::command]
pub async fn serial_auto_connect_save(
    app: AppHandle,
    state: State<'_, AppState>,
    expected: String,
    settings: SerialAutoConnectSettings,
) -> Result<SerialAutoConnectSnapshot, String> {
    let service = ensure(&app, &state).await?;
    service.save(&expected, settings).await?;
    service.update_startup_owner(&state.config().endpoint);
    Ok(service.snapshot())
}

#[tauri::command]
pub async fn serial_auto_connect_resume(
    app: AppHandle,
    state: State<'_, AppState>,
    expected: AutoConnectIntentRef,
) -> Result<AutoConnectStatus, String> {
    let service = ensure(&app, &state).await?;
    if !service.is_current(&expected) {
        return Err("automatic connection intent changed".into());
    }
    let mut snapshot = service.snapshot();
    snapshot.settings.policy_mut(expected.role).paused_by_user = false;
    snapshot.settings.policy_mut(expected.role).enabled = true;
    let saved = service
        .save_with_reset(
            &snapshot.settings_revision,
            snapshot.settings,
            Some(expected.role),
        )
        .await?;
    Ok(status(&saved, expected.role).clone())
}

#[tauri::command]
pub async fn serial_auto_connect_pause(
    app: AppHandle,
    state: State<'_, AppState>,
    expected: AutoConnectIntentRef,
) -> Result<AutoConnectStatus, String> {
    let service = ensure(&app, &state).await?;
    if expected.role == AutoConnectRole::PrimaryFc {
        crate::commands::disconnect_for_intent(&app, &state, Some(&expected)).await?;
    } else {
        let revision = service.pause_memory(expected.role, Some(&expected))?;
        if let Some(id) = state.rtk.base.snapshot().session_id {
            state.rtk.base.stop(id).await.map_err(|e| e.to_string())?;
        }
        service.persist(&revision).await?;
    }
    Ok(status(&service.snapshot(), expected.role).clone())
}

#[cfg(test)]
mod tests;
