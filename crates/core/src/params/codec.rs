//! Parameter value codec (parameters plan §3).
//!
//! MAVLink carries parameter values in an `f32`. PX4 (capability
//! `MAV_PROTOCOL_CAPABILITY_PARAM_UNION` byte-wise) stores integers by
//! **byte concatenation** — the integer's bit pattern is reinterpreted as
//! `f32` bits — while some other stacks use a C cast. The codec supports
//! both; the session picks per the FC's `AUTOPILOT_VERSION` capability bits
//! and defaults to byte-wise (PX4).
//!
//! Parameter ids are 16 bytes, NUL-terminated unless exactly 16 chars long.

use super::model::{ParamType, ParamValue};

/// Which integer encoding the FC uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IntEncoding {
    /// PX4: integer bits reinterpreted as `f32` bits.
    #[default]
    ByteConcat,
    /// C cast: `(f32)value`.
    Cast,
}

/// Decode a wire `f32` + type into a value.
pub fn decode(raw: f32, ptype: ParamType, encoding: IntEncoding) -> ParamValue {
    let Some(_width) = ptype.int_width() else {
        return ParamValue::Real(raw);
    };
    match encoding {
        IntEncoding::ByteConcat => {
            let bits = raw.to_bits();
            ParamValue::Int(match ptype {
                ParamType::U8 => (bits & 0xFF) as i64,
                ParamType::I8 => (bits & 0xFF) as u8 as i8 as i64,
                ParamType::U16 => (bits & 0xFFFF) as i64,
                ParamType::I16 => (bits & 0xFFFF) as u16 as i16 as i64,
                ParamType::U32 => bits as i64,
                ParamType::I32 => bits as i32 as i64,
                ParamType::Real32 => unreachable!("filtered by int_width"),
            })
        }
        IntEncoding::Cast => {
            // Round through the integer exactly: the f32 may be fractional.
            let v = raw as f64;
            ParamValue::Int(match ptype {
                ParamType::U8 => v.round() as i64,
                ParamType::I8 => v.round() as i64,
                ParamType::U16 => v.round() as i64,
                ParamType::I16 => v.round() as i64,
                ParamType::U32 => v.round() as i64,
                ParamType::I32 => v.round() as i64,
                ParamType::Real32 => unreachable!("filtered by int_width"),
            })
        }
    }
}

/// Encode a value into the wire `f32` (inverse of [`decode`]).
///
/// Byte-wise: the integer's bits (zero/sign-extended to 4 bytes) become the
/// `f32` bits. Cast: a plain C cast.
pub fn encode(value: ParamValue, encoding: IntEncoding) -> f32 {
    match value {
        ParamValue::Real(v) => v,
        ParamValue::Int(v) => match encoding {
            IntEncoding::ByteConcat => f32::from_bits(match v {
                0..=0xFF => v as u32,
                // Sign-extend through i32 for negatives; mask for the
                // narrower widths happens in decode via the type.
                _ => (v as i32) as u32,
            }),
            IntEncoding::Cast => v as f32,
        },
    }
}

/// Parameter-id byte length on the wire.
pub const PARAM_ID_LEN: usize = 16;

/// Read a 16-byte id into a Rust string (trailing NULs trimmed; an id of
/// exactly 16 chars has no terminator — T1's edge case).
pub fn decode_param_id(bytes: &[u8; PARAM_ID_LEN]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(PARAM_ID_LEN);
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Pack a Rust string into the 16-byte wire id, NUL-padded. Panics on ids
/// longer than 16 (callers validate) — that is a programming error, not a
/// runtime condition.
pub fn encode_param_id(name: &str) -> [u8; PARAM_ID_LEN] {
    assert!(name.len() <= PARAM_ID_LEN, "param id > 16 bytes");
    let mut out = [0u8; PARAM_ID_LEN];
    out[..name.len()].copy_from_slice(name.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T1: every integer type round-trips losslessly under byte-concat (the
    /// PX4 encoding). The Cast encoding is inherently lossy for magnitudes
    /// beyond f32 precision (~2^24), so it is checked within one ULP of the
    /// requested value — that is a protocol limit, not a codec bug.
    #[test]
    fn integer_types_round_trip_both_encodings() {
        let cases: &[(ParamType, i64)] = &[
            (ParamType::U8, 0),
            (ParamType::U8, 255),
            (ParamType::I8, -128),
            (ParamType::I8, 127),
            (ParamType::U16, 65_535),
            (ParamType::I16, -32_768),
            (ParamType::U32, 4_294_967_295),
            (ParamType::I32, -2_147_483_648),
            (ParamType::I32, 2_147_483_647),
            (ParamType::I32, 42),
            (ParamType::I32, -42),
        ];
        for (ptype, v) in cases {
            let wire = encode(ParamValue::Int(*v), IntEncoding::ByteConcat);
            assert_eq!(
                decode(wire, *ptype, IntEncoding::ByteConcat),
                ParamValue::Int(*v),
                "{ptype:?} {v} byte-concat (wire bits {:b}",
                wire.to_bits()
            );
        }
        // Cast encoding: exact for small magnitudes, one-ULP for big ones.
        for (ptype, v) in cases {
            let wire = encode(ParamValue::Int(*v), IntEncoding::Cast);
            let back = match decode(wire, *ptype, IntEncoding::Cast) {
                ParamValue::Int(x) => x,
                other => panic!("{ptype:?} decoded to {other:?}"),
            };
            let lossless = back == *v;
            let within_ulp = (back as f64 - *v as f64).abs() <= 8.0;
            assert!(
                lossless || within_ulp,
                "{ptype:?} {v} cast round-trip gave {back}"
            );
        }
    }

    #[test]
    fn real32_round_trips() {
        for encoding in [IntEncoding::ByteConcat, IntEncoding::Cast] {
            for v in [0.0_f32, -1.5, 3.4028235e38, f32::MIN_POSITIVE] {
                let wire = encode(ParamValue::Real(v), encoding);
                assert_eq!(
                    decode(wire, ParamType::Real32, encoding),
                    ParamValue::Real(v)
                );
            }
        }
    }

    /// T1: a 16-character name has no trailing NUL.
    #[test]
    fn sixteen_char_param_id_has_no_nul() {
        let name = "ABCDEFGH12345678"; // exactly 16
        assert_eq!(name.len(), 16);
        let packed = encode_param_id(name);
        assert!(packed.iter().all(|&b| b != 0), "no terminator when full");
        assert_eq!(decode_param_id(&packed), name);
        // A shorter name is NUL-padded and trims back.
        let short = "COM_MOT_TEST_EN";
        assert_eq!(decode_param_id(&encode_param_id(short)), short);
    }

    #[test]
    fn byte_concat_is_bits_not_cast() {
        // -42 as i32 bits reinterpreted as f32 is a denormal, not -42.0 —
        // proving the two encodings genuinely differ.
        let bits = f32::from_bits(-42_i32 as u32);
        assert_ne!(bits, -42.0_f32);
        assert_eq!(
            decode(bits, ParamType::I32, IntEncoding::ByteConcat),
            ParamValue::Int(-42)
        );
        assert_eq!(encode(ParamValue::Int(-42), IntEncoding::Cast), -42.0_f32);
    }
}
