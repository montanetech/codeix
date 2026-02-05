use std::path::Path;

use anyhow::Result;

/// Compute the BLAKE3 hash of a file's contents, truncated to 64 bits,
/// returned as a 16-character hex string.
pub fn hash_file(path: &Path) -> Result<String> {
    let contents = std::fs::read(path)?;
    Ok(hash_bytes(&contents))
}

/// Compute the BLAKE3 hash of byte content, truncated to 64 bits,
/// returned as a 16-character hex string.
pub fn hash_bytes(content: &[u8]) -> String {
    let hash = blake3::hash(content);
    let bytes = hash.as_bytes();
    // Truncate to first 8 bytes (64 bits), hex-encode to 16 chars
    bytes[..8].iter().map(|b| format!("{b:02x}")).collect()
}
