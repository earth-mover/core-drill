//! Raw inspection of Icechunk metadata files as the flatbuffers they are.
//!
//! Decoding is driven by the binary schemas in `schema/*.bfbs` (compiled from
//! the Icechunk `.fbs` files by `scripts/gen-schema.sh`), so every field shows
//! up with its real name and type, including fields newer than the icechunk
//! crate this binary links against. Pure functions only; fetching lives in
//! `fetch::raw`.

mod annotate;
mod header;
mod lookup;
mod value;
mod walk;

use std::collections::HashMap;
use std::sync::OnceLock;

use color_eyre::eyre::{Result, eyre};
use flatbuffers_reflection::reflection::{Schema, root_as_schema};
use icechunk::format::format_constants::{FileTypeBin, SpecVersionBin};
use icechunk::format::manifest::Manifest;
use icechunk::format::repo_info::RepoInfo;
use icechunk::format::snapshot::Snapshot;
use icechunk::format::transaction_log::TransactionLog;
use icechunk::format::{
    CHUNKS_FILE_PATH, MANIFESTS_FILE_PATH, OVERWRITTEN_FILES_PATH, REPO_INFO_FILE_PATH,
    SNAPSHOTS_FILE_PATH, TRANSACTION_LOGS_FILE_PATH,
};
use serde::Serialize;
use zstd::dict::DecoderDictionary;

pub use header::FileHeader;
pub use value::{Decoded, RawValue, hex};

use annotate::Annotator;
use walk::{Seg, Walker, parse_selector};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FileKind {
    RepoInfo,
    Snapshot,
    Manifest,
    TransactionLog,
    Chunk,
}

impl FileKind {
    /// Directory holding this kind of file (`repo` is a single file at the root;
    /// superseded copies of it live in `overwritten/`).
    pub fn dir(self) -> &'static str {
        match self {
            FileKind::RepoInfo => REPO_INFO_FILE_PATH,
            FileKind::Snapshot => SNAPSHOTS_FILE_PATH,
            FileKind::Manifest => MANIFESTS_FILE_PATH,
            FileKind::TransactionLog => TRANSACTION_LOGS_FILE_PATH,
            FileKind::Chunk => CHUNKS_FILE_PATH,
        }
    }

    /// Kind implied by a repo-relative object path.
    pub fn from_path(path: &str) -> Option<FileKind> {
        let dir = path.split('/').next().unwrap_or(path);
        if dir == OVERWRITTEN_FILES_PATH {
            return Some(FileKind::RepoInfo);
        }
        [
            FileKind::RepoInfo,
            FileKind::Snapshot,
            FileKind::Manifest,
            FileKind::TransactionLog,
            FileKind::Chunk,
        ]
        .into_iter()
        .find(|k| k.dir() == dir)
    }

    fn from_header(file_type: u8) -> Option<FileKind> {
        Some(match FileTypeBin::try_from(file_type).ok()? {
            FileTypeBin::Snapshot => FileKind::Snapshot,
            FileTypeBin::Manifest => FileKind::Manifest,
            FileTypeBin::TransactionLog => FileKind::TransactionLog,
            FileTypeBin::Chunk => FileKind::Chunk,
            FileTypeBin::RepoInfo => FileKind::RepoInfo,
            FileTypeBin::Attributes => return None,
        })
    }

    fn schema(self) -> Option<Schema<'static>> {
        static REPO: OnceLock<Schema<'static>> = OnceLock::new();
        static SNAPSHOT: OnceLock<Schema<'static>> = OnceLock::new();
        static MANIFEST: OnceLock<Schema<'static>> = OnceLock::new();
        static TRANSACTION_LOG: OnceLock<Schema<'static>> = OnceLock::new();
        let (cell, bytes): (_, &'static [u8]) = match self {
            FileKind::RepoInfo => (&REPO, include_bytes!("../../schema/repo.bfbs")),
            FileKind::Snapshot => (&SNAPSHOT, include_bytes!("../../schema/snapshot.bfbs")),
            FileKind::Manifest => (&MANIFEST, include_bytes!("../../schema/manifest.bfbs")),
            FileKind::TransactionLog => (
                &TRANSACTION_LOG,
                include_bytes!("../../schema/transaction_log.bfbs"),
            ),
            FileKind::Chunk => return None,
        };
        Some(*cell.get_or_init(|| root_as_schema(bytes).expect("embedded schema is valid")))
    }
}

#[derive(Default)]
pub struct DecodeOptions {
    /// Path into the value tree, e.g. `arrays/0/refs/10..20`.
    pub selector: Option<String>,
    /// Elements shown per vector of tables; 0 means all.
    pub max_items: usize,
    /// Node ID → path, used to label node IDs in manifests and transaction logs.
    pub node_paths: HashMap<[u8; 8], String>,
}

enum Body {
    Bytes(Vec<u8>),
    /// A manifest icechunk accepted, kept typed for chunk payload lookups.
    Manifest(Manifest),
}

/// A fetched file with its header parsed and its body decompressed and
/// verified, ready to walk any number of times.
pub struct OpenedFile {
    pub path: String,
    pub kind: FileKind,
    pub stored_bytes: usize,
    pub header: Option<FileHeader>,
    body: Body,
    /// `None` when icechunk accepts the flatbuffer.
    pub verify_error: Option<String>,
    location_dictionary: Option<DecoderDictionary<'static>>,
}

