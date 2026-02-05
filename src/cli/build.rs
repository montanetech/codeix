use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tracing::info;

use crate::scanner::walker::walk_directory;
use crate::server::db::SearchDb;
use crate::watcher::handler::{flush_index_to_disk, process_file_change};

/// Build the index into a database without flushing to disk.
/// Returns the populated database for immediate use.
pub fn build_index_to_db(path: &Path) -> Result<Arc<Mutex<SearchDb>>> {
    let root = path
        .canonicalize()
        .with_context(|| format!("cannot resolve path: {}", path.display()))?;

    // Derive project name from directory name
    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    info!("building index for '{}' at {}", name, root.display());

    // Create empty database
    let db = SearchDb::new().context("failed to create search database")?;
    let db = Arc::new(Mutex::new(db));

    // Walk directory to find all files
    let all_paths = walk_directory(&root)?;
    let total = all_paths.len();
    info!("found {} files", total);

    let show_progress = std::io::stderr().is_terminal();
    let mut count = 0usize;

    // Process each file through the same path as the watcher
    for abs_path in &all_paths {
        // Compute relative path (forward slashes, no leading ./)
        let rel_path = abs_path
            .strip_prefix(&root)
            .unwrap_or(abs_path)
            .to_string_lossy()
            .replace('\\', "/");

        // Skip .codeindex/ directory itself
        if rel_path.starts_with(".codeindex/") || rel_path == ".codeindex" {
            continue;
        }

        count += 1;

        if show_progress {
            eprint!("\r  indexing [{count}/{total}] {rel_path}");
            // Clear any trailing characters from previous longer line
            eprint!("\x1b[K");
            let _ = std::io::stderr().flush();
        }

        // Process file using shared indexing logic
        if let Err(e) = process_file_change(abs_path, &rel_path, &db) {
            // Log errors but continue with other files
            tracing::warn!("failed to process {}: {}", rel_path, e);
        }
    }

    if show_progress {
        eprintln!();
    }

    // Single FTS rebuild after all files
    let db_guard = db
        .lock()
        .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
    db_guard.rebuild_fts()?;
    drop(db_guard);

    Ok(db)
}

/// Build the index: scan the directory tree, parse files with tree-sitter,
/// and write the `.codeindex/` output.
pub fn build_index(path: &Path) -> Result<()> {
    let root = path
        .canonicalize()
        .with_context(|| format!("cannot resolve path: {}", path.display()))?;

    let db = build_index_to_db(path)?;

    // Flush to disk using shared logic
    flush_index_to_disk(&root, &db).context("failed to flush index to disk")?;

    // Log final stats
    let db_guard = db
        .lock()
        .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
    let (files, symbols, texts) = db_guard.export_all()?;
    drop(db_guard);

    info!(
        "wrote .codeindex/: {} files, {} symbols, {} texts",
        files.len(),
        symbols.len(),
        texts.len()
    );

    Ok(())
}

/// Run the `build` subcommand: scan the directory tree, parse files with
/// tree-sitter, and write the `.codeindex/` output.
pub fn run(path: &Path) -> Result<()> {
    build_index(path)
}
