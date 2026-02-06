use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::cli::build::build_index_to_db;
use crate::server::mcp::start_server;
use crate::watcher::handler::{flush_mount_to_disk, start_watcher};

/// Run the `serve` subcommand: load the index into an in-memory SQLite FTS5
/// database and start the MCP server over stdio.
pub fn run(path: &Path, watch: bool) -> Result<()> {
    let root = path
        .canonicalize()
        .with_context(|| format!("cannot resolve path: {}", path.display()))?;

    // Build index (loads from .codeindex/ if exists, otherwise parses files)
    let (mount_table, db) = build_index_to_db(path).context("failed to build/load index")?;

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

    // Start the tokio runtime
    let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;

    if watch {
        tracing::info!("starting watch mode");
        let root_clone = root.clone();
        let mount_table_clone = Arc::clone(&mount_table);
        let db_clone = Arc::clone(&db);

        // Spawn watcher in background thread
        std::thread::spawn(move || {
            if let Err(e) = start_watcher(root_clone, mount_table_clone, db_clone) {
                tracing::error!("watcher error: {}", e);
            }
        });
    }

    rt.block_on(async {
        tracing::info!("starting MCP server on stdio");
        start_server(db).await
    })
}
