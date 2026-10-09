//! Generic numeric-field extraction via a custom `serde::Serializer`.
//!
//! MAVLink messages enable `serde` (workspace `mavlink` feature), so a message
//! can be walked without a per-message mapping: the serializer below collects
//! every numeric leaf as `(field_path, f64)`. Container and variant names are
//! skipped (a message's `SignalId.message_id` already names the type), struct
//! fields join with `.`, and sequence elements render as `field[i]`. Strings
//! are skipped except a field literally named `name`, which becomes the signal
//! field (`NAMED_VALUE_FLOAT/<name>`).
//!
//! Magnetic sensor messages (`HIGHRES_IMU`, `RAW_IMU`, …) additionally get a
//! synthetic **total-field** signal: `mag_total = √(xmag² + ymag² + zmag²)`, so
//! the operator can watch the field magnitude instead of three rotated axes.

use serde::ser::Error as _;
use serde::ser::Impossible;
use serde::Serialize;

use super::SignalId;

/// The concrete error type for extraction (serde requires a sized error).
#[derive(Debug)]
pub struct ExtractError(pub String);

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ExtractError {}

impl serde::ser::Error for ExtractError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        ExtractError(msg.to_string())
    }
}

/// A numeric leaf plus the path it was found at (message-relative).
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedField {
    pub field: String,
    pub value: f64,
}

/// The numeric fields of one message, plus the optional `name` string.
#[derive(Debug, Clone, PartialEq)]
pub struct Extraction {
    pub name: Option<String>,
    pub fields: Vec<ExtractedField>,
}

impl Extraction {
    /// Resolve to signals for one node. A named message (`NAMED_VALUE_FLOAT`)
    /// uses its `name` as the field; arrays keep their `[i]` suffix.
    pub fn signals(&self, system_id: u8, component_id: u8, message_id: u32) -> Vec<SignalId> {
        self.fields
            .iter()
            .map(|f| {
                let field = match (&self.name, f.field.as_str()) {
                    (Some(name), "value") => name.clone(),
                    _ => f.field.clone(),
                };
                SignalId::new(system_id, component_id, message_id, field)
            })
            .collect()
    }
}

/// Extract the numeric fields of any serde-serializable message.
pub fn extract<T: Serialize + ?Sized>(value: &T) -> Result<Extraction, ExtractError> {
    let mut collector = FieldCollector::default();
    value.serialize(&mut collector)?;
    synthesize_mag_total(&mut collector);
    Ok(Extraction {
        name: collector.name,
        fields: collector.fields,
    })
}

/// Append a `mag_total` signal when the message carries a magnetometer triple.
fn synthesize_mag_total(collector: &mut FieldCollector) {
    let get = |name: &str| {
        collector
            .fields
            .iter()
            .find(|f| f.field == name)
            .map(|f| f.value)
    };
    if let (Some(x), Some(y), Some(z)) = (get("xmag"), get("ymag"), get("zmag")) {
        let total = (x * x + y * y + z * z).sqrt();
        collector.fields.push(ExtractedField {
            field: "mag_total".into(),
            value: total,
        });
    }
}

/// Walks a serde value, collecting numeric leaves.
#[derive(Debug, Default)]
pub struct FieldCollector {
    path: String,
    seq_base: String,
    seq_index: usize,
    name: Option<String>,
    fields: Vec<ExtractedField>,
}

impl FieldCollector {
    fn leaf(&mut self, v: f64) -> Result<(), ExtractError> {
        let field = if self.path.is_empty() {
            "value".to_string()
        } else {
            self.path.clone()
        };
        self.fields.push(ExtractedField { field, value: v });
        Ok(())
    }

    fn push_field(&mut self, key: &str) {
        self.path = if self.path.is_empty() {
            key.to_string()
        } else {
            format!("{}.{key}", self.path)
        };
    }

    fn pop_field(&mut self, key: &str) {
        let remove = if self.path.is_empty() {
            0
        } else {
            key.len() + 1
        };
        let end = self.path.len().saturating_sub(remove);
        self.path.truncate(end);
    }
}

