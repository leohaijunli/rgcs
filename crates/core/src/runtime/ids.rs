//! Lossless Runtime identity types. Opaque session IDs are not MAVLink addresses
//! or permission tokens. Numeric identity allocation never wraps.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::RuntimeError;

const MAX_OPAQUE_ID_BYTES: usize = 128;
const RANDOM_ID_BYTES: usize = 16;
static NEXT_LINK_ID: AtomicU64 = AtomicU64::new(1);

fn validate_token(kind: &str, value: &str) -> Result<(), RuntimeError> {
    if value.is_empty()
        || value.len() > MAX_OPAQUE_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
    {
        return Err(RuntimeError::InvalidIdentity {
            kind: kind.into(),
            reason: format!("expected 1–{MAX_OPAQUE_ID_BYTES} ASCII identifier bytes"),
        });
    }
    Ok(())
}

fn fresh_token() -> Result<String, RuntimeError> {
    let mut bytes = [0; RANDOM_ID_BYTES];
    getrandom::fill(&mut bytes).map_err(|error| RuntimeError::IdentityUnavailable {
        message: error.to_string(),
    })?;
    Ok(bytes.map(|byte| format!("{byte:02x}")).concat())
}

macro_rules! opaque_id {
    ($name:ident, $typescript:literal, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, TS)]
        #[ts(export, type = "string", rename = $typescript)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, RuntimeError> {
                let value = value.into();
                validate_token(stringify!($name), &value)?;
                Ok(Self(value))
            }

            /// Runtime issuance, independent of process counters/OS clock.
            pub fn fresh() -> Result<Self, RuntimeError> {
                Self::new(fresh_token()?)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

opaque_id!(
    VehicleId,
    "RuntimeVehicleId",
    "Application logical aircraft identity, separate from telemetry routing."
);
opaque_id!(
    VehicleSessionId,
    "VehicleSessionId",
    "Ephemeral Runtime target/flight context, not a physical boot epoch."
);
opaque_id!(
    ClientId,
    "ClientId",
    "A Runtime client session; an echoed ID alone does not authenticate a client."
);
opaque_id!(
    ModuleInstanceId,
    "ModuleInstanceId",
    "One module instance, separate from its descriptor or window label."
);
opaque_id!(
    OperationId,
    "OperationId",
    "One owner-bound Runtime operation record."
);
opaque_id!(
    RequestId,
    "RequestId",
    "Request/result correlation identity, not automatic replay intent."
);
opaque_id!(
    SubscriptionId,
    "SubscriptionId",
    "One owner-bound Runtime data subscription."
);

macro_rules! counter_id {
    ($name:ident, $nonzero:literal, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TS)]
        #[ts(export, type = "string")]
        pub struct $name(u64);

        impl $name {
            pub fn new(value: u64) -> Result<Self, RuntimeError> {
                if $nonzero && value == 0 {
                    return Err(RuntimeError::InvalidIdentity {
                        kind: stringify!($name).into(),
                        reason: "zero is not an active identity".into(),
                    });
                }
                Ok(Self(value))
            }

            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                let value = text.parse::<u64>().map_err(serde::de::Error::custom)?;
                if text != value.to_string() {
                    return Err(serde::de::Error::custom(
                        "expected canonical decimal identity",
                    ));
                }
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

counter_id!(
    LinkId,
    true,
    "Identity of an existing native I/O-owner entry; transport generation is separate."
);
counter_id!(
    TransportGeneration,
    true,
    "Actual nonzero successful-open generation of an existing connection worker."
);
counter_id!(
    Revision,
    false,
    "Monotonic revision within an explicitly identified resource scope."
);

impl LinkId {
    /// Reuses the desktop's process-wide, monotonic identity allocation, but
    /// reports exhaustion rather than wrapping into an older link identity.
    pub fn next() -> Result<Self, RuntimeError> {
        next_link_id(&NEXT_LINK_ID)
    }
}

fn next_link_id(counter: &AtomicU64) -> Result<LinkId, RuntimeError> {
    let value = counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .map_err(|_| RuntimeError::IdentityExhausted {
            kind: "LinkId".into(),
        })?;
    LinkId::new(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_round_trip_above_javascript_integer_precision() {
        for value in [9_007_199_254_740_993, u64::MAX] {
            let link = LinkId::new(value).expect("link ID");
            let json = serde_json::to_string(&link).expect("serialize");
            assert_eq!(json, format!("\"{value}\""));
            assert_eq!(
                serde_json::from_str::<LinkId>(&json).expect("deserialize"),
                link
            );
            let generation = TransportGeneration::new(value).expect("generation");
            assert_eq!(
                serde_json::from_str::<TransportGeneration>(&json).expect("generation"),
                generation
            );
            assert_eq!(
                serde_json::from_str::<Revision>(&json)
                    .expect("revision")
                    .get(),
                value
            );
        }
    }

    #[test]
    fn counters_reject_lossy_numeric_and_noncanonical_json() {
        for input in [
            "0",
            "9007199254740993",
            "null",
            "\"0\"",
            "\"01\"",
            "\"+1\"",
            "\"-1\"",
            "\"1.0\"",
            "\" 1\"",
            "\"18446744073709551616\"",
        ] {
            assert!(
                serde_json::from_str::<LinkId>(input).is_err(),
                "accepted {input}"
            );
            assert!(
                serde_json::from_str::<TransportGeneration>(input).is_err(),
                "accepted {input}"
            );
        }
        assert_eq!(
            serde_json::from_str::<Revision>("\"0\"")
                .expect("initial revision")
                .get(),
            0
        );
    }

    #[test]
    fn allocation_is_monotonic_and_exhaustion_never_wraps() {
        let counter = AtomicU64::new(u64::MAX - 1);
        assert_eq!(
            next_link_id(&counter).expect("last allocatable link").get(),
            u64::MAX - 1
        );
        assert!(matches!(
            next_link_id(&counter),
            Err(RuntimeError::IdentityExhausted { .. })
        ));
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
        assert!(next_link_id(&counter).is_err());
    }

    #[test]
    fn opaque_ids_validate_deserialization_and_preserve_text() {
        let id = VehicleId::new("logical-aircraft:site_a.1").expect("ID");
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(
            serde_json::from_str::<VehicleId>(&json).expect("deserialize"),
            id
        );
        for value in ["", " a", "a b", "a/b", "aircraft🚁"] {
            assert!(VehicleId::new(value).is_err());
            assert!(serde_json::from_str::<VehicleSessionId>(
                &serde_json::to_string(value).expect("JSON")
            )
            .is_err());
        }
        assert!(VehicleId::new("x".repeat(MAX_OPAQUE_ID_BYTES + 1)).is_err());
        assert!(serde_json::from_str::<ClientId>("1").is_err());
    }

    #[test]
    fn fresh_sessions_do_not_reuse_a_process_counter_or_clock_identity() {
        let first = VehicleSessionId::fresh().expect("session");
        let second = VehicleSessionId::fresh().expect("session");
        assert_ne!(first, second);
        assert_eq!(first.as_str().len(), RANDOM_ID_BYTES * 2);
    }

    #[test]
    fn typescript_uses_strings_and_separates_logical_vehicle_from_route() {
        assert_eq!(LinkId::decl(), "type LinkId = string;");
        assert_eq!(
            TransportGeneration::decl(),
            "type TransportGeneration = string;"
        );
        assert_eq!(VehicleId::decl(), "type RuntimeVehicleId = string;");
    }
}
