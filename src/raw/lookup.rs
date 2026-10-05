//! Binary search for a chunk ref inside a decoded manifest.
//!
//! Manifests sort `arrays` by node ID bytes and each array's `refs` by chunk
//! index, so a lookup is two binary searches. Probes go through the same
//! bounds-checked walker as display.

use std::cmp::Ordering;

use color_eyre::eyre::{Result, bail};

use super::annotate::encode_id;
use super::walk::Seg;
use super::{OpenedFile, RawValue};

fn field(name: &str) -> Seg {
    Seg::Field(name.to_string())
}

impl OpenedFile {
    fn vec_len(&self, path: &[Seg]) -> Result<usize> {
        let window = [path, &[Seg::Range(0, 0)]].concat();
        match self.select(&window)? {
            RawValue::Vector { len, .. } => Ok(len),
            _ => bail!("expected a vector of tables"),
        }
    }

    /// Find `(array index, ref index)` of the chunk ref for `coords` in node
    /// `node_id`'s array manifest.
    pub fn find_chunk_ref(
        &self,
        node_id: &[u8; 8],
        coords: &[u32],
    ) -> Result<Option<(usize, usize)>> {
        // Crockford base32 of equal-length inputs sorts like the raw bytes.
        let target = encode_id(node_id);
        let arrays = [field("arrays")];
        let array = binary_search(self.vec_len(&arrays)?, |i| {
            match self.select(&[field("arrays"), Seg::Index(i), field("node_id")])? {
                RawValue::NodeId { id, .. } => Ok(id.cmp(&target)),
                other => bail!("unexpected node_id value {other:?}"),
            }
        })?;
        let Some(array) = array else {
            return Ok(None);
        };
        let refs = [field("arrays"), Seg::Index(array), field("refs")];
        let found = binary_search(self.vec_len(&refs)?, |j| {
            let probe = [&refs[..], &[Seg::Index(j), field("index")]].concat();
            let RawValue::Scalars(index) = self.select(&probe)? else {
                bail!("unexpected chunk index value");
            };
            let index = index.iter().map(|v| match v {
                RawValue::UInt(u) => *u,
                _ => u64::MAX,
            });
            Ok(index.cmp(coords.iter().map(|&c| c as u64)))
        })?;
        Ok(found.map(|r| (array, r)))
    }
}

fn binary_search(
    len: usize,
    mut cmp: impl FnMut(usize) -> Result<Ordering>,
) -> Result<Option<usize>> {
    let (mut lo, mut hi) = (0, len);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match cmp(mid)? {
            Ordering::Equal => return Ok(Some(mid)),
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
        }
    }
    Ok(None)
}
