//! Field-specific interpretation layered on the generic walk: IDs become
//! base32 strings linked to the files they name, timestamps get a readable
//! form, and opaque byte fields are decoded where the format defines them.

use std::collections::HashMap;
use std::io::Read;

use chrono::DateTime;
use icechunk::format::{NodeId, OVERWRITTEN_FILES_PATH, SnapshotId};
use zstd::dict::DecoderDictionary;

use super::FileKind;
use super::value::{Decoded, RawValue};
use crate::sanitize::sanitize;

const PREVIEW_BYTES: usize = 32;

/// Locations are URLs, so anything near this size is corrupt.
const MAX_LOCATION_LEN: u64 = 64 * 1024;

pub struct Annotator<'a> {
    /// Node ID bytes → node path, from a snapshot that contains the nodes.
    node_paths: &'a HashMap<[u8; 8], String>,
    /// The manifest's `location_dictionary`, for `compressed_location`.
    location_dictionary: Option<&'a DecoderDictionary<'static>>,
}

/// Crockford base32, the form icechunk prints IDs in.
pub fn encode_id(bytes: &[u8]) -> String {
    match bytes.len() {
        8 => NodeId::new(bytes.try_into().expect("length checked")).to_string(),
        12 => SnapshotId::new(bytes.try_into().expect("length checked")).to_string(),
        _ => super::hex(bytes),
    }
}

impl<'a> Annotator<'a> {
    pub fn new(
        node_paths: &'a HashMap<[u8; 8], String>,
        location_dictionary: Option<&'a DecoderDictionary<'static>>,
    ) -> Self {
        Self {
            node_paths,
            location_dictionary,
        }
    }

    pub fn object_id(&self, owner: &str, field: &str, bytes: &[u8]) -> RawValue {
        let id = encode_id(bytes);
        if let Ok(node) = <[u8; 8]>::try_from(bytes) {
            return RawValue::NodeId {
                path: self.node_paths.get(&node).cloned(),
                id,
            };
        }
        let targets: &[FileKind] = match (owner, field) {
            ("Snapshot" | "TransactionLog" | "SnapshotInfo", "id")
            | ("Snapshot", "parent_id")
            | (_, "new_snap_id" | "previous_snap_id") => {
                &[FileKind::Snapshot, FileKind::TransactionLog]
            }
            ("SnapshotInfo", "pruned_ancestor_tx_logs") => &[FileKind::TransactionLog],
            ("Manifest" | "ManifestFileInfo" | "ManifestFileInfoV2", "id")
            | ("ManifestRef", "object_id") => &[FileKind::Manifest],
            ("ChunkRef", "chunk_id") => &[FileKind::Chunk],
            _ => &[],
        };
        RawValue::Id {
            links: targets
                .iter()
                .map(|k| format!("{}/{id}", k.dir()))
                .collect(),
            id,
        }
    }

    pub fn scalar(&self, owner: &str, field: &str, value: RawValue) -> RawValue {
        let RawValue::UInt(raw) = value else {
            return value;
        };
        let time = match (owner, field) {
            ("Snapshot" | "SnapshotInfo", "flushed_at")
            | ("RepoStatus", "set_at")
            | ("Update", "updated_at") => DateTime::from_timestamp_micros(raw as i64),
            ("ChunkRef", "checksum_last_modified") => DateTime::from_timestamp(raw as i64, 0),
            _ => None,
        };
        match time {
            Some(t) => RawValue::Time {
                raw,
                iso: t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            },
            None => value,
        }
    }

    pub fn string(&self, owner: &str, field: &str, s: String) -> RawValue {
        match (owner, field) {
            ("Update", "backup_path") | ("Repo", "repo_before_updates") => RawValue::Id {
                links: vec![format!("{OVERWRITTEN_FILES_PATH}/{s}")],
                id: s,
            },
            _ => RawValue::Str(s),
        }
    }

    pub fn bytes(&self, owner: &str, field: &str, data: &[u8]) -> RawValue {
        let decoded = match (owner, field) {
            ("MetadataItem", "value") | ("Repo", "config") => flexbuffer_json(data),
            ("NodeSnapshot", "user_data") => utf8_json(data),
            ("ChunkRef", "compressed_location") => self.decompress_location(data),
            _ => None,
        };
        RawValue::Bytes {
            len: data.len(),
            preview: data[..data.len().min(PREVIEW_BYTES)].to_vec(),
            decoded,
        }
    }

