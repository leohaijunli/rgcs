//! FC parameter types and values (parameters plan §4.1).
//!
//! Integer parameters are kept exact (`i64`), floats carry the wire `f32`
//! and compare by bits — a `PARAM_SET` echo that differs in one ULP is a
//! different value, not a rounding artefact.

use serde::{Deserialize, Serialize};

/// MAVLink `MAV_PARAM_TYPE` (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ParamType {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    Real32,
}

impl ParamType {
    pub fn from_mav(t: ::mavlink::common::MavParamType) -> Option<Self> {
        use ::mavlink::common::MavParamType as M;
        Some(match t {
            M::MAV_PARAM_TYPE_UINT8 => Self::U8,
            M::MAV_PARAM_TYPE_INT8 => Self::I8,
            M::MAV_PARAM_TYPE_UINT16 => Self::U16,
            M::MAV_PARAM_TYPE_INT16 => Self::I16,
            M::MAV_PARAM_TYPE_UINT32 => Self::U32,
            M::MAV_PARAM_TYPE_INT32 => Self::I32,
            M::MAV_PARAM_TYPE_REAL32 => Self::Real32,
            _ => return None,
        })
    }

    pub fn to_mav(self) -> ::mavlink::common::MavParamType {
        use ::mavlink::common::MavParamType as M;
        match self {
            Self::U8 => M::MAV_PARAM_TYPE_UINT8,
            Self::I8 => M::MAV_PARAM_TYPE_INT8,
            Self::U16 => M::MAV_PARAM_TYPE_UINT16,
            Self::I16 => M::MAV_PARAM_TYPE_INT16,
            Self::U32 => M::MAV_PARAM_TYPE_UINT32,
            Self::I32 => M::MAV_PARAM_TYPE_INT32,
            Self::Real32 => M::MAV_PARAM_TYPE_REAL32,
        }
    }

    /// Integer payload width in bytes; `None` for `Real32`.
    pub fn int_width(self) -> Option<u32> {
        Some(match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 => 4,
            Self::Real32 => return None,
        })
    }
}

/// One parameter value: integers exact, floats bit-exact.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ParamValue {
    Int(i64),
    Real(f32),
}

impl ParamValue {
    /// Bit-exact equality for floats (§4.1: 浮点按位比较).
    pub fn same_as(self, other: Self) -> bool {
        match (self, other) {
            (ParamValue::Int(a), ParamValue::Int(b)) => a == b,
            (ParamValue::Real(a), ParamValue::Real(b)) => a.to_bits() == b.to_bits(),
            _ => false,
        }
    }

    /// The value as a wire `f32` (integers keep their bit pattern via the
    /// chosen codec; here we only render display/JSON purposes).
    pub fn display(&self) -> String {
        match self {
            ParamValue::Int(v) => v.to_string(),
            ParamValue::Real(v) => {
                // Render like PX4: shortest form that round-trips f32.
                let r = format!("{}", v);
                r
            }
        }
    }
}

/// One onboard parameter as the UI and cache store it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ParamEntry {
    pub name: String,
    pub index: u16,
    pub value: ParamValue,
    pub ptype: ParamType,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_equality_is_bitwise() {
        let a = ParamValue::Real(1.0_f32);
        let b = ParamValue::Real(1.0_f32);
        let c = ParamValue::Real(1.0_f32 + f32::EPSILON);
        assert!(a.same_as(b));
        assert!(!a.same_as(c), "one ULP apart is a different value");
        // Int vs Real never compare equal even if numerically close.
        assert!(!ParamValue::Int(1).same_as(ParamValue::Real(1.0)));
    }

    #[test]
    fn mav_param_type_round_trips() {
        use ::mavlink::common::MavParamType as M;
        for t in [
            ParamType::U8,
            ParamType::I8,
            ParamType::U16,
            ParamType::I16,
            ParamType::U32,
            ParamType::I32,
            ParamType::Real32,
        ] {
            assert_eq!(ParamType::from_mav(t.to_mav()), Some(t));
        }
        assert_eq!(ParamType::from_mav(M::MAV_PARAM_TYPE_UINT64), None);
        assert_eq!(ParamType::U16.int_width(), Some(2));
        assert_eq!(ParamType::Real32.int_width(), None);
    }
}
