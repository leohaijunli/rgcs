//! Atomic serial reservations shared by manual and automatic connection paths.
//! A lease is moved into the I/O owner and dropped only after its port closes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::watch;

use super::discovery::{
    next_revision, stable_usb_id, InventoryHealth, LeasePhase, SerialDiscoveryError,
    SerialDiscoveryHandle, SerialInventory, SerialLeaseId, SerialLeaseStatus, SerialOwner,
    SerialPortRef, SerialPortSnapshot, StableUsbId,
};

static NEXT_LEASE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct LeaseRequest {
    pub expected: SerialPortRef,
    pub owner: SerialOwner,
}

struct Claim {
    port: SerialPortRef,
    canonical: PathBuf,
    stable_id: Option<StableUsbId>,
    status: SerialLeaseStatus,
}

/// Construction is private and the lease cannot be cloned or serialized.
/// The retained discovery handle keeps the registry alive until I/O teardown.
pub struct SerialLease {
    id: SerialLeaseId,
    owner: SerialOwner,
    port: SerialPortRef,
    canonical: PathBuf,
    discovery: SerialDiscoveryHandle,
}

impl std::fmt::Debug for SerialLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerialLease")
            .field("id", &self.id)
            .field("owner", &self.owner)
            .field("port", &self.port)
            .finish()
    }
}

impl SerialLease {
    pub fn id(&self) -> SerialLeaseId {
        self.id
    }
    pub fn owner(&self) -> SerialOwner {
        self.owner
    }
    pub fn port(&self) -> &SerialPortRef {
        &self.port
    }

    pub fn matches_path(&self, path: &Path) -> bool {
        path == Path::new(&self.port.device.port_name)
            || canonical_serial_path(path) == self.canonical
    }

    /// Recheck the selected attachment and ownership immediately before open.
    /// An observed detach needs a new lease even if the USB device returns.
    pub async fn revalidate(&self) -> Result<(), SerialDiscoveryError> {
        self.discovery.refresh().await?;
        self.discovery.registry.validate(self.id, &self.port)
    }

    pub fn set_phase(&self, phase: LeasePhase) -> Result<(), SerialDiscoveryError> {
        self.discovery.registry.set_phase(self.id, phase)
    }

    pub async fn cancelled(&self) {
        self.discovery.cancelled().await;
    }
}

impl Drop for SerialLease {
    fn drop(&mut self) {
        // On a poisoned registry retain the claim rather than permit reuse.
        let _ = self.discovery.registry.release(self.id);
    }
}

/// Canonicalize live aliases, caching this value in the lease for later unplug.
/// Normalize Windows COM spelling even in cross-platform fixtures.
pub fn canonical_serial_path(path: &Path) -> PathBuf {
    if let Some(raw) = path.to_str() {
        let upper = raw
            .strip_prefix(r"\\.\")
            .unwrap_or(raw)
            .to_ascii_uppercase();
        if upper
            .strip_prefix("COM")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            return PathBuf::from(upper);
        }
    }
    match std::fs::canonicalize(path) {
        Ok(path) => path,
        Err(_) => path.to_path_buf(),
    }
}

pub(super) struct InventoryRegistry {
    inventory: watch::Sender<SerialInventory>,
    claims: Mutex<BTreeMap<SerialLeaseId, Claim>>,
    count: watch::Sender<usize>,
}

