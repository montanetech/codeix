use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use notify::Event;
use tracing::info;

use crate::mount::MountTable;
use crate::server::db::SearchDb;
use crate::watcher::handler::{flush_mount_to_disk, on_project_discovery};

/// Result type for build_index_to_db: (MountTable, SearchDb)
pub type BuildResult = (Arc<Mutex<MountTable>>, Arc<Mutex<SearchDb>>);

/// Build the index into a database without flushing to disk.
/// Returns MountTable + SearchDb for both build (flush to disk) and serve (keep in memory).
///
/// Uses `on_project_discovery` which:
/// 1. Loads from .codeindex/ if it exists
/// 2. Otherwise indexes files (stopping at subproject boundaries)
/// 3. Recursively handles discovered subprojects
///
/// When `enable_fts` is true, creates FTS5 tables for search (serve mode).
/// When false, skips FTS to reduce memory on large repos (build mode).
///
/// When `tx` is provided, initializes notify watchers during walk so directories
/// are watched immediately (single walk strategy for serve --watch).
pub fn build_index_to_db(
    path: &Path,
    enable_fts: bool,
    tx: Option<Sender<Result<Event, notify::Error>>>,
) -> Result<BuildResult> {
    let root = path
        .canonicalize()
        .with_context(|| format!("cannot resolve path: {}", path.display()))?;

    info!("building index at {}", root.display());

    // Create mount table and database
    let mount_table = Arc::new(Mutex::new(MountTable::new(root.clone())));
    let db = Arc::new(Mutex::new(if enable_fts {
        SearchDb::new().context("failed to create search database")?
    } else {
        SearchDb::new_no_fts().context("failed to create search database")?
    }));

    // Process root project (will recursively discover and handle subprojects)
    // Pass tx to initialize notify watchers during walk (if provided)
    on_project_discovery(&root, &mount_table, &db, tx).context("failed to process root project")?;

    Ok((mount_table, db))
}

/// Build the index: scan the directory tree, parse files with tree-sitter,
/// and write the `.codeindex/` output.
///
/// Discovers .git/ boundaries and creates separate .codeindex/ for each
/// project found. Root is always treated as a project (with or without .git/).
pub fn build_index(path: &Path) -> Result<()> {
    // Build mode: disable FTS to reduce memory on large repos, no watcher
    let (mount_table, db) = build_index_to_db(path, false, None)?;

    // Flush each dirty mount to disk
    let mt = mount_table
        .lock()
        .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;

    let mut total_files = 0usize;
    let mut total_symbols = 0usize;
    let mut total_texts = 0usize;

    for (root, mount) in mt.iter() {
        if mount.dirty {
            flush_mount_to_disk(root, &mt, &db)
                .with_context(|| format!("failed to flush index to disk for {}", root.display()))?;

            // Get stats for this mount (use relative project path)
            let project_str = mt.relative_project(root);
            let db_guard = db
                .lock()
                .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
            let (files, symbols, texts, _refs) = db_guard.export_for_project(&project_str)?;
            drop(db_guard);

            let name = root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            info!(
                "wrote .codeindex/ for '{}': {} files, {} symbols, {} texts",
                name,
                files.len(),
                symbols.len(),
                texts.len()
            );

            total_files += files.len();
            total_symbols += symbols.len();
            total_texts += texts.len();
        }
    }

    info!(
        "total: {} files, {} symbols, {} texts",
        total_files, total_symbols, total_texts
    );

    Ok(())
}

/// Run the `build` subcommand: scan the directory tree, parse files with
/// tree-sitter, and write the `.codeindex/` output.
pub fn run(path: &Path) -> Result<()> {
    build_index(path)
}
