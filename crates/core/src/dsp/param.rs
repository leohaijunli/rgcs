//! Parameter descriptions and values for DSP algorithms.
//!
//! `ParamSpec` is serialized to the frontend so it can render a parameter form
//! without knowing the algorithm; `ParamValues` is the runtime map a processor
//! reads its settings from (the frontend edits it over IPC).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::DspError;

/// How a parameter is edited and validated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamKind {
    /// A continuous value; `log` switches the slider to a log scale.
    Float {
        min: f64,
        max: f64,
        step: f64,
        log: bool,
    },
    /// A whole-number value.
    Int { min: i64, max: i64 },
    /// One option from a closed list (the value is the option index).
    Enum { options: Vec<String> },
    /// A boolean toggle (1/0).
    Bool,
}

/// One editable parameter of an algorithm, as sent to the frontend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ParamSpec {
    pub key: String,
    pub label: String,
    pub unit: Option<String>,
    pub kind: ParamKind,
    pub default: f64,
}

impl ParamSpec {
    /// A continuous float parameter.
    pub fn float(key: &str, label: &str, unit: &str, min: f64, max: f64, default: f64) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            unit: (!unit.is_empty()).then(|| unit.into()),
            kind: ParamKind::Float {
                min,
                max,
                step: (max - min) / 100.0,
                log: false,
            },
            default,
        }
    }

    /// A whole-number parameter.
    pub fn int(key: &str, label: &str, unit: &str, min: i64, max: i64, default: i64) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            unit: (!unit.is_empty()).then(|| unit.into()),
            kind: ParamKind::Int { min, max },
            default: default as f64,
        }
    }

    /// A boolean toggle.
    pub fn boolean(key: &str, label: &str, default: bool) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            unit: None,
            kind: ParamKind::Bool,
            default: default as u8 as f64,
        }
    }
}

/// Runtime values a processor reads its settings from.
#[derive(Debug, Clone, Default)]
pub struct ParamValues {
    map: HashMap<String, f64>,
}

impl ParamValues {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a parameter by key.
    pub fn set(&mut self, key: &str, value: f64) {
        self.map.insert(key.into(), value);
    }

    /// Set many parameters at once.
    pub fn set_all(&mut self, pairs: &[(&str, f64)]) {
        for (k, v) in pairs {
            self.map.insert((*k).into(), *v);
        }
    }

    /// Get a parameter value.
    pub fn get(&self, key: &str) -> Option<f64> {
        self.map.get(key).copied()
    }

    /// Get a parameter as a boolean (nonzero).
    pub fn boolean(&self, key: &str) -> bool {
        self.get(key).is_some_and(|v| v != 0.0)
    }

    /// Get a finite parameter value, or an [`DspError::InvalidParam`].
    pub fn finite(&self, key: &str) -> Result<f64, DspError> {
        match self.get(key) {
            Some(v) if v.is_finite() => Ok(v),
            Some(v) => Err(DspError::InvalidParam(format!("{key} is not finite: {v}"))),
            None => Err(DspError::InvalidParam(format!("{key} is not set"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_values_default_to_constructor_specs() {
        let mut p = ParamValues::new();
        p.set("fc", 5.0);
        assert_eq!(p.get("fc"), Some(5.0));
        assert_eq!(p.finite("fc"), Ok(5.0));
        assert!(!p.boolean("missing"));
        p.set("on", 1.0);
        assert!(p.boolean("on"));
        p.set("nan", f64::NAN);
        assert!(matches!(p.finite("nan"), Err(DspError::InvalidParam(_))));
    }
}
