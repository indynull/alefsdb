//! Versioned canonical binary encoding for [`Value`](crate::Value).

use crate::value::{Scalar, Value};
use std::collections::BTreeMap;

/// On-disk / equality codec version byte.
pub const CODEC_VERSION: u8 = 1;

const TAG_NULL: u8 = 0x00;
const TAG_BOOL: u8 = 0x01;
const TAG_INT: u8 = 0x02;
const TAG_FLOAT: u8 = 0x03;
const TAG_STRING: u8 = 0x04;
const TAG_BYTES: u8 = 0x05;
const TAG_HASH: u8 = 0x10;
const TAG_SET: u8 = 0x11;
const TAG_LIST: u8 = 0x12;
const TAG_TREE: u8 = 0x13;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    EmptyInput,
    UnsupportedVersion(u8),
    UnexpectedEof,
    UnknownTag(u8),
    InvalidUtf8,
    InvalidBool(u8),
    TrailingBytes,
    DuplicateSetMember,
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecError::EmptyInput => write!(f, "empty input"),
            CodecError::UnsupportedVersion(v) => write!(f, "unsupported codec version {v}"),
            CodecError::UnexpectedEof => write!(f, "unexpected end of input"),
            CodecError::UnknownTag(t) => write!(f, "unknown value tag 0x{t:02x}"),
            CodecError::InvalidUtf8 => write!(f, "invalid UTF-8 in string"),
            CodecError::InvalidBool(b) => write!(f, "invalid bool byte {b}"),
            CodecError::TrailingBytes => write!(f, "trailing bytes after value"),
            CodecError::DuplicateSetMember => write!(f, "duplicate set member"),
        }
    }
}

impl std::error::Error for CodecError {}

/// Encode `value` to canonical bytes (version byte + payload).
pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(CODEC_VERSION);
    encode_value(value, &mut out);
    out
}

/// Decode a full buffer produced by [`encode`].
pub fn decode(bytes: &[u8]) -> Result<Value, CodecError> {
    if bytes.is_empty() {
        return Err(CodecError::EmptyInput);
    }
    let version = bytes[0];
    if version != CODEC_VERSION {
        return Err(CodecError::UnsupportedVersion(version));
    }
    let mut cur = &bytes[1..];
    let value = decode_value(&mut cur)?;
    if !cur.is_empty() {
        return Err(CodecError::TrailingBytes);
    }
    Ok(value)
}

/// Canonical payload only (no version), used for set membership ordering.
pub fn encode_payload(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode_value(value, &mut out);
    out
}

fn encode_value(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Scalar(s) => encode_scalar(s, out),
        Value::Hash(map) => {
            out.push(TAG_HASH);
            write_u32(out, map.len() as u32);
            for (k, v) in map {
                write_str(out, k);
                encode_value(v, out);
            }
        }
        Value::Set(members) => {
            // Canonical: unique members sorted by payload encoding.
            let mut encoded: Vec<(Vec<u8>, &Value)> = members
                .iter()
                .map(|m| (encode_payload(m), m))
                .collect();
            encoded.sort_by(|a, b| a.0.cmp(&b.0));
            encoded.dedup_by(|a, b| a.0 == b.0);
            out.push(TAG_SET);
            write_u32(out, encoded.len() as u32);
            for (payload, _) in encoded {
                out.extend_from_slice(&payload);
            }
        }
        Value::List(items) => {
            out.push(TAG_LIST);
            write_u32(out, items.len() as u32);
            for item in items {
                encode_value(item, out);
            }
        }
        Value::Tree(map) => {
            out.push(TAG_TREE);
            write_u32(out, map.len() as u32);
            for (k, v) in map {
                encode_scalar(k, out);
                encode_value(v, out);
            }
        }
    }
}