impl<'a> serde::Serializer for &'a mut FieldCollector {
    type Ok = ();
    type Error = ExtractError;
    type SerializeSeq = Seq<'a>;
    type SerializeTuple = Seq<'a>;
    type SerializeTupleStruct = Seq<'a>;
    type SerializeTupleVariant = Seq<'a>;
    type SerializeMap = Map<'a>;
    type SerializeStruct = Struct<'a>;
    type SerializeStructVariant = Struct<'a>;

    fn serialize_bool(self, v: bool) -> Result<Self::Ok, Self::Error> {
        self.leaf(if v { 1.0 } else { 0.0 })
    }
    fn serialize_i8(self, v: i8) -> Result<Self::Ok, Self::Error> {
        self.leaf(v as f64)
    }
    fn serialize_i16(self, v: i16) -> Result<Self::Ok, Self::Error> {
        self.leaf(v as f64)
    }
    fn serialize_i32(self, v: i32) -> Result<Self::Ok, Self::Error> {
        self.leaf(v as f64)
    }
    fn serialize_i64(self, v: i64) -> Result<Self::Ok, Self::Error> {
        self.leaf(v as f64)
    }
    fn serialize_u8(self, v: u8) -> Result<Self::Ok, Self::Error> {
        self.leaf(v as f64)
    }
    fn serialize_u16(self, v: u16) -> Result<Self::Ok, Self::Error> {
        self.leaf(v as f64)
    }
    fn serialize_u32(self, v: u32) -> Result<Self::Ok, Self::Error> {
        self.leaf(v as f64)
    }
    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error> {
        self.leaf(v as f64)
    }
    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        self.leaf(v as f64)
    }
    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        self.leaf(v)
    }
    fn serialize_char(self, _v: char) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_str(self, v: &str) -> Result<Self::Ok, Self::Error> {
        if self.path == "name" || self.path.ends_with(".name") {
            self.name = Some(v.to_string());
        }
        Ok(())
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Ok(Seq { col: self })
    }
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Ok(Map { col: self })
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(Struct { col: self })
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Ok(Struct { col: self })
    }
}

/// Serializes sequence elements under `base[i]`, restoring the path after.
pub struct Seq<'a> {
    col: &'a mut FieldCollector,
}

impl serde::ser::SerializeSeq for Seq<'_> {
    type Ok = ();
    type Error = ExtractError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        let index = self.col.seq_index;
        self.col.seq_index += 1;
        let saved = self.col.path.clone();
        self.col.path = format!("{}[{index}]", self.col.seq_base);
        value.serialize(&mut *self.col)?;
        self.col.path = saved;
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl serde::ser::SerializeTuple for Seq<'_> {
    type Ok = ();
    type Error = ExtractError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        serde::ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        serde::ser::SerializeSeq::end(self)
    }
}

impl serde::ser::SerializeTupleStruct for Seq<'_> {
    type Ok = ();
    type Error = ExtractError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        serde::ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        serde::ser::SerializeSeq::end(self)
    }
}

impl serde::ser::SerializeTupleVariant for Seq<'_> {
    type Ok = ();
    type Error = ExtractError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        serde::ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        serde::ser::SerializeSeq::end(self)
    }
}

/// Serializes map values under their string key's path.
pub struct Map<'a> {
    col: &'a mut FieldCollector,
}

impl serde::ser::SerializeMap for Map<'_> {
    type Ok = ();
    type Error = ExtractError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Self::Error> {
        let mut k = String::new();
        key.serialize(KeySerializer { out: &mut k })?;
        self.col.push_field(&k);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        value.serialize(&mut *self.col)?;
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

/// Serializes struct fields: push the field name, serialize, pop it.
pub struct Struct<'a> {
    col: &'a mut FieldCollector,
}

impl serde::ser::SerializeStruct for Struct<'_> {
    type Ok = ();
    type Error = ExtractError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.col.push_field(key);
        value.serialize(&mut *self.col)?;
        self.col.pop_field(key);
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl serde::ser::SerializeStructVariant for Struct<'_> {
    type Ok = ();
    type Error = ExtractError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        serde::ser::SerializeStruct::serialize_field(self, key, value)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        serde::ser::SerializeStruct::end(self)
    }
}

/// Serializes a map key into a string (only string keys are used).
struct KeySerializer<'a> {
    out: &'a mut String,
}

