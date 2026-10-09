//! Process-wide state: active vehicle links and the latest link status.
//!
//! Links are keyed by [`LinkId`] and stored in a map with a `primary` pointer
//! so adding a second vehicle later is an insert rather than a rewrite
//! (issues.md #21). `connect`/`disconnect` serialize on [`AppState::ops`] so
//! two concurrent commands cannot race the swap.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use maggcs_core::mavlink::connection::LinkStatus;
use maggcs_core::mavlink::ConnectionHandle;
use maggcs_core::mission::service::MissionService;
use maggcs_core::telemetry::hub::TelemetryHub;
use maggcs_core::CommandService;
use parking_lot::Mutex;

use crate::inspector_service::InspectorState;

/// Stable identifier for one connection/vehicle session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkId(u64);

impl LinkId {
    /// Allocate the next unused link id (monotonic, process-wide).
    pub fn next() -> Self {
        Self(NEXT_LINK_ID.fetch_add(1, Ordering::Relaxed))
    }
}

static NEXT_LINK_ID: AtomicU64 = AtomicU64::new(1);

/// Everything bound to a single connection.
pub struct ActiveLink {
    /// Stable id, also the map key.
    pub id: LinkId,
    /// The connection worker handle.
    pub connection: ConnectionHandle,
    /// Telemetry aggregation hub for this link.
    pub hub: TelemetryHub,
    /// Mission service for this link.
    pub mission: MissionService,
    /// Command service for this link.
    pub command: CommandService,
}

#[derive(Default)]
struct AppInner {
    links: BTreeMap<LinkId, ActiveLink>,
    primary: Option<LinkId>,
    link: Option<LinkStatus>,
}

/// Shared application state managed by Tauri.
#[derive(Default)]
pub struct AppState {
    inner: Mutex<AppInner>,
    /// Signal Inspector tap/catalog/batcher (survives link swaps, ADR-016).
    pub inspector: InspectorState,
    /// Held for the whole duration of a `connect`/`disconnect` command so the
    /// link swap is atomic with respect to other commands (issues.md #21).
    ops: tokio::sync::Mutex<()>,
}

impl AppState {
    /// Serialization lock for connection lifecycle commands.
    pub fn ops(&self) -> &tokio::sync::Mutex<()> {
        &self.ops
    }

    fn with_primary<T>(&self, f: impl FnOnce(&ActiveLink) -> T) -> Option<T> {
        let inner = self.inner.lock();
        let id = inner.primary?;
        inner.links.get(&id).map(f)
    }

    /// Insert `link`, make it primary, and return the previous primary.
    pub fn set_primary_link(&self, link: ActiveLink) -> Option<ActiveLink> {
        let mut inner = self.inner.lock();
        let old = inner.primary.and_then(|id| inner.links.remove(&id));
        inner.primary = Some(link.id);
        inner.links.insert(link.id, link);
        old
    }

    /// Remove and return the primary link, if any.
    pub fn take_primary_link(&self) -> Option<ActiveLink> {
        let mut inner = self.inner.lock();
        let id = inner.primary.take()?;
        inner.links.remove(&id)
    }

    /// Number of active links (multi-vehicle readiness).
    #[cfg(test)]
    pub fn link_count(&self) -> usize {
        self.inner.lock().links.len()
    }

    /// Cloned hub of the primary link, if any.
    pub fn hub(&self) -> Option<TelemetryHub> {
        self.with_primary(|l| l.hub.clone())
    }

    /// Cloned mission service of the primary link, if any.
    pub fn mission(&self) -> Option<MissionService> {
        self.with_primary(|l| l.mission.clone())
    }

    /// Cloned command service of the primary link, if any.
    pub fn command(&self) -> Option<CommandService> {
        self.with_primary(|l| l.command.clone())
    }

    /// The connection handle of the primary link, if any.
    pub fn connection(&self) -> Option<ConnectionHandle> {
        self.with_primary(|l| l.connection.clone())
    }

    /// Latest link status published by the pump.
    pub fn link_status(&self) -> Option<LinkStatus> {
        self.inner.lock().link.clone()
    }

    /// Publish a new link status.
    pub fn publish_link(&self, status: LinkStatus) {
        self.inner.lock().link = Some(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_ids_are_unique_and_monotonic() {
        let a = LinkId::next();
        let b = LinkId::next();
        assert_ne!(a, b);
        assert!(a < b);
    }

    #[test]
    fn link_status_round_trips() {
        let state = AppState::default();
        assert!(state.link_status().is_none());
        assert!(state.take_primary_link().is_none());
        assert_eq!(state.link_count(), 0);

        state.publish_link(LinkStatus::disconnected());
        assert!(state.link_status().is_some());
    }
}
