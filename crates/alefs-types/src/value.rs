//! Typed values: scalars and nested structures.

use std::collections::BTreeMap;

/// Scalar leaf values.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scalar {
    Null,
    Bool(bool),
    Int(i64),
    /// IEEE-754 bits; equality is by bit pattern (see design §4.2).
    Float(u64),
    String(String),
    Bytes(Vec<u8>),
}

impl Scalar {
    pub fn from_f64(f: f64) -> Self {
        Scalar::Float(f.to_bits())
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Scalar::Float(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }
}

/// First-class alefsdb value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Scalar(Scalar),
    /// String keys; `BTreeMap` keeps iteration ordered for canonical encoding.
    Hash(BTreeMap<String, Value>),
    /// Members unique by canonical encoding; stored sorted by encoding.
    Set(Vec<Value>),
    List(Vec<Value>),
    /// Ordered map with scalar keys.
    Tree(BTreeMap<Scalar, Value>),
}

impl Value {
    pub fn null() -> Self {
        Value::Scalar(Scalar::Null)
    }

    pub fn bool(b: bool) -> Self {
        Value::Scalar(Scalar::Bool(b))
    }

    pub fn int(n: i64) -> Self {
        Value::Scalar(Scalar::Int(n))
    }

    pub fn float(f: f64) -> Self {
        Value::Scalar(Scalar::from_f64(f))
    }

    pub fn string(s: impl Into<String>) -> Self {
        Value::Scalar(Scalar::String(s.into()))
    }

    pub fn bytes(b: impl Into<Vec<u8>>) -> Self {
        Value::Scalar(Scalar::Bytes(b.into()))
    }

    pub fn typename(&self) -> &'static str {
        match self {
            Value::Scalar(Scalar::Null) => "null",
            Value::Scalar(Scalar::Bool(_)) => "bool",
            Value::Scalar(Scalar::Int(_)) => "int",
            Value::Scalar(Scalar::Float(_)) => "float",
            Value::Scalar(Scalar::String(_)) => "string",
            Value::Scalar(Scalar::Bytes(_)) => "bytes",
            Value::Hash(_) => "hash",
            Value::Set(_) => "set",
            Value::List(_) => "list",
            Value::Tree(_) => "tree",
        }
    }
}