impl serde::Serializer for KeySerializer<'_> {
    type Ok = ();
    type Error = ExtractError;
    type SerializeSeq = Impossible<(), ExtractError>;
    type SerializeTuple = Impossible<(), ExtractError>;
    type SerializeTupleStruct = Impossible<(), ExtractError>;
    type SerializeTupleVariant = Impossible<(), ExtractError>;
    type SerializeMap = Impossible<(), ExtractError>;
    type SerializeStruct = Impossible<(), ExtractError>;
    type SerializeStructVariant = Impossible<(), ExtractError>;

    fn serialize_str(self, v: &str) -> Result<Self::Ok, Self::Error> {
        self.out.push_str(v);
        Ok(())
    }

    fn serialize_bool(self, _v: bool) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_i8(self, _v: i8) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_i16(self, _v: i16) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_i32(self, _v: i32) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_i64(self, _v: i64) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_u8(self, _v: u8) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_u16(self, _v: u16) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_u32(self, _v: u32) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_u64(self, _v: u64) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_f32(self, _v: f32) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_f64(self, _v: f64) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_char(self, _v: char) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_some<T: Serialize + ?Sized>(self, _v: &T) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _v: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _v: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Err(ExtractError::custom("non-string map key"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mavlink::common::{
        HighresImuUpdatedFlags, MavMessage, ATTITUDE_DATA, HIGHRES_IMU_DATA, NAMED_VALUE_FLOAT_DATA,
    };
    use mavlink::types::CharArray;

    fn value_of(fields: &[ExtractedField], name: &str) -> Option<f64> {
        fields.iter().find(|f| f.field == name).map(|f| f.value)
    }

    #[test]
    fn attitude_fields_are_extracted_by_name() {
        let msg = MavMessage::ATTITUDE(ATTITUDE_DATA {
            time_boot_ms: 100,
            roll: 0.1,
            pitch: 0.2,
            yaw: 0.3,
            rollspeed: 1.0,
            pitchspeed: 2.0,
            yawspeed: 3.0,
        });
        let ex = extract(&msg).expect("extract");
        assert_eq!(ex.name, None);
        assert!(
            ex.fields
                .iter()
                .any(|f| f.field == "roll" && (f.value - 0.1).abs() < 1e-6),
            "roll"
        );
        assert!(
            ex.fields
                .iter()
                .any(|f| f.field == "time_boot_ms" && f.value == 100.0),
            "time"
        );
    }

    #[test]
    fn highres_imu_mag_triple_yields_mag_total() {
        let msg = MavMessage::HIGHRES_IMU(HIGHRES_IMU_DATA {
            time_usec: 1,
            xacc: 0.0,
            yacc: 0.0,
            zacc: 0.0,
            xgyro: 0.0,
            ygyro: 0.0,
            zgyro: 0.0,
            xmag: 3.0,
            ymag: 4.0,
            zmag: 0.0,
            abs_pressure: 0.0,
            diff_pressure: 0.0,
            pressure_alt: 0.0,
            temperature: 0.0,
            fields_updated: HighresImuUpdatedFlags::empty(),
        });
        let ex = extract(&msg).expect("extract");
        assert_eq!(value_of(&ex.fields, "mag_total"), Some(5.0), "3-4-5 total");
        assert_eq!(value_of(&ex.fields, "xmag"), Some(3.0));
    }

    #[test]
    fn named_value_float_uses_the_name() {
        let msg = MavMessage::NAMED_VALUE_FLOAT(NAMED_VALUE_FLOAT_DATA {
            time_boot_ms: 0,
            name: CharArray::from(*b"mag_total\0"),
            value: 48600.0,
        });
        let ex = extract(&msg).expect("extract");
        assert_eq!(ex.name.as_deref(), Some("mag_total"));
        let ids = ex.signals(1, 1, 251);
        assert!(ids.iter().any(|s| s.field == "mag_total"), "named field");
    }

    #[test]
    fn no_mag_triple_means_no_mag_total() {
        let msg = MavMessage::ATTITUDE(ATTITUDE_DATA {
            time_boot_ms: 0,
            roll: 0.0,
            pitch: 0.0,
            yaw: 0.0,
            rollspeed: 0.0,
            pitchspeed: 0.0,
            yawspeed: 0.0,
        });
        let ex = extract(&msg).expect("extract");
        assert!(
            !ex.fields.iter().any(|f| f.field == "mag_total"),
            "no mag fields"
        );
    }
}