    fn decompress_location(&self, data: &[u8]) -> Option<Decoded> {
        let dict = self.location_dictionary?;
        let mut out = Vec::new();
        let result = zstd::stream::read::Decoder::with_prepared_dictionary(data, dict)
            .and_then(|d| d.take(MAX_LOCATION_LEN).read_to_end(&mut out));
        Some(Decoded::Text(match result {
            Ok(_) => sanitize(&String::from_utf8_lossy(&out)),
            Err(e) => format!("<decompression failed: {e}>"),
        }))
    }
}

fn flexbuffer_json(data: &[u8]) -> Option<Decoded> {
    if data.is_empty() {
        return None;
    }
    // The flexbuffers reader indexes the buffer unchecked at crafted offsets,
    // so contain its panics. `flex_to_json` bounds the work: each element costs
    // one unit of a budget equal to the buffer length, which stops vectors that
    // claim more elements than the buffer could hold.
    let value = std::panic::catch_unwind(|| {
        let root = flexbuffers::Reader::get_root(data).ok()?;
        flex_to_json(&root, &mut data.len(), 0)
    })
    .ok()
    .flatten()?;
    Some(Decoded::Json(sanitize_json(value)))
}

fn flex_to_json(
    r: &flexbuffers::Reader<&[u8]>,
    budget: &mut usize,
    depth: usize,
) -> Option<serde_json::Value> {
    use flexbuffers::FlexBufferType as T;
    use serde_json::Value;
    if depth > 64 || *budget == 0 {
        return None;
    }
    *budget -= 1;
    let mut take = |n: usize| -> Option<()> {
        *budget = budget.checked_sub(n)?;
        Some(())
    };
    Some(match r.flexbuffer_type() {
        T::Null => Value::Null,
        T::Bool => Value::Bool(r.as_bool()),
        T::Int | T::IndirectInt => Value::from(r.as_i64()),
        T::UInt | T::IndirectUInt => Value::from(r.as_u64()),
        T::Float | T::IndirectFloat => {
            serde_json::Number::from_f64(r.as_f64()).map_or(Value::Null, Value::Number)
        }
        T::String | T::Key => Value::String(r.get_str().ok()?.to_string()),
        T::Blob => {
            let blob = r.get_blob().ok()?;
            take(blob.0.len())?;
            Value::Array(blob.0.iter().map(|&b| Value::from(b)).collect())
        }
        T::Map => {
            let map = r.get_map().ok()?;
            take(map.len())?;
            let mut out = serde_json::Map::new();
            for (k, v) in map.iter_keys().zip(map.iter_values()) {
                out.insert(k.to_string(), flex_to_json(&v, budget, depth + 1)?);
            }
            Value::Object(out)
        }
        t if t.is_vector() => {
            let vec = r.get_vector().ok()?;
            take(vec.len())?;
            let items = vec
                .iter()
                .map(|v| flex_to_json(&v, budget, depth + 1))
                .collect::<Option<Vec<_>>>()?;
            Value::Array(items)
        }
        _ => return None,
    })
}

fn utf8_json(data: &[u8]) -> Option<Decoded> {
    if data.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
        return Some(Decoded::Json(sanitize_json(value)));
    }
    std::str::from_utf8(data)
        .ok()
        .map(|s| Decoded::Text(sanitize(s)))
}

fn sanitize_json(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::String(s) => Value::String(sanitize(&s)),
        Value::Array(items) => Value::Array(items.into_iter().map(sanitize_json).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (sanitize(&k), sanitize_json(v)))
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use flatbuffers_reflection::reflection::BaseType;

    use super::*;
    use crate::raw::walk::short_name;

    /// A regenerated schema that adds an object-ID field must also say which
    /// files it points at.
    #[test]
    fn every_object_id12_field_links_somewhere() {
        let no_paths = HashMap::new();
        let annotator = Annotator::new(&no_paths, None);
        for kind in [
            FileKind::RepoInfo,
            FileKind::Snapshot,
            FileKind::Manifest,
            FileKind::TransactionLog,
        ] {
            let schema = kind.schema().unwrap();
            let objects = schema.objects();
            for obj in objects.iter() {
                for field in obj.fields().iter() {
                    let ty = field.type_();
                    let is_obj = ty.base_type() == BaseType::Obj
                        || (ty.base_type() == BaseType::Vector && ty.element() == BaseType::Obj);
                    if !is_obj
                        || short_name(objects.get(ty.index() as usize).name()) != "ObjectId12"
                    {
                        continue;
                    }
                    let owner = short_name(obj.name());
                    let value = annotator.object_id(owner, field.name(), &[0; 12]);
                    assert!(
                        matches!(&value, RawValue::Id { links, .. } if !links.is_empty()),
                        "{owner}.{} has no link target in annotate.rs",
                        field.name()
                    );
                }
            }
        }
    }
}
