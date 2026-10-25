//! Shared serial inventory (AUTO-02a, ADR-028).
//!
//! One worker serializes refreshes and periodic scans. OS enumeration runs on
//! the blocking pool; a timed-out call is drained before another scan starts.
//! The retained watch snapshot recovers initial state and coalesced updates.
//! This service enumerates metadata only: it never opens or writes a port.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

use super::lease::{InventoryRegistry, LeaseRequest, SerialLease};
use super::manager::{is_operator_port, DEFAULT_WATCH_INTERVAL};
use super::{DeviceDatabase, DeviceError, DeviceManager, SerialDeviceInfo};

mod types;
pub use types::*;

/// Deadline includes time queued on the blocking pool.
pub const DEFAULT_ENUMERATION_TIMEOUT: Duration = Duration::from_secs(2);

static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);
static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);

/// Injectable OS inventory provider. Calls are serialized by the service.
pub trait SerialInventorySource: Send + Sync {
    fn enumerate(&self) -> Result<Vec<SerialDeviceInfo>, DeviceError>;
}

impl SerialInventorySource for DeviceManager {
    fn enumerate(&self) -> Result<Vec<SerialDeviceInfo>, DeviceError> {
        DeviceManager::enumerate(self)
    }
}

/// Polling and blocking-call budgets. Both durations must be positive.
#[derive(Debug, Clone, Copy)]
pub struct SerialDiscoveryConfig {
    pub poll_interval: Duration,
    pub enumeration_timeout: Duration,
}

impl Default for SerialDiscoveryConfig {
    fn default() -> Self {
        Self {
            poll_interval: DEFAULT_WATCH_INTERVAL,
            enumeration_timeout: DEFAULT_ENUMERATION_TIMEOUT,
        }
    }
}

