use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

use crate::cli::build::build_index_to_db;
use crate::index::reader::read_index;
use crate::server::db::SearchDb;
use crate::server::mcp::start_server;
use crate::watcher::handler::{flush_index_to_disk, start_watcher};

/// Run the `serve` subcommand: load the index into an in-memory SQLite FTS5
/// database and start the MCP server over stdio.
pub fn run(path: &Path, watch: bool) -> Result<()> {
    let root = path
        .canonicalize()
        .with_context(|| format!("cannot resolve path: {}", path.display()))?;
    let index_dir = root.join(".codeindex");

    let db = if !index_dir.is_dir() {
        // No index exists - build directly into memory and flush to disk
        tracing::info!("no .codeindex/ found, building initial index");
        let db = build_index_to_db(path).context("failed to build initial index")?;
        flush_index_to_disk(&root, &db).context("failed to flush initial index")?;

        // Log stats
        let db_guard = db.lock().map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
        let (files, symbols, texts) = db_guard.export_all()?;
        drop(db_guard);
        tracing::info!(
            files = files.len(),
            symbols = symbols.len(),
            texts = texts.len(),
            "built and loaded index"
        );

        db
    } else {
        // Index exists - read from disk and load into memory
        let (manifest, files, symbols, texts) =
            read_index(&index_dir).context("failed to read index")?;

        tracing::info!(
            name = %manifest.name,
            files = files.len(),
            symbols = symbols.len(),
            texts = texts.len(),
            "loaded index"
        );

        let db = SearchDb::new().context("failed to create search database")?;
        db.load(&files, &symbols, &texts)
            .context("failed to load index into search database")?;

        tracing::info!("search database ready");
        Arc::new(Mutex::new(db))
    };

    // Start the tokio runtime
    let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;

    if watch {
        tracing::info!("starting watch mode");
        let path_clone = path.to_path_buf();
        let db_clone = Arc::clone(&db);

        // Spawn watcher in background thread
        std::thread::spawn(move || {
            if let Err(e) = start_watcher(path_clone, db_clone) {
                tracing::error!("watcher error: {}", e);
            }
        });
    }

    rt.block_on(async {
        tracing::info!("starting MCP server on stdio");
        start_server(db).await
    })
}
