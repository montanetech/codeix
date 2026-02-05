use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::Result;

use super::format::{FileEntry, IndexManifest, SymbolEntry, TextEntry};

/// Write a complete `.codeindex/` directory to disk.
pub fn write_index(
    output_dir: &Path,
    manifest: &IndexManifest,
    files: &[FileEntry],
    symbols: &[SymbolEntry],
    texts: &[TextEntry],
) -> Result<()> {
    // Create the output directory
    fs::create_dir_all(output_dir)?;

    // Write index.json (pretty-printed)
    let index_path = output_dir.join("index.json");
    let json = serde_json::to_string_pretty(manifest)?;
    fs::write(&index_path, json.as_bytes())?;

    // Write files.jsonl — sorted by path
    let mut sorted_files = files.to_vec();
    sorted_files.sort_by(|a, b| a.path.cmp(&b.path));
    write_jsonl(&output_dir.join("files.jsonl"), &sorted_files)?;

    // Write symbols.jsonl — sorted by file, then line[0]
    let mut sorted_symbols = symbols.to_vec();
    sorted_symbols.sort_by(|a, b| a.file.cmp(&b.file).then(a.line[0].cmp(&b.line[0])));
    write_jsonl(&output_dir.join("symbols.jsonl"), &sorted_symbols)?;

    // Write texts.jsonl — sorted by file, then line[0]
    let mut sorted_texts = texts.to_vec();
    sorted_texts.sort_by(|a, b| a.file.cmp(&b.file).then(a.line[0].cmp(&b.line[0])));
    write_jsonl(&output_dir.join("texts.jsonl"), &sorted_texts)?;

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
