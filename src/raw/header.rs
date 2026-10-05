//! Icechunk binary file header and body decompression.

use std::io::Read;

use color_eyre::eyre::{Result, bail, eyre};
use icechunk::format::format_constants::{
    CompressionAlgorithmBin, FileTypeBin, ICECHUNK_FORMAT_MAGIC_BYTES,
};
use serde::Serialize;

pub const HEADER_LEN: usize = 39;

/// Flatbuffers rejects buffers over 2 GiB, so a body that decompresses past
/// that is either corrupt or a zip bomb.
const MAX_DECOMPRESSED: u64 = 1 << 31;

#[derive(Debug, Clone, Serialize)]
pub struct FileHeader {
    pub implementation: String,
    pub spec_version: u8,
    pub file_type: u8,
    pub file_type_name: String,
    pub compression: u8,
    pub compression_name: String,
}

/// Parse the 39-byte header: magic (12), implementation name (24),
/// spec version, file type, compression. Returns `None` for files without
/// the magic prefix (chunk files).
pub fn parse_header(bytes: &[u8]) -> Option<FileHeader> {
    if bytes.len() < HEADER_LEN || !bytes.starts_with(ICECHUNK_FORMAT_MAGIC_BYTES) {
        return None;
    }
    let implementation = String::from_utf8_lossy(&bytes[12..36])
        .trim_end()
        .to_string();
    let (file_type, compression) = (bytes[37], bytes[38]);
    let unknown = |_| "unknown".to_string();
    Some(FileHeader {
        implementation: crate::sanitize::sanitize(&implementation),
        spec_version: bytes[36],
        file_type,
        file_type_name: FileTypeBin::try_from(file_type).map_or_else(unknown, |t| format!("{t:?}")),
        compression,
        compression_name: CompressionAlgorithmBin::try_from(compression)
            .map_or_else(unknown, |c| format!("{c:?}").to_lowercase()),
    })
}

/// Return the flatbuffer body that follows the header, decompressed.
pub fn decompress_body(header: &FileHeader, bytes: &[u8]) -> Result<Vec<u8>> {
    let body = &bytes[HEADER_LEN..];
    match CompressionAlgorithmBin::try_from(header.compression) {
        Ok(CompressionAlgorithmBin::None) => Ok(body.to_vec()),
        Ok(CompressionAlgorithmBin::Zstd) => {
            let decoder = zstd::stream::read::Decoder::new(body)?;
            let mut out = Vec::new();
            decoder
                .take(MAX_DECOMPRESSED + 1)
                .read_to_end(&mut out)
                .map_err(|e| eyre!("zstd decompression failed: {e}"))?;
            if out.len() as u64 > MAX_DECOMPRESSED {
                bail!("decompressed body exceeds 2 GiB");
            }
            Ok(out)
        }
        Err(_) => bail!("unknown compression code {}", header.compression),
    }
}