impl InventoryRegistry {
    pub fn new(inventory: watch::Sender<SerialInventory>) -> Self {
        Self {
            inventory,
            claims: Mutex::new(BTreeMap::new()),
            count: watch::channel(0).0,
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, BTreeMap<SerialLeaseId, Claim>>, SerialDiscoveryError> {
        self.claims
            .lock()
            .map_err(|_| SerialDiscoveryError::WorkerGone)
    }

    pub fn reserve(
        self: &Arc<Self>,
        request: LeaseRequest,
        canonical: PathBuf,
        discovery: SerialDiscoveryHandle,
    ) -> Result<SerialLease, SerialDiscoveryError> {
        let mut claims = self.lock()?;
        self.check_selection(&request.expected)?;
        let stable_id = stable_usb_id(&request.expected.device);
        if let Some(claim) = claims.values().find(|claim| {
            claim.canonical == canonical
                || claim.port.instance_id == request.expected.instance_id
                || claim
                    .port
                    .device
                    .port_name
                    .eq_ignore_ascii_case(&request.expected.device.port_name)
                || stable_id
                    .as_ref()
                    .is_some_and(|id| claim.stable_id.as_ref() == Some(id))
        }) {
            return Err(SerialDiscoveryError::PortBusy {
                owner: Some(claim.status.owner),
            });
        }
        let id = SerialLeaseId(
            NEXT_LEASE
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .map_err(|_| SerialDiscoveryError::IdExhausted)?,
        );
        let revision = next_revision()?;
        let status = SerialLeaseStatus {
            id,
            owner: request.owner,
            phase: LeasePhase::Reserved,
        };
        claims.insert(
            id,
            Claim {
                port: request.expected.clone(),
                canonical: canonical.clone(),
                stable_id,
                status,
            },
        );
        self.publish_claims(&claims, revision);
        self.count.send_replace(claims.len());
        Ok(SerialLease {
            id,
            owner: request.owner,
            port: request.expected,
            canonical,
            discovery,
        })
    }

    fn check_selection(&self, expected: &SerialPortRef) -> Result<(), SerialDiscoveryError> {
        let inventory = self.inventory.borrow();
        if inventory.health != InventoryHealth::Ready {
            return Err(SerialDiscoveryError::InventoryUnavailable);
        }
        match inventory
            .ports
            .iter()
            .find(|p| p.port.instance_id == expected.instance_id)
        {
            Some(port) if &port.port == expected => Ok(()),
            Some(_) => Err(SerialDiscoveryError::DeviceChanged {
                instance: expected.instance_id,
            }),
            None if inventory
                .ports
                .iter()
                .any(|p| p.port.device.port_name == expected.device.port_name) =>
            {
                Err(SerialDiscoveryError::DeviceChanged {
                    instance: expected.instance_id,
                })
            }
            None => Err(SerialDiscoveryError::DeviceGone {
                instance: expected.instance_id,
            }),
        }
    }

    fn validate(
        &self,
        id: SerialLeaseId,
        expected: &SerialPortRef,
    ) -> Result<(), SerialDiscoveryError> {
        let claims = self.lock()?;
        if !claims.contains_key(&id) {
            return Err(SerialDiscoveryError::StaleAttempt);
        }
        self.check_selection(expected)
    }

    fn set_phase(&self, id: SerialLeaseId, phase: LeasePhase) -> Result<(), SerialDiscoveryError> {
        let mut claims = self.lock()?;
        let claim = claims
            .get_mut(&id)
            .ok_or(SerialDiscoveryError::StaleAttempt)?;
        if claim.status.phase != phase {
            let revision = next_revision()?;
            claim.status.phase = phase;
            self.publish_claims(&claims, revision);
        }
        Ok(())
    }

    fn release(&self, id: SerialLeaseId) -> Result<(), SerialDiscoveryError> {
        let mut claims = self.lock()?;
        if claims.contains_key(&id) {
            let revision = next_revision()?;
            claims.remove(&id);
            self.publish_claims(&claims, revision);
            self.count.send_replace(claims.len());
        }
        Ok(())
    }

    fn apply_claims(ports: &mut [SerialPortSnapshot], claims: &BTreeMap<SerialLeaseId, Claim>) {
        for port in ports {
            port.lease = claims
                .values()
                .find(|claim| {
                    claim.port.instance_id == port.port.instance_id
                        || claim
                            .port
                            .device
                            .port_name
                            .eq_ignore_ascii_case(&port.port.device.port_name)
                        || port
                            .stable_id
                            .as_ref()
                            .is_some_and(|id| claim.stable_id.as_ref() == Some(id))
                })
                .map(|claim| claim.status.clone());
        }
    }

    fn publish_claims(
        &self,
        claims: &BTreeMap<SerialLeaseId, Claim>,
        revision: super::InventoryRevision,
    ) {
        let mut snapshot = self.inventory.borrow().clone();
        snapshot.revision = revision;
        Self::apply_claims(&mut snapshot.ports, claims);
        self.inventory.send_replace(snapshot);
    }

    pub fn publish(
        &self,
        health: InventoryHealth,
        ports: Option<Vec<SerialPortSnapshot>>,
    ) -> Result<SerialInventory, SerialDiscoveryError> {
        let claims = self.lock()?;
        let mut snapshot = self.inventory.borrow().clone();
        let mut ports = ports.unwrap_or_else(|| snapshot.ports.clone());
        Self::apply_claims(&mut ports, &claims);
        if snapshot.health != health || snapshot.ports != ports {
            snapshot.revision = next_revision()?;
            snapshot.health = health;
            snapshot.ports = ports;
            self.inventory.send_replace(snapshot.clone());
        }
        Ok(snapshot)
    }

    pub async fn wait_until_released(&self) -> Result<(), SerialDiscoveryError> {
        let mut count = self.count.subscribe();
        let result = count.wait_for(|count| *count == 0).await;
        result
            .map(|_| ())
            .map_err(|_| SerialDiscoveryError::WorkerGone)
    }
}

#[cfg(test)]
mod tests;
