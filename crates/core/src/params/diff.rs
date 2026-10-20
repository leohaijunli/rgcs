//! Compare two parameter sets (parameters plan §4.1): FC vs snapshot,
//! snapshot vs snapshot, FC vs import file. Only differences are reported.

use std::collections::HashMap;

use super::model::ParamEntry;

/// Which side holds the differing value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ParamDiffKind {
    /// Present in A only.
    OnlyInA,
    /// Present in B only.
    OnlyInB,
    /// Present in both with different values.
    Changed,
}

/// One differing parameter.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ParamDiff {
    pub name: String,
    pub kind: ParamDiffKind,
    /// Value in set A (if present).
    pub a: Option<ParamEntryLite>,
    /// Value in set B (if present).
    pub b: Option<ParamEntryLite>,
}

/// Value/type only — enough for the diff table and staging.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ParamEntryLite {
    pub value: f32,
    pub ptype: super::model::ParamType,
}

/// Compute the diff of two parameter sets keyed by name. Order: by name.
pub fn diff_param_sets(a: &[ParamEntry], b: &[ParamEntry]) -> Vec<ParamDiff> {
    let ma: HashMap<&str, &ParamEntry> = a.iter().map(|e| (e.name.as_str(), e)).collect();
    let mb: HashMap<&str, &ParamEntry> = b.iter().map(|e| (e.name.as_str(), e)).collect();

    let mut names: Vec<&str> = ma
        .keys()
        .chain(mb.keys())
        .copied()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    names.sort_unstable();

    let mut out = Vec::new();
    for name in names {
        let ea = ma.get(name);
        let eb = mb.get(name);
        let to_lite = |e: &&ParamEntry| ParamEntryLite {
            value: e.value_real_wire(),
            ptype: e.ptype,
        };
        match (ea, eb) {
            (Some(x), Some(y)) => {
                if !x.value.same_as(y.value) || x.ptype != y.ptype {
                    out.push(ParamDiff {
                        name: name.to_string(),
                        kind: ParamDiffKind::Changed,
                        a: Some(to_lite(x)),
                        b: Some(to_lite(y)),
                    });
                }
            }
            (Some(x), None) => out.push(ParamDiff {
                name: name.to_string(),
                kind: ParamDiffKind::OnlyInA,
                a: Some(to_lite(x)),
                b: None,
            }),
            (None, Some(y)) => out.push(ParamDiff {
                name: name.to_string(),
                kind: ParamDiffKind::OnlyInB,
                a: None,
                b: Some(to_lite(y)),
            }),
            (None, None) => unreachable!("name came from one of the maps"),
        }
    }
    out
}

impl ParamEntry {
    /// The wire `f32` of this entry's value (diff/file tables carry wire
    /// floats so the UI can apply them straight back).
    fn value_real_wire(&self) -> f32 {
        super::codec::encode(self.value, IntEncoding::default())
    }
}

use super::codec::IntEncoding;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::model::{ParamType, ParamValue};

    fn e(name: &str, index: u16, value: ParamValue) -> ParamEntry {
        ParamEntry {
            name: name.into(),
            index,
            value,
            ptype: ParamType::I32,
        }
    }

    #[test]
    fn diff_reports_changed_only_and_missing() {
        let a = vec![
            e("A_SAME", 0, ParamValue::Int(1)),
            e("B_CHANGED", 1, ParamValue::Int(2)),
            e("C_ONLY_A", 2, ParamValue::Int(3)),
        ];
        let b = vec![
            e("A_SAME", 0, ParamValue::Int(1)),
            e("B_CHANGED", 1, ParamValue::Int(20)),
            e("D_ONLY_B", 2, ParamValue::Int(4)),
        ];
        let d = diff_param_sets(&a, &b);
        assert_eq!(d.len(), 3, "only differences appear");
        assert!(d
            .iter()
            .any(|x| x.name == "B_CHANGED" && x.kind == ParamDiffKind::Changed));
        assert!(d
            .iter()
            .any(|x| x.name == "C_ONLY_A" && x.kind == ParamDiffKind::OnlyInA));
        assert!(d
            .iter()
            .any(|x| x.name == "D_ONLY_B" && x.kind == ParamDiffKind::OnlyInB));
        // Sorted by name.
        let names: Vec<&str> = d.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["B_CHANGED", "C_ONLY_A", "D_ONLY_B"]);
    }

    #[test]
    fn one_ulp_real_difference_is_a_change() {
        let a = vec![e("REAL", 0, ParamValue::Real(1.0))];
        let b = vec![e("REAL", 0, ParamValue::Real(1.0 + f32::EPSILON))];
        assert_eq!(diff_param_sets(&a, &b).len(), 1);
    }

    #[test]
    fn type_change_without_value_change_is_reported() {
        let mut a = e("T", 0, ParamValue::Int(1));
        let mut b = e("T", 0, ParamValue::Int(1));
        a.ptype = ParamType::I32;
        b.ptype = ParamType::I16;
        assert_eq!(diff_param_sets(&[a], &[b]).len(), 1);
    }

    #[test]
    fn identical_sets_diff_empty() {
        let a = vec![
            e("X", 0, ParamValue::Int(9)),
            e("Y", 1, ParamValue::Real(-1.5)),
        ];
        let b = a.clone();
        assert!(diff_param_sets(&a, &b).is_empty());
    }
}