fn encode_scalar(s: &Scalar, out: &mut Vec<u8>) {
    match s {
        Scalar::Null => out.push(TAG_NULL),
        Scalar::Bool(b) => {
            out.push(TAG_BOOL);
            out.push(u8::from(*b));
        }
        Scalar::Int(n) => {
            out.push(TAG_INT);
            out.extend_from_slice(&n.to_le_bytes());
        }
        Scalar::Float(bits) => {
            out.push(TAG_FLOAT);
            out.extend_from_slice(&bits.to_le_bytes());
        }
        Scalar::String(s) => {
            out.push(TAG_STRING);
            write_str(out, s);
        }
        Scalar::Bytes(b) => {
            out.push(TAG_BYTES);
            write_u32(out, b.len() as u32);
            out.extend_from_slice(b);
        }
    }
}

fn decode_value(cur: &mut &[u8]) -> Result<Value, CodecError> {
    let tag = read_u8(cur)?;
    match tag {
        TAG_NULL => Ok(Value::Scalar(Scalar::Null)),
        TAG_BOOL => {
            let b = read_u8(cur)?;
            match b {
                0 => Ok(Value::bool(false)),
                1 => Ok(Value::bool(true)),
                other => Err(CodecError::InvalidBool(other)),
            }
        }
        TAG_INT => {
            let n = i64::from_le_bytes(read_array(cur)?);
            Ok(Value::int(n))
        }
        TAG_FLOAT => {
            let bits = u64::from_le_bytes(read_array(cur)?);
            Ok(Value::Scalar(Scalar::Float(bits)))
        }
        TAG_STRING => {
            let s = read_str(cur)?;
            Ok(Value::string(s))
        }
        TAG_BYTES => {
            let len = read_u32(cur)? as usize;
            let data = read_slice(cur, len)?.to_vec();
            Ok(Value::bytes(data))
        }
        TAG_HASH => {
            let n = read_u32(cur)? as usize;
            let mut map = BTreeMap::new();
            for _ in 0..n {
                let k = read_str(cur)?;
                let v = decode_value(cur)?;
                map.insert(k, v);
            }
            Ok(Value::Hash(map))
        }
        TAG_SET => {
            let n = read_u32(cur)? as usize;
            let mut members = Vec::with_capacity(n);
            let mut last_payload: Option<Vec<u8>> = None;
            for _ in 0..n {
                let v = decode_value(cur)?;
                let payload = encode_payload(&v);
                // Canonical sets are sorted and unique by payload.
                if let Some(prev) = &last_payload {
                    if payload.as_slice() <= prev.as_slice() {
                        return Err(CodecError::DuplicateSetMember);
                    }
                }
                last_payload = Some(payload);
                members.push(v);
            }
            Ok(Value::Set(members))
        }
        TAG_LIST => {
            let n = read_u32(cur)? as usize;
            let mut items = Vec::with_capacity(n);
            for _ in 0..n {
                items.push(decode_value(cur)?);
            }
            Ok(Value::List(items))
        }
        TAG_TREE => {
            let n = read_u32(cur)? as usize;
            let mut map = BTreeMap::new();
            for _ in 0..n {
                let k = decode_scalar_value(cur)?;
                let v = decode_value(cur)?;
                map.insert(k, v);
            }
            Ok(Value::Tree(map))
        }
        other => Err(CodecError::UnknownTag(other)),
    }
}

fn decode_scalar_value(cur: &mut &[u8]) -> Result<Scalar, CodecError> {
    match decode_value(cur)? {
        Value::Scalar(s) => Ok(s),
        _ => Err(CodecError::UnknownTag(0xff)),
    }
}

fn write_u32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}

fn write_str(out: &mut Vec<u8>, s: &str) {
    write_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

fn read_u8(cur: &mut &[u8]) -> Result<u8, CodecError> {
    if cur.is_empty() {
        return Err(CodecError::UnexpectedEof);
    }
    let b = cur[0];
    *cur = &cur[1..];
    Ok(b)
}

fn read_array<const N: usize>(cur: &mut &[u8]) -> Result<[u8; N], CodecError> {
    let slice = read_slice(cur, N)?;
    let mut arr = [0u8; N];
    arr.copy_from_slice(slice);
    Ok(arr)
}

fn read_u32(cur: &mut &[u8]) -> Result<u32, CodecError> {
    Ok(u32::from_le_bytes(read_array(cur)?))
}

fn read_slice<'a>(cur: &mut &'a [u8], len: usize) -> Result<&'a [u8], CodecError> {
    if cur.len() < len {
        return Err(CodecError::UnexpectedEof);
    }
    let (head, tail) = cur.split_at(len);
    *cur = tail;
    Ok(head)
}

