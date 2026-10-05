//! Decoded value tree produced by the flatbuffer walker.

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};

/// One decoded flatbuffer value. Tables keep schema field order; fields
/// absent from the buffer are omitted.
#[derive(Debug, Clone)]
pub enum RawValue {
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str(String),
    Enum {
        name: Option<String>,
        value: i64,
    },
    /// A timestamp field, kept as its stored integer plus a readable form.
    Time {
        raw: u64,
        iso: String,
    },
    /// A 12-byte object ID, with the repo-relative files it points at.
    Id {
        id: String,
        links: Vec<String>,
    },
    /// An 8-byte node ID, resolved to a node path when a snapshot is known.
    NodeId {
        id: String,
        path: Option<String>,
    },
    Bytes {
        len: usize,
        preview: Vec<u8>,
        decoded: Option<Decoded>,
    },
    Table {
        type_name: String,
        fields: Vec<(String, RawValue)>,
    },
    /// Vector of tables, structs, or strings. `items` holds the window
    /// `start..start + items.len()` of `len` elements.
    Vector {
        len: usize,
        start: usize,
        items: Vec<RawValue>,
    },
    /// Vector of scalars, always complete (these are short coordinate lists).
    Scalars(Vec<RawValue>),
}

/// Human-meaningful decoding of an opaque byte field.
#[derive(Debug, Clone)]
pub enum Decoded {
    Text(String),
    Json(serde_json::Value),
}

impl RawValue {
    pub fn field(&self, name: &str) -> Option<&RawValue> {
        match self {
            RawValue::Table { fields, .. } => {
                fields.iter().find(|(n, _)| n == name).map(|(_, v)| v)
            }
            _ => None,
        }
    }
}

impl Serialize for Decoded {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Decoded::Text(t) => s.serialize_str(t),
            Decoded::Json(v) => v.serialize(s),
        }
    }
}

impl Serialize for RawValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            RawValue::Bool(b) => s.serialize_bool(*b),
            RawValue::Int(i) => s.serialize_i64(*i),
            RawValue::UInt(u) => s.serialize_u64(*u),
            RawValue::Float(f) => s.serialize_f64(*f),
            RawValue::Str(t) => s.serialize_str(t),
            RawValue::Enum { name: Some(n), .. } => s.serialize_str(n),
            RawValue::Enum { name: None, value } => s.serialize_i64(*value),
            RawValue::Time { raw, iso } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("value", raw)?;
                m.serialize_entry("time", iso)?;
                m.end()
            }
            RawValue::Id { id, links } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("id", id)?;
                m.serialize_entry("links", links)?;
                m.end()
            }
            RawValue::NodeId { id, path } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("node_id", id)?;
                m.serialize_entry("path", path)?;
                m.end()
            }
            RawValue::Bytes {
                len,
                preview,
                decoded,
            } => {
                let mut m = s.serialize_map(None)?;
                m.serialize_entry("len", len)?;
                m.serialize_entry("hex", &hex(preview))?;
                if let Some(d) = decoded {
                    m.serialize_entry("decoded", d)?;
                }
                m.end()
            }
            RawValue::Table { type_name, fields } => {
                let mut m = s.serialize_map(Some(fields.len() + 1))?;
                m.serialize_entry("_type", type_name)?;
                for (k, v) in fields {
                    m.serialize_entry(k, v)?;
                }
                m.end()
            }
            RawValue::Vector { len, start, items } => {
                let mut m = s.serialize_map(Some(3))?;
                m.serialize_entry("len", len)?;
                m.serialize_entry("start", start)?;
                m.serialize_entry("items", items)?;
                m.end()
            }
            RawValue::Scalars(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for i in items {
                    seq.serialize_element(i)?;
                }
                seq.end()
            }
        }
    }
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}
