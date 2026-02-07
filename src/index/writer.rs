use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::Result;

use super::format::{FileEntry, IndexManifest, ReferenceEntry, SymbolEntry, TextEntry};

/// Write a complete `.codeindex/` directory to disk.
///
/// Note: Data is expected to be pre-sorted from the database export
/// (files by path, symbols/texts by file then line). This avoids
/// memory-intensive copies for large repositories.
pub fn write_index(
    output_dir: &Path,
    manifest: &IndexManifest,
    files: &[FileEntry],
    symbols: &[SymbolEntry],
    texts: &[TextEntry],
    references: &[ReferenceEntry],
) -> Result<()> {
    // Create the output directory
    fs::create_dir_all(output_dir)?;

    // Write index.json (pretty-printed)
    let index_path = output_dir.join("index.json");
    let json = serde_json::to_string_pretty(manifest)?;
    fs::write(&index_path, json.as_bytes())?;

    // Write JSONL files directly - data is pre-sorted from DB export
    write_jsonl(&output_dir.join("files.jsonl"), files)?;
    write_jsonl(&output_dir.join("symbols.jsonl"), symbols)?;
    write_jsonl(&output_dir.join("texts.jsonl"), texts)?;
    write_jsonl(&output_dir.join("references.jsonl"), references)?;

    Ok(())
}

/// Write a slice of serializable items as JSONL (one JSON object per line).
fn write_jsonl<T: serde::Serialize>(path: &Path, items: &[T]) -> Result<()> {
    let file = fs::File::create(path)?;
    let mut writer = BufWriter::new(file);

    for item in items {
        serde_json::to_writer(&mut writer, item)?;
        writer.write_all(b"\n")?;
    }

    writer.flush()?;
    Ok(())
}
