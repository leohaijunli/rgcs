//! Inbound message routing (issue #20).
//!
//! The connection worker broadcasts every [`ConnectionEvent`] to all
//! subscribers. Without a shared predicate each consumer — the telemetry hub,
//! the mission/command services, and the future RTK/parameter/log services —
//! re-implements "is this message for me?" and inspects the whole bus.
//!
//! [`MessageRoute`] centralizes that predicate and [`RoutedEvents`] applies it
//! while still forwarding lifecycle events (`Connected`, heartbeat changes,
//! `LinkError`, `Failed`) unchanged, so subscribers keep tracking the link but
//! only wake for message traffic they declared interest in.

use mavlink::Message;
use tokio::sync::broadcast;

use super::connection::ConnectionEvent;
use super::message::MessageEnvelope;

/// Predicate selecting which inbound messages a subscriber wants.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageRoute {
    /// MAVLink message ids of interest (empty = every message type).
    message_ids: Vec<u32>,
    /// Source system id (None = any).
    system_id: Option<u8>,
    /// Source component id (None = any).
    component_id: Option<u8>,
}

impl MessageRoute {
    /// Accept every inbound message (any source, any id).
    pub fn all() -> Self {
        Self::default()
    }

    /// Accept only the given MAVLink message ids, from any source.
    pub fn messages(message_ids: &[u32]) -> Self {
        Self {
            message_ids: message_ids.to_vec(),
            ..Self::default()
        }
    }

    /// Accept every message from one MAVLink node.
    pub fn from_node(system_id: u8, component_id: u8) -> Self {
        Self {
            system_id: Some(system_id),
            component_id: Some(component_id),
            ..Self::default()
        }
    }

    /// Chainable: restrict to the given message ids.
    pub fn only(mut self, message_ids: &[u32]) -> Self {
        self.message_ids = message_ids.to_vec();
        self
    }

    /// Chainable: restrict to one source node.
    pub fn from(mut self, system_id: u8, component_id: u8) -> Self {
        self.system_id = Some(system_id);
        self.component_id = Some(component_id);
        self
    }

    /// True when `env` matches this route.
    pub fn matches(&self, env: &MessageEnvelope) -> bool {
        if let Some(sys) = self.system_id {
            if env.header.system_id != sys {
                return false;
            }
        }
        if let Some(comp) = self.component_id {
            if env.header.component_id != comp {
                return false;
            }
        }
        self.message_ids.is_empty() || self.message_ids.contains(&env.message.message_id())
    }
}

/// A connection event stream filtered by a [`MessageRoute`].
///
/// Non-matching `Message` events are skipped; every lifecycle event
/// (`Connected`, `HeartbeatLost`/`HeartbeatRestored`, `LinkError`, `Failed`)
/// is delivered. Bus lag is surfaced as [`RecvError::Lagged`] so a subscriber
/// can keep the authoritative dropped-frame counter (issues.md #17).
pub struct RoutedEvents {
    events: broadcast::Receiver<ConnectionEvent>,
    route: MessageRoute,
}

impl RoutedEvents {
    /// Wrap an event receiver with a routing predicate.
    pub fn new(events: broadcast::Receiver<ConnectionEvent>, route: MessageRoute) -> Self {
        Self { events, route }
    }

    /// The route applied to this stream.
    pub fn route(&self) -> &MessageRoute {
        &self.route
    }

    /// Await the next event of interest.
    ///
    /// Returns `Err(RecvError::Lagged(n))` when the shared bus overflowed and
    /// `Err(RecvError::Closed)` when the connection worker has gone.
    pub async fn recv(&mut self) -> Result<ConnectionEvent, broadcast::error::RecvError> {
        loop {
            match self.events.recv().await {
                Ok(ConnectionEvent::Message(env)) if !self.route.matches(&env) => continue,
                other => return other,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::MavHeader;
    use ::mavlink::common::{MavMessage, HEARTBEAT_DATA};
    use ::mavlink::MessageData;

    fn envelope(system_id: u8, component_id: u8, message: MavMessage) -> MessageEnvelope {
        MessageEnvelope {
            header: MavHeader {
                system_id,
                component_id,
                sequence: 0,
            },
            message,
            received_at: std::time::Instant::now(),
        }
    }

    fn heartbeat() -> MavMessage {
        MavMessage::HEARTBEAT(HEARTBEAT_DATA {
            custom_mode: 0,
            mavtype: ::mavlink::common::MavType::MAV_TYPE_QUADROTOR,
            autopilot: ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_PX4,
            base_mode: ::mavlink::common::MavModeFlag::empty(),
            system_status: ::mavlink::common::MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        })
    }

    #[test]
    fn all_route_accepts_everything() {
        let route = MessageRoute::all();
        assert!(route.matches(&envelope(1, 1, heartbeat())));
        assert!(route.matches(&envelope(255, 190, heartbeat())));
    }

    #[test]
    fn node_route_filters_by_source() {
        let route = MessageRoute::from_node(1, 1);
        assert!(route.matches(&envelope(1, 1, heartbeat())));
        assert!(!route.matches(&envelope(1, 2, heartbeat())));
        assert!(!route.matches(&envelope(2, 1, heartbeat())));
    }

    #[test]
    fn message_id_route_filters_by_type() {
        let route = MessageRoute::messages(&[HEARTBEAT_DATA::ID]);
        assert!(route.matches(&envelope(9, 9, heartbeat())));
        assert!(!route.matches(&envelope(
            9,
            9,
            MavMessage::COMMAND_ACK(::mavlink::common::COMMAND_ACK_DATA {
                command: ::mavlink::common::MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH,
                result: ::mavlink::common::MavResult::MAV_RESULT_ACCEPTED,
            })
        )));
    }

    #[test]
    fn routes_compose() {
        let route = MessageRoute::messages(&[HEARTBEAT_DATA::ID]).from(1, 1);
        assert!(route.matches(&envelope(1, 1, heartbeat())));
        assert!(!route.matches(&envelope(2, 1, heartbeat())));
    }
}
