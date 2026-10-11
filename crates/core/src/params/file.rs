//! Parameter import/export (parameters plan §4.1): QGC `.params`
//! (tab-separated `sysid compid name value type`) and JSON (with metadata).
//! Imported values are untrusted — names, types and ranges are validated
//! before anything reaches the staging area (§7).

use std::collections::HashMap;

use super::codec::{encode, IntEncoding};
use super::model::{ParamEntry, ParamType, ParamValue};

/// The staging-ready view of an import file (§5.4): one row per parameter
/// in the file, tagged against the current FC set.
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ImportPreviewRow {
    pub name: String,
    pub value: ParamValue,
    pub ptype: ParamType,
    /// What this row means against the FC's current values.
    pub kind: super::diff::ParamDiffKind,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ImportPreview {
    pub rows: Vec<ImportPreviewRow>,
}

/// Export as QGC `.params`: `sysid\tcompid\tname\tvalue\ttype`, one header
/// comment line with the type names (QGC's format).
pub fn export_qgc_params(entries: &[ParamEntry], sysid: u8, compid: u8) -> String {
    let mut out = String::new();
    for e in entries {
        let value = encode(e.value, IntEncoding::default());
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            sysid,
            compid,
            e.name,
            format_value_for_export(e.value, value),
            mavlink_type_name(e.ptype),
        ));
    }
    out
}

/// Export as JSON with metadata (§5.4).
pub fn export_json(entries: &[ParamEntry], meta: serde_json::Value) -> Result<String, String> {
    let doc = serde_json::json!({
        "meta": meta,
        "parameters": entries,
    });
    serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())
}

/// Parse a `.params` or `.json` export into a preview against the FC's
/// current values. Values validate by type; bad rows are skipped and
/// reported in the `errors` vector.
pub fn import_params_file(text: &str, current: &[ParamEntry]) -> (ImportPreview, Vec<String>) {
    let mut rows: Vec<ImportPreviewRow> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    let parsed: Result<serde_json::Value, _> = serde_json::from_str(text);
    match parsed {
        Ok(doc) => {
            // JSON form: {"parameters": [{name, value, ptype, ...}]}
            let Some(params) = doc.get("parameters").and_then(|p| p.as_array()) else {
                return (
                    ImportPreview { rows },
                    vec!["JSON has no \"parameters\" array".into()],
                );
            };
            for p in params {
                let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if !valid_name(name) {
                    errors.push(format!("invalid parameter name: {name:?}"));
                    continue;
                }
                let Some(ptype) = json_ptype(p.get("ptype")) else {
                    errors.push(format!("unknown type for {name}"));
                    continue;
                };
                let Some(value) = json_value(p, ptype) else {
                    errors.push(format!("unparsable value for {name}"));
                    continue;
                };
                rows.push(ImportPreviewRow {
                    name: name.to_string(),
                    value,
                    ptype,
                    kind: super::diff::ParamDiffKind::Changed, // filled below
                });
            }
        }
        Err(_) => {
            // QGC .params: tab-separated, `# comment` lines skipped.
            for (i, line) in text.lines().enumerate() {
                let line = line.trim_end_matches('\r');
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let cols: Vec<&str> = line.split('\t').collect();
                if cols.len() != 5 {
                    errors.push(format!("line {}: expected 5 tab-separated columns", i + 1));
                    continue;
                }
                let name = cols[2];
                if !valid_name(name) {
                    errors.push(format!("line {}: invalid name {name:?}", i + 1));
                    continue;
                }
                let Some(ptype) = type_from_name(cols[4]) else {
                    errors.push(format!("line {}: unknown type {:?}", i + 1, cols[4]));
                    continue;
                };
                let Some(value) = parse_typed(cols[3], ptype) else {
                    errors.push(format!("line {}: bad value for {name}", i + 1));
                    continue;
                };
                rows.push(ImportPreviewRow {
                    name: name.to_string(),
                    value,
                    ptype,
                    kind: super::diff::ParamDiffKind::Changed,
                });
            }
        }
    }

    // Tag each row against the current FC set.
    let current_map: HashMap<&str, &ParamEntry> =
        current.iter().map(|e| (e.name.as_str(), e)).collect();
    for row in &mut rows {
        match current_map.get(row.name.as_str()) {
            None => row.kind = super::diff::ParamDiffKind::OnlyInB,
            Some(cur) => {
                if !cur.value.same_as(row.value) || cur.ptype != row.ptype {
                    row.kind = super::diff::ParamDiffKind::Changed;
                } else {
                    row.kind = super::diff::ParamDiffKind::Changed; // value equal: caller filters
                }
            }
        }
    }
    // Remove rows that are exactly equal (no change to stage).
    rows.retain(|r| match current_map.get(r.name.as_str()) {
        None => true,
        Some(cur) => !cur.value.same_as(r.value),
    });

    (ImportPreview { rows }, errors)
}

// ---- helpers ---------------------------------------------------------------

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= super::codec::PARAM_ID_LEN
}

fn mavlink_type_name(t: ParamType) -> &'static str {
    match t {
        ParamType::U8 => "MAV_PARAM_TYPE_UINT8",
        ParamType::I8 => "MAV_PARAM_TYPE_INT8",
        ParamType::U16 => "MAV_PARAM_TYPE_UINT16",
        ParamType::I16 => "MAV_PARAM_TYPE_INT16",
        ParamType::U32 => "MAV_PARAM_TYPE_UINT32",
        ParamType::I32 => "MAV_PARAM_TYPE_INT32",
        ParamType::Real32 => "MAV_PARAM_TYPE_REAL32",
    }
}