/// Inventory and lifecycle errors. Protocol/lease errors are added with their
/// implementation slices rather than exposing unimplemented operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SerialDiscoveryError {
    #[error(transparent)]
    Enumeration(#[from] DeviceError),
    #[error("invalid serial discovery configuration: {field}")]
    InvalidConfig { field: &'static str },
    #[error("serial enumeration timed out")]
    EnumerationTimeout,
    #[error("serial enumeration returned conflicting metadata for {port}")]
    InvalidInventory { port: String },
    #[error("serial inventory is unavailable")]
    InventoryUnavailable,
    #[error("the selected serial device is no longer attached")]
    DeviceGone { instance: PortInstanceId },
    #[error("the selected serial device changed; select it again")]
    DeviceChanged { instance: PortInstanceId },
    #[error("serial port is in use by {owner:?}")]
    PortBusy { owner: Option<SerialOwner> },
    #[error("serial reservation is no longer current")]
    StaleAttempt,
    #[error("unsupported serial baud rate {baud}")]
    InvalidBaud { baud: u32 },
    #[error("select a device with a unique USB serial number for automatic reconnect")]
    StableIdentityRequired,
    #[error("serial device role {observed:?} does not match {expected:?}")]
    RoleMismatch {
        expected: super::DeviceRole,
        observed: super::DeviceRole,
    },
    #[error("multiple serial devices match; select the intended device")]
    AmbiguousDevices { ports: Vec<PortInstanceId> },
    #[error("permission denied for serial port {port}")]
    PermissionDenied { port: String },
    #[error("no autopilot heartbeat was verified at the selected baud rates")]
    NoAutopilot,
    #[error("serial probe timed out during {stage}")]
    ProbeTimeout { stage: String },
    #[error("automatic serial linking requires MAVLink 2")]
    UnsupportedMavlinkVersion,
    #[error("this autopilot/vehicle is not supported by automatic serial linking")]
    UnsupportedAutopilot {
        observed: crate::mavlink::serial::FcIdentity,
    },
    #[error("multiple autopilots were found; select the intended target")]
    AmbiguousTargets {
        targets: Vec<crate::mavlink::serial::FcIdentity>,
    },
    #[error("the autopilot differs from the saved target")]
    UnexpectedTarget {
        expected: crate::mavlink::serial::FcIdentity,
        observed: crate::mavlink::serial::FcIdentity,
    },
    #[error("autopilot system ID {system_id} conflicts with this GCS")]
    LocalSystemIdConflict { system_id: u8 },
    #[error("cannot open serial port {port}: {message}")]
    OpenFailed { port: String, message: String },
    #[error("serial discovery identities exhausted")]
    IdExhausted,
    #[error("serial discovery cancelled")]
    Cancelled,
    #[error("serial discovery worker is unavailable")]
    WorkerGone,
    #[error("timed out waiting for serial discovery to stop")]
    StopTimeout,
}

#[derive(Clone, Default)]
struct RefreshCompletion {
    through_request: u64,
    result: Option<Result<SerialInventory, SerialDiscoveryError>>,
}

/// Cloneable access to one shared inventory worker. Dropping the last handle
/// also requests shutdown; retained snapshot subscribers do not keep it alive.
#[derive(Clone)]
pub struct SerialDiscoveryHandle {
    pub(super) registry: Arc<InventoryRegistry>,
    inventory: watch::Receiver<SerialInventory>,
    requests: watch::Sender<u64>,
    shutdown: watch::Sender<bool>,
    completion: watch::Receiver<RefreshCompletion>,
    stopped: watch::Receiver<bool>,
}

/// Start one metadata-only inventory worker inside a Tokio runtime.
///
/// The first scan starts immediately. Subscribe before reading the retained
/// snapshot to avoid a fetch/subscribe race. The returned task can be joined;
/// use [`SerialDiscoveryHandle::shutdown`] to cancel and drain it rather than
/// aborting a task that may own an in-flight blocking OS enumeration.
pub fn spawn_serial_discovery(
    source: Arc<dyn SerialInventorySource>,
    config: SerialDiscoveryConfig,
) -> Result<(SerialDiscoveryHandle, JoinHandle<()>), SerialDiscoveryError> {
    for (field, duration) in [
        ("poll_interval", config.poll_interval),
        ("enumeration_timeout", config.enumeration_timeout),
    ] {
        if duration.is_zero() || Instant::now().checked_add(duration).is_none() {
            return Err(SerialDiscoveryError::InvalidConfig { field });
        }
    }
    let runtime =
        tokio::runtime::Handle::try_current().map_err(|_| SerialDiscoveryError::WorkerGone)?;
    let initial = SerialInventory {
        revision: InventoryRevision(next_id(&NEXT_REVISION)?),
        health: InventoryHealth::Starting,
        ports: Vec::new(),
    };
    let (inventory_tx, inventory_rx) = watch::channel(initial);
    let registry = Arc::new(InventoryRegistry::new(inventory_tx.clone()));
    let (requests_tx, requests_rx) = watch::channel(0);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (completion_tx, completion_rx) = watch::channel(RefreshCompletion::default());
    let (stopped_tx, stopped_rx) = watch::channel(false);
    let worker = Worker {
        registry: registry.clone(),
        source,
        config,
        database: DeviceDatabase::builtin(),
        inventory: inventory_tx,
        requests: requests_rx,
        shutdown: shutdown_rx,
        completion: completion_tx,
        stopped: stopped_tx,
    };
    let task = runtime.spawn(worker.run());
    Ok((
        SerialDiscoveryHandle {
            registry,
            inventory: inventory_rx,
            requests: requests_tx,
            shutdown: shutdown_tx,
            completion: completion_rx,
            stopped: stopped_rx,
        },
        task,
    ))
}

impl SerialDiscoveryHandle {
    pub fn snapshot(&self) -> SerialInventory {
        self.inventory.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<SerialInventory> {
        self.inventory.clone()
    }

    /// Atomically reserve a still-current selection after a fresh scan.
    pub async fn reserve(
        &self,
        request: LeaseRequest,
    ) -> Result<SerialLease, SerialDiscoveryError> {
        self.refresh().await?;
        let path = request.expected.device.port_name.clone();
        let canonical = tokio::task::spawn_blocking(move || {
            super::lease::canonical_serial_path(std::path::Path::new(&path))
        })
        .await
        .map_err(|_| SerialDiscoveryError::WorkerGone)?;
        if *self.shutdown.borrow() {
            return Err(SerialDiscoveryError::Cancelled);
        }
        self.registry.reserve(request, canonical, self.clone())
    }

    pub(crate) async fn cancelled(&self) {
        cancelled(&mut self.shutdown.subscribe()).await;
    }

    /// Request a scan started after this call. Concurrent requests coalesce
    /// into one scan, with at most one follow-up to an already-running scan.
    /// An unavailable result is an error, never an apparently fresh cached list.
    pub async fn refresh(&self) -> Result<SerialInventory, SerialDiscoveryError> {
        if *self.shutdown.borrow() {
            return Err(SerialDiscoveryError::Cancelled);
        }
        if self.requests.is_closed() {
            return Err(SerialDiscoveryError::WorkerGone);
        }
        let mut completion = self.completion.clone();
        let mut shutdown = self.shutdown.subscribe();
        let mut ticket = None;
        self.requests.send_if_modified(|request| {
            let Some(next) = request.checked_add(1) else {
                return false;
            };
            *request = next;
            ticket = Some(next);
            true
        });
        let ticket = ticket.ok_or(SerialDiscoveryError::IdExhausted)?;
        tokio::select! {
            biased;
            _ = cancelled(&mut shutdown) => Err(SerialDiscoveryError::Cancelled),
            result = completion.wait_for(|state| state.through_request >= ticket) => {
                result
                    .map_err(|_| SerialDiscoveryError::WorkerGone)?
                    .result.clone().ok_or(SerialDiscoveryError::WorkerGone)?
            }
        }
    }

    /// Cancel and wait for the worker, including any blocking scan, to finish.
    /// A timeout retains the task; a later call can finish waiting for it.
    pub async fn shutdown(&self, deadline: Duration) -> Result<(), SerialDiscoveryError> {
        self.shutdown.send_replace(true);
        let mut stopped = self.stopped.clone();
        if *stopped.borrow() {
            return Ok(());
        }
        let result = tokio::time::timeout(deadline, stopped.wait_for(|done| *done)).await;
        match result {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(_)) => Err(SerialDiscoveryError::WorkerGone),
            Err(_) => Err(SerialDiscoveryError::StopTimeout),
        }
    }
}

struct Worker {
    registry: Arc<InventoryRegistry>,
    source: Arc<dyn SerialInventorySource>,
    config: SerialDiscoveryConfig,
    database: DeviceDatabase,
    inventory: watch::Sender<SerialInventory>,
    requests: watch::Receiver<u64>,
    shutdown: watch::Receiver<bool>,
    completion: watch::Sender<RefreshCompletion>,
    stopped: watch::Sender<bool>,
}

impl Worker {
    async fn run(mut self) {
        match self.run_until_stopped().await {
            Ok(()) => {
                self.stopped.send_replace(true);
            }
            Err(error) => {
                let _ = self.fail_pending(error);
            }
        }
    }

    async fn run_until_stopped(&mut self) -> Result<(), SerialDiscoveryError> {
        let mut tick = tokio::time::interval_at(
            Instant::now() + self.config.poll_interval,
            self.config.poll_interval,
        );
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut initial = true;
        loop {
            if !initial {
                tokio::select! {
                    biased;
                    _ = cancelled(&mut self.shutdown) => break,
                    changed = self.requests.changed() => {
                        if changed.is_err() { break; }
                    }
                    _ = tick.tick() => {}
                }
            }
            initial = false;
            if *self.shutdown.borrow() || self.shutdown.has_changed().is_err() {
                break;
            }
            let through_request = *self.requests.borrow_and_update();
            if !self.scan(through_request).await? {
                break;
            }
        }
        self.fail_pending(SerialDiscoveryError::Cancelled)?;
        self.registry.wait_until_released().await
    }

    /// Returns false after cancellation has drained the active blocking call.
    async fn scan(&mut self, through_request: u64) -> Result<bool, SerialDiscoveryError> {
        let source = self.source.clone();
        let deadline = Instant::now() + self.config.enumeration_timeout;
        let mut scan = tokio::task::spawn_blocking(move || source.enumerate());
        let result = tokio::select! {
            biased;
            _ = cancelled(&mut self.shutdown) => {
                let publication = self.fail_pending(SerialDiscoveryError::Cancelled);
                let _ = scan.await;
                publication?;
                return Ok(false);
            }
            result = &mut scan => Some(result),
            _ = tokio::time::sleep_until(deadline) => None,
        };
        if let Some(result) = result {
            // A delayed executor can observe both a finished call and an
            // expired timer. Never publish that late result as fresh evidence.
            if Instant::now() >= deadline {
                self.fail_pending(SerialDiscoveryError::EnumerationTimeout)?;
            } else {
                let result = match result {
                    Ok(result) => result.map_err(SerialDiscoveryError::from),
                    Err(error) => Err(DeviceError::WatcherGone(format!(
                        "enumeration task failed: {error}"
                    ))
                    .into()),
                };
                self.complete_scan(through_request, result)?;
            }
            return Ok(true);
        }

        let publication = self.fail_pending(SerialDiscoveryError::EnumerationTimeout);
        if let Err(error) = publication {
            let _ = scan.await;
            return Err(error);
        }
        // spawn_blocking cannot cancel an OS call already running. Retain it,
        // reject waiting refreshes, and discard its eventual result before
        // admitting another scan. No unbounded queue of blocking jobs builds up.
        loop {
            tokio::select! {
                biased;
                _ = cancelled(&mut self.shutdown) => {
                    let publication = self.fail_pending(SerialDiscoveryError::Cancelled);
                    let _ = scan.await;
                    publication?;
                    return Ok(false);
                }
                _ = &mut scan => return Ok(true),
                changed = self.requests.changed() => {
                    let error = if changed.is_err() {
                        SerialDiscoveryError::Cancelled
                    } else {
                        SerialDiscoveryError::EnumerationTimeout
                    };
                    if let Err(error) = self.fail_pending(error) {
                        let _ = scan.await;
                        return Err(error);
                    }
                }
            }
        }
    }

    fn complete_scan(
        &mut self,
        through_request: u64,
        result: Result<Vec<SerialDeviceInfo>, SerialDiscoveryError>,
    ) -> Result<(), SerialDiscoveryError> {
        let result = result.and_then(|devices| self.reconcile(devices));
        if let Err(error) = &result {
            self.publish(
                InventoryHealth::Unavailable {
                    message: error.to_string(),
                },
                None,
            )?;
        }
        self.completion.send_replace(RefreshCompletion {
            through_request,
            result: Some(result),
        });
        Ok(())
    }

    fn fail_pending(&mut self, error: SerialDiscoveryError) -> Result<(), SerialDiscoveryError> {
        let publication = self.publish(
            InventoryHealth::Unavailable {
                message: error.to_string(),
            },
            None,
        );
        self.completion.send_replace(RefreshCompletion {
            through_request: *self.requests.borrow_and_update(),
            result: Some(Err(error)),
        });
        publication.map(|_| ())
    }

    fn reconcile(
        &mut self,
        mut devices: Vec<SerialDeviceInfo>,
    ) -> Result<SerialInventory, SerialDiscoveryError> {
        devices.retain(|device| is_operator_port(&device.port_name));
        devices.sort_by(|a, b| a.port_name.cmp(&b.port_name));
        devices.dedup();
        for pair in devices.windows(2) {
            if pair[0].port_name == pair[1].port_name {
                return Err(SerialDiscoveryError::InvalidInventory {
                    port: pair[0].port_name.clone(),
                });
            }
        }
        let previous = self.inventory.borrow().clone();
        let mut ports = Vec::with_capacity(devices.len());
        for device in devices {
            let existing = previous.ports.iter().find(|port| {
                let old = &port.port.device;
                old.port_name == device.port_name
                    && old.transport == device.transport
                    && old.device_id == device.device_id
                    && old.serial_number == device.serial_number
            });
            let instance_id = match existing {
                Some(port) => port.port.instance_id,
                None => PortInstanceId(next_id(&NEXT_INSTANCE)?),
            };
            ports.push(SerialPortSnapshot {
                stable_id: stable_usb_id(&device),
                suggested_role: self.database.suggested_role(&device),
                port: SerialPortRef {
                    instance_id,
                    device,
                },
                lease: existing.and_then(|port| port.lease.clone()),
            });
        }
        self.publish(InventoryHealth::Ready, Some(ports))
    }

    fn publish(
        &self,
        health: InventoryHealth,
        ports: Option<Vec<SerialPortSnapshot>>,
    ) -> Result<SerialInventory, SerialDiscoveryError> {
        self.registry.publish(health, ports)
    }
}

async fn cancelled(shutdown: &mut watch::Receiver<bool>) {
    let _ = shutdown.wait_for(|stopping| *stopping).await;
}

fn next_id(counter: &AtomicU64) -> Result<u64, SerialDiscoveryError> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| SerialDiscoveryError::IdExhausted)
}

pub(super) fn next_revision() -> Result<InventoryRevision, SerialDiscoveryError> {
    next_id(&NEXT_REVISION).map(InventoryRevision)
}

#[cfg(test)]
mod tests;
