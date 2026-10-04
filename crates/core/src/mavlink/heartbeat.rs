//! Heartbeat monitoring for a single MAVLink node (system/component pair).
//!
//! The connection worker feeds received HEARTBEAT frames into a monitor and
//! emits transition events (`Alive` → `Lost`, `Lost` → `Alive`).

use std::time::{Duration, Instant};

/// Tracks heartbeat state for one system/component pair.
#[derive(Debug, Clone)]
pub struct HeartbeatMonitor {
    sys_id: u8,
    comp_id: u8,
    timeout: Duration,
    last_seen: Option<Instant>,
    lost_since: Option<Instant>,
    /// Number of consecutive heartbeat frames observed.
    consecutive: u64,
}

/// Heartbeat status snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeartbeatStatus {
    /// No heartbeat observed yet.
    NeverSeen,
    /// Heartbeats arriving within the timeout.
    Alive {
        /// Milliseconds since the last heartbeat frame.
        last_seen_ms_ago: u64,
        consecutive: u64,
    },
    /// Heartbeats stopped arriving.
    Lost {
        /// Milliseconds since the link was declared lost.
        since_ms_ago: u64,
        consecutive: u64,
    },
}

impl HeartbeatMonitor {
    /// Create a monitor for the given node with a timeout.
    pub fn new(sys_id: u8, comp_id: u8, timeout: Duration) -> Self {
        Self {
            sys_id,
            comp_id,
            timeout,
            last_seen: None,
            lost_since: None,
            consecutive: 0,
        }
    }

    /// Node this monitor tracks.
    pub fn node(&self) -> (u8, u8) {
        (self.sys_id, self.comp_id)
    }

    /// Heartbeat timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Change the heartbeat timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Feed a heartbeat frame from `sys_id`/`comp_id`.
    ///
    /// Returns `true` when the status transitioned
    /// (`NeverSeen` → `Alive`, `Alive` → `Lost`, `Lost` → `Alive`).
    pub fn observe(&mut self, sys_id: u8, comp_id: u8, now: Instant) -> bool {
        if sys_id != self.sys_id || comp_id != self.comp_id {
            return false;
        }
        let was_lost = self.is_lost(now);
        self.last_seen = Some(now);
        self.lost_since = None;
        self.consecutive = self.consecutive.saturating_add(1);
        was_lost
    }

    /// True if a heartbeat was seen within the timeout.
    pub fn is_alive(&self, now: Instant) -> bool {
        match self.last_seen {
            Some(last) => now.duration_since(last) <= self.timeout,
            None => false,
        }
    }

    /// True if the link is currently declared lost (or never seen).
    pub fn is_lost(&self, now: Instant) -> bool {
        !self.is_alive(now)
    }

    /// Snapshot of the current status.
    pub fn status(&self, now: Instant) -> HeartbeatStatus {
        match self.last_seen {
            None => HeartbeatStatus::NeverSeen,
            Some(last) => {
                if now.duration_since(last) <= self.timeout {
                    HeartbeatStatus::Alive {
                        last_seen_ms_ago: now.duration_since(last).as_millis() as u64,
                        consecutive: self.consecutive,
                    }
                } else {
                    HeartbeatStatus::Lost {
                        since_ms_ago: self.lost_since.unwrap_or(last).elapsed().as_millis() as u64,
                        consecutive: self.consecutive,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn never_seen_until_heartbeat() {
        let mut m = HeartbeatMonitor::new(1, 1, secs(3));
        let t0 = Instant::now();
        assert_eq!(m.status(t0), HeartbeatStatus::NeverSeen);
        assert!(m.observe(1, 1, t0));
        assert!(matches!(m.status(t0), HeartbeatStatus::Alive { .. }));
    }

    #[test]
    fn observes_other_nodes_only() {
        let mut m = HeartbeatMonitor::new(1, 1, secs(3));
        assert!(!m.observe(2, 1, Instant::now()));
        assert!(!m.observe(1, 2, Instant::now()));
        assert_eq!(m.status(Instant::now()), HeartbeatStatus::NeverSeen);
    }

    #[test]
    fn alive_then_lost_then_restored() {
        let mut m = HeartbeatMonitor::new(1, 1, secs(3));
        let t0 = Instant::now();
        m.observe(1, 1, t0);
        assert!(m.is_alive(t0));

        let t1 = t0 + secs(2);
        m.observe(1, 1, t1);
        assert!(m.is_alive(t1));

        // Heartbeats stop; 3s timeout.
        let t2 = t1 + secs(4);
        assert!(!m.is_alive(t2));
        assert!(matches!(m.status(t2), HeartbeatStatus::Lost { .. }));

        // Restored.
        let t3 = t2 + secs(1);
        assert!(m.observe(1, 1, t3), "lost->alive must transition");
        assert!(m.is_alive(t3));
    }

    #[test]
    fn timeout_is_configurable() {
        let mut m = HeartbeatMonitor::new(1, 1, secs(3));
        let t0 = Instant::now();
        m.observe(1, 1, t0);
        m.set_timeout(secs(10));
        assert!(m.is_alive(t0 + secs(5)));
    }
}
