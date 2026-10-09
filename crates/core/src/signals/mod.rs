//! Signal collection for the Signal Inspector (plan §4, ADR-015/016).
//!
//! The inspector cannot reuse the 20 Hz telemetry snapshot: it needs arbitrary
//! message fields at their native rate for FFT work. This module taps the raw
//! inbound MAVLink stream (`MessageRoute::all()`) and turns every message into
//! a set of numeric samples:
//!
//! - [`SignalId`] identifies one numeric field of one message type from one
//!   node (system/component ids reserve multi-vehicle use).
//! - [`extract::extract_fields`] walks a message with a custom `serde`
//!   serializer, so a new message type needs no hand-written mapping.
//! - [`catalog::SignalCatalog`] tracks the last value and arrival rate of every
//!   known `(msg, field)`.
//! - [`SampleSource`] is the source abstraction (plan decision B): live MAVLink
//!   now, a ULog replay later.
//!
//! The tap task itself (`tap::SignalTap`, P2c) subscribes to a connection route
//! with a reference count, so no work happens while the inspector is closed.

pub mod catalog;
pub mod extract;
pub mod tap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Identifies one numeric signal: a field of a message type from one node.
///
/// Array fields are expanded to one signal per element, `field[i]`; messages
/// with a string `name` field (`NAMED_VALUE_FLOAT`, `DEBUG_*`) use the name as
/// the field (`NAMED_VALUE_FLOAT/<name>`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SignalId {
    pub system_id: u8,
    pub component_id: u8,
    /// MAVLink message id.
    pub message_id: u32,
    /// Field path within the message, e.g. `"roll"` or `"accel[0]"`.
    pub field: String,
}

impl SignalId {
    pub fn new(system_id: u8, component_id: u8, message_id: u32, field: impl Into<String>) -> Self {
        Self {
            system_id,
            component_id,
            message_id,
            field: field.into(),
        }
    }
}

/// One collected numeric sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SignalSample {
    pub id: SignalId,
    /// Host-aligned time in milliseconds (see `tap::TimestampPolicy`).
    pub t_ms: f64,
    pub value: f64,
}

/// A source of samples. Live MAVLink is the first implementation; ULog replay
/// is the planned second (plan decision B).
pub trait SampleSource: Send {
    /// The current catalog of known `(msg, field)` entries.
    fn catalog(&self) -> catalog::SignalCatalog;
}
