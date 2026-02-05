use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};

use super::format::{FileEntry, IndexManifest, SymbolEntry, TextEntry};

/// Convenience alias for the tuple returned by `read_index`.
pub type IndexData = (
    IndexManifest,
    Vec<FileEntry>,
    Vec<SymbolEntry>,
    Vec<TextEntry>,
);

/// Read an existing `.codeindex/` directory from disk.
/// Returns the manifest, files, symbols, and texts.
pub fn read_index(path: &Path) -> Result<IndexData> {
    let manifest: IndexManifest = {
        let data =
            fs::read_to_string(path.join("index.json")).context("failed to read index.json")?;
        serde_json::from_str(&data).context("failed to parse index.json")?
    };

    let files = read_jsonl(&path.join("files.jsonl")).context("failed to read files.jsonl")?;

    let symbols =
        read_jsonl(&path.join("symbols.jsonl")).context("failed to read symbols.jsonl")?;

    let texts = read_jsonl(&path.join("texts.jsonl")).context("failed to read texts.jsonl")?;

    Ok((manifest, files, symbols, texts))
}

/// Read a JSONL file into a Vec of deserialized items.
fn read_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut items = Vec::new();

    for (i, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let item: T = serde_json::from_str(&line)
            .with_context(|| format!("failed to parse line {} of {}", i + 1, path.display()))?;
        items.push(item);
    }

    Ok(items)
}