fn read_str(cur: &mut &[u8]) -> Result<String, CodecError> {
    let len = read_u32(cur)? as usize;
    let bytes = read_slice(cur, len)?;
    std::str::from_utf8(bytes)
        .map(|s| s.to_owned())
        .map_err(|_| CodecError::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn rt(v: Value) -> Value {
        decode(&encode(&v)).expect("round-trip")
    }

    #[test]
    fn round_trip_scalars() {
        assert_eq!(rt(Value::null()), Value::null());
        assert_eq!(rt(Value::bool(true)), Value::bool(true));
        assert_eq!(rt(Value::bool(false)), Value::bool(false));
        assert_eq!(rt(Value::int(-42)), Value::int(-42));
        assert_eq!(rt(Value::float(1.5)), Value::float(1.5));
        assert_eq!(rt(Value::string("hi")), Value::string("hi"));
        assert_eq!(rt(Value::bytes(vec![0, 1, 2])), Value::bytes(vec![0, 1, 2]));
    }

    #[test]
    fn float_equality_is_by_bits() {
        let nan1 = Value::Scalar(Scalar::Float(f64::NAN.to_bits()));
        let nan2 = Value::Scalar(Scalar::Float(f64::NAN.to_bits()));
        assert_eq!(encode(&nan1), encode(&nan2));
        assert_eq!(rt(nan1.clone()), nan2);
    }

    #[test]
    fn round_trip_list_and_hash() {
        let mut map = BTreeMap::new();
        map.insert("b".into(), Value::int(2));
        map.insert("a".into(), Value::int(1));
        let v = Value::List(vec![Value::Hash(map), Value::string("x")]);
        assert_eq!(rt(v.clone()), v);
    }

    #[test]
    fn hash_encoding_is_key_order_independent() {
        let mut a = BTreeMap::new();
        a.insert("z".into(), Value::int(1));
        a.insert("a".into(), Value::int(2));
        let mut b = BTreeMap::new();
        b.insert("a".into(), Value::int(2));
        b.insert("z".into(), Value::int(1));
        assert_eq!(encode(&Value::Hash(a)), encode(&Value::Hash(b)));
    }

    #[test]
    fn set_dedupes_and_orders_by_encoding() {
        let v = Value::Set(vec![
            Value::int(2),
            Value::int(1),
            Value::int(2),
            Value::string("a"),
        ]);
        let decoded = rt(v);
        match decoded {
            Value::Set(members) => {
                assert_eq!(members.len(), 3);
                // Sorted by payload encoding: int tags before string tag... depends on tags.
                // int=0x02, string=0x04 so ints first; int 1 before int 2.
                assert_eq!(members[0], Value::int(1));
                assert_eq!(members[1], Value::int(2));
                assert_eq!(members[2], Value::string("a"));
            }
            other => panic!("expected set, got {other:?}"),
        }
    }

    #[test]
    fn round_trip_tree() {
        let mut map = BTreeMap::new();
        map.insert(Scalar::Int(1), Value::string("one"));
        map.insert(Scalar::String("k".into()), Value::bool(true));
        let v = Value::Tree(map);
        assert_eq!(rt(v.clone()), v);
    }

    #[test]
    fn rejects_bad_version() {
        let mut bytes = encode(&Value::int(1));
        bytes[0] = 99;
        assert!(matches!(
            decode(&bytes),
            Err(CodecError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(decode(&[]), Err(CodecError::EmptyInput));
    }

    #[test]
    fn encode_is_deterministic_for_nested() {
        let v = Value::List(vec![
            Value::Set(vec![Value::int(3), Value::int(1)]),
            Value::null(),
        ]);
        assert_eq!(encode(&v), encode(&v));
        assert_eq!(rt(v.clone()), rt(v));
    }
}
