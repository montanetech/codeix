use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;

use anyhow::{Context, Result};

use crate::cli::build::build_index_to_db;
use crate::mount::MountedEvent;
use crate::mount::handler::{flush_mount_to_disk, run_event_loop};
use crate::server::mcp::start_server;

/// Run the `serve` subcommand: load the index into an in-memory SQLite FTS5
/// database and start the MCP server over stdio.
pub fn run(path: &Path, watch: bool) -> Result<()> {
    let _root = path
        .canonicalize()
        .with_context(|| format!("cannot resolve path: {}", path.display()))?;

    // If watch mode: create channel BEFORE building
    // This way directories are watched during the single walk (no second walk needed)
    // Channel carries (mount_root, event) tuples for direct mount lookup
    let (tx, rx): (
        Option<mpsc::Sender<MountedEvent>>,
        Option<mpsc::Receiver<MountedEvent>>,
    ) = if watch {
        tracing::info!("starting watch mode");
        let (tx, rx) = mpsc::channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    // Build index with FTS enabled (loads from .codeindex/ if exists, otherwise parses files)
    // Serve mode needs FTS for search functionality
    // load_from_cache=true: load from .codeindex/ if available
    // Pass tx to initialize notify watchers during walk (single walk strategy)
    let (mount_table, db) =
        build_index_to_db(path, true, true, tx.clone()).context("failed to build/load index")?;

    // Flush any dirty mounts to disk (projects that were indexed, not loaded)
    {
        let mt = mount_table
            .lock()
            .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;
        for (mount_root, mount) in mt.iter() {
            if mount.dirty {
                flush_mount_to_disk(mount_root, &mt, &db)
                    .with_context(|| format!("failed to flush {}", mount_root.display()))?;
            }
        }
    }

    // Spawn event loop AFTER build (needs mount_table and db)
    // But notify watchers are already initialized and watching during build
    if let (Some(tx), Some(rx)) = (tx, rx) {
        let mount_table_clone = Arc::clone(&mount_table);
        let db_clone = Arc::clone(&db);

        std::thread::spawn(move || {
            if let Err(e) = run_event_loop(rx, tx, mount_table_clone, db_clone) {
                tracing::error!("event loop error: {}", e);
            }
        });
    }

    // Start the tokio runtime
    let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;

    rt.block_on(async {
        tracing::info!("starting MCP server on stdio");
        start_server(db, mount_table).await
    })
}