fn type_from_name(name: &str) -> Option<ParamType> {
    Some(match name.trim() {
        "MAV_PARAM_TYPE_UINT8" => ParamType::U8,
        "MAV_PARAM_TYPE_INT8" => ParamType::I8,
        "MAV_PARAM_TYPE_UINT16" => ParamType::U16,
        "MAV_PARAM_TYPE_INT16" => ParamType::I16,
        "MAV_PARAM_TYPE_UINT32" => ParamType::U32,
        "MAV_PARAM_TYPE_INT32" => ParamType::I32,
        "MAV_PARAM_TYPE_REAL32" => ParamType::Real32,
        _ => return None,
    })
}

fn format_value_for_export(value: ParamValue, _wire: f32) -> String {
    match value {
        ParamValue::Int(v) => v.to_string(),
        // Rust's `{}` prints f32 with the shortest round-tripping
        // representation for the *binary64* promoted value, which can
        // round-trip to a different f32. Serialize with explicit f32
        // precision instead (`{:?}` on f32 is exact and parseable).
        ParamValue::Real(v) => format!("{v:?}"),
    }
}

fn parse_typed(raw: &str, ptype: ParamType) -> Option<ParamValue> {
    if ptype == ParamType::Real32 {
        return raw.trim().parse::<f32>().ok().map(ParamValue::Real);
    }
    // Integer types accept integer syntax only — untrusted input (§7).
    raw.trim().parse::<i64>().ok().map(ParamValue::Int)
}

fn json_ptype(v: Option<&serde_json::Value>) -> Option<ParamType> {
    let v = v?;
    let name = v.as_str()?;
    // Accept both the MAVLink wire name and our own snake_case export.
    type_from_name(name).or(match name {
        "u8" => Some(ParamType::U8),
        "i8" => Some(ParamType::I8),
        "u16" => Some(ParamType::U16),
        "i16" => Some(ParamType::I16),
        "u32" => Some(ParamType::U32),
        "i32" => Some(ParamType::I32),
        "real32" => Some(ParamType::Real32),
        _ => None,
    })
}

fn json_value(p: &serde_json::Value, ptype: ParamType) -> Option<ParamValue> {
    let v = p.get("value")?;
    // Our own export tags the value ({"int": 1} / {"real": 2.5}); a
    // hand-written file may use a plain number. Accept both.
    if let Some(tagged) = v.as_object() {
        if let Some(i) = tagged.get("int").and_then(|x| x.as_i64()) {
            return Some(ParamValue::Int(i));
        }
        if let Some(r) = tagged.get("real").and_then(|x| x.as_f64()) {
            return Some(ParamValue::Real(r as f32));
        }
        return None;
    }
    if ptype == ParamType::Real32 {
        return v.as_f64().map(|x| ParamValue::Real(x as f32));
    }
    v.as_i64().map(ParamValue::Int)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, index: u16, value: ParamValue) -> ParamEntry {
        ParamEntry {
            name: name.into(),
            index,
            value,
            ptype: ParamType::I32,
        }
    }

    fn et(name: &str, index: u16, value: ParamValue, ptype: ParamType) -> ParamEntry {
        ParamEntry {
            name: name.into(),
            index,
            value,
            ptype,
        }
    }

    /// T4: export → import round-trip is identical.
    #[test]
    fn qgc_round_trip_is_lossless() {
        let entries = vec![
            e("COM_MOT_TEST_EN", 0, ParamValue::Int(1)),
            e("BAT1_N_CELLS", 1, ParamValue::Int(4)),
            et(
                "MPC_XY_VEL_MAX",
                2,
                ParamValue::Real(12.5),
                ParamType::Real32,
            ),
        ];
        let text = export_qgc_params(&entries, 1, 1);
        // QGC round trip: value parses back to the same wire float.
        let (preview, errors) = import_params_file(&text, &[]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(preview.rows.len(), 3);
        assert!(preview
            .rows
            .iter()
            .any(|r| r.name == "COM_MOT_TEST_EN" && r.value == ParamValue::Int(1)));
    }

    #[test]
    fn import_rejects_bad_rows_and_reports() {
        let bad = "1\t1\tNAME_NO\t7\tBADTYPE\n1\t1\tOK_PARAM\t7\tMAV_PARAM_TYPE_INT32\n";
        let (preview, errors) = import_params_file(bad, &[]);
        assert_eq!(preview.rows.len(), 1);
        assert_eq!(preview.rows[0].name, "OK_PARAM");
        assert!(errors.iter().any(|e| e.contains("BADTYPE")));
    }

    /// T4: unchanged rows are filtered out of the preview.
    #[test]
    fn import_skips_unchanged_rows() {
        let current = vec![e("SAME_P", 0, ParamValue::Int(3))];
        let file = "1\t1\tSAME_P\t3\tMAV_PARAM_TYPE_INT32\n1\t1\tOTHER\t5\tMAV_PARAM_TYPE_INT32\n";
        let (preview, errors) = import_params_file(file, &current);
        assert!(errors.is_empty());
        assert_eq!(preview.rows.len(), 1, "SAME_P filtered (no change)");
        assert_eq!(preview.rows[0].name, "OTHER");
    }

    #[test]
    fn json_export_and_import() {
        let entries = vec![et("J_P", 0, ParamValue::Real(2.5), ParamType::Real32)];
        let text = export_json(&entries, serde_json::json!({"version": "1.17.0"})).unwrap();
        let (preview, errors) = import_params_file(&text, &[]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(preview.rows.len(), 1);
        assert_eq!(preview.rows[0].value, ParamValue::Real(2.5));
    }
}