/// A decoded metadata file (or the selected part of one).
#[derive(Debug, Clone, Serialize)]
pub struct RawObject {
    pub path: String,
    pub kind: FileKind,
    pub stored_bytes: usize,
    pub header: Option<FileHeader>,
    pub decoded_bytes: Option<usize>,
    pub verify_error: Option<String>,
    pub selector: Option<String>,
    /// Selector of the vector a window hint should extend: the selector with a
    /// trailing range removed.
    #[serde(skip)]
    pub window_base: String,
    pub value: RawValue,
}

pub fn open(path: &str, bytes: Vec<u8>) -> Result<OpenedFile> {
    let header = header::parse_header(&bytes);
    let kind = header
        .as_ref()
        .and_then(|h| FileKind::from_header(h.file_type))
        .or_else(|| FileKind::from_path(path))
        .ok_or_else(|| eyre!("cannot tell what kind of file '{path}' is"))?;
    let stored_bytes = bytes.len();
    let (body, verify_error) = match &header {
        Some(h) if kind != FileKind::Chunk => verify(kind, h, &bytes)?,
        _ => (Body::Bytes(bytes), None),
    };
    let mut file = OpenedFile {
        path: path.to_string(),
        kind,
        stored_bytes,
        header,
        body,
        verify_error,
        location_dictionary: None,
    };
    if let (FileKind::Manifest, Some(schema)) = (kind, kind.schema()) {
        let no_paths = HashMap::new();
        let annotator = Annotator::new(&no_paths, None);
        let walker = Walker::new(file.body(), schema, &annotator, 0);
        let (root, loc) = walker.root_table()?;
        file.location_dictionary = walker
            .bytes_field(root, loc, "location_dictionary")?
            .map(DecoderDictionary::copy);
    }
    Ok(file)
}

/// Decompress the body and hand it to icechunk's own constructor, so a
/// verification failure means icechunk would reject the file. The walker
/// bounds-checks every read on its own; this only reports.
/// (`flatbuffers-reflection`'s schema verifier is not used because it panics
/// on out-of-range union tags.)
fn verify(kind: FileKind, header: &FileHeader, stored: &[u8]) -> Result<(Body, Option<String>)> {
    let body = header::decompress_body(header, stored)?;
    let result = match kind {
        FileKind::Manifest => {
            // Manifests can be large: hand over the buffer rather than cloning
            // it, and decompress again only in the rare rejected case.
            return Ok(match Manifest::from_buffer(body) {
                Ok(m) => (Body::Manifest(m), None),
                Err(e) => (
                    Body::Bytes(header::decompress_body(header, stored)?),
                    Some(e.to_string()),
                ),
            });
        }
        FileKind::Snapshot => SpecVersionBin::try_from(header.spec_version)
            .map_err(|e| e.to_string())
            .and_then(|spec| {
                Snapshot::from_buffer(spec, body.clone())
                    .map(drop)
                    .map_err(|e| e.to_string())
            }),
        FileKind::TransactionLog => TransactionLog::from_buffer(body.clone())
            .map(drop)
            .map_err(|e| e.to_string()),
        FileKind::RepoInfo => RepoInfo::from_buffer(body.clone())
            .map(drop)
            .map_err(|e| e.to_string()),
        FileKind::Chunk => Ok(()),
    };
    Ok((Body::Bytes(body), result.err()))
}

impl OpenedFile {
    /// Decompressed flatbuffer, or the raw bytes for chunk files.
    pub fn body(&self) -> &[u8] {
        match &self.body {
            Body::Bytes(b) => b,
            Body::Manifest(m) => m.bytes(),
        }
    }

    pub fn manifest(&self) -> Option<&Manifest> {
        match &self.body {
            Body::Manifest(m) => Some(m),
            Body::Bytes(_) => None,
        }
    }

    pub fn walk(&self, opts: DecodeOptions) -> Result<RawObject> {
        let sel = opts
            .selector
            .as_deref()
            .map(parse_selector)
            .transpose()?
            .unwrap_or_default();
        let selector = (!sel.is_empty()).then(|| join(&sel));
        let window_base = match sel.split_last() {
            Some((Seg::Range(..), head)) => join(head),
            _ => selector.clone().unwrap_or_default(),
        };
        let annotator = Annotator::new(&opts.node_paths, self.location_dictionary.as_ref());
        let value = self.walk_with(&annotator, opts.max_items, &sel)?;
        Ok(RawObject {
            path: self.path.clone(),
            kind: self.kind,
            stored_bytes: self.stored_bytes,
            header: self.header.clone(),
            decoded_bytes: self.header.as_ref().map(|_| self.body().len()),
            verify_error: self.verify_error.clone(),
            selector,
            window_base,
            value,
        })
    }

    /// Walk the subtree at `sel` without display annotations (node paths,
    /// decompressed locations), for cheap repeated probes.
    fn select(&self, sel: &[Seg]) -> Result<RawValue> {
        let no_paths = HashMap::new();
        self.walk_with(&Annotator::new(&no_paths, None), 0, sel)
    }

    fn walk_with(&self, annotator: &Annotator, max_items: usize, sel: &[Seg]) -> Result<RawValue> {
        match (self.kind.schema(), &self.header) {
            (Some(schema), Some(_)) => {
                Walker::new(self.body(), schema, annotator, max_items).walk_root(sel)
            }
            _ => Ok(annotator.bytes("", "", self.body())),
        }
    }
}

fn join(segs: &[Seg]) -> String {
    segs.iter()
        .map(Seg::to_string)
        .collect::<Vec<_>>()
        .join("/")
}
