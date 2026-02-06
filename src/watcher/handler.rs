use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{Config, Event, EventKindMask, RecommendedWatcher, RecursiveMode, Watcher};

use crate::index::format::{FileEntry, IndexManifest};
use crate::index::writer::write_index;
use crate::parser::languages::detect_language;
use crate::parser::treesitter::parse_file;
use crate::scanner::hasher::hash_bytes;
use crate::server::db::SearchDb;

const DEBOUNCE_DELAY: Duration = Duration::from_millis(500);
const FLUSH_DELAY: Duration = Duration::from_secs(5);

/// Start watching the given directory for file changes and trigger
/// re-indexing when files are modified.
pub fn start_watcher(root: PathBuf, db: Arc<Mutex<SearchDb>>) -> Result<()> {
    let root_canonical = root
        .canonicalize()
        .with_context(|| format!("cannot resolve path: {}", root.display()))?;

    // Build gitignore matcher for the root
    let gitignore = load_gitignore(&root_canonical)?;

    tracing::info!("starting file watcher on {}", root_canonical.display());

    // Channel for receiving events
    let (tx, rx) = std::sync::mpsc::channel();

    // Create watcher with CORE mask (excludes ACCESS/OPEN events)
    let config = Config::default().with_event_kinds(EventKindMask::CORE);
    let mut watcher: RecommendedWatcher = Watcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        config,
    )
    .context("failed to create file watcher")?;

    // Watch root recursively
    watcher
        .watch(&root_canonical, RecursiveMode::Recursive)
        .context("failed to watch directory")?;

    tracing::info!("file watcher ready (recursive mode)");

    // Debounce state: path -> last event time
    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
    let mut last_flush = Instant::now();
    let mut dirty = false;

    loop {
        // Wait for events with timeout
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => {
                let now = Instant::now();
                for path in event.paths {
                    pending.insert(path, now);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Check for debounced events ready to process
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                if dirty {
                    let _ = flush_index_to_disk(&root_canonical, &db);
                }
                tracing::info!("watcher channel closed, shutting down");
                break;
            }
        }

        // Process debounced events
        let now = Instant::now();
        let ready: Vec<PathBuf> = pending
            .iter()
            .filter(|&(_, time)| now.duration_since(*time) >= DEBOUNCE_DELAY)
            .map(|(path, _)| path.clone())
            .collect();

        if !ready.is_empty() {
            for path in &ready {
                pending.remove(path);
            }

            if let Err(e) = handle_events(&ready, &root_canonical, &db, &gitignore) {
                tracing::error!("error handling watch events: {}", e);
            } else {
                dirty = true;
            }
        }

        // Flush to disk after quiet period
        if dirty && now.duration_since(last_flush) >= FLUSH_DELAY && pending.is_empty() {
            if let Err(e) = flush_index_to_disk(&root_canonical, &db) {
                tracing::error!("failed to flush index to disk: {}", e);
            }
            dirty = false;
            last_flush = now;
        }
    }

    Ok(())
}

/// Handle a batch of file system events.
fn handle_events(
    paths: &[PathBuf],
    root: &Path,
    db: &Arc<Mutex<SearchDb>>,
    gitignore: &Gitignore,
) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }

    tracing::debug!("processing {} file events", paths.len());

    let mut changed_files = Vec::new();

    for path in paths {
        // Compute relative path
        let rel_path = match path.strip_prefix(root) {
            Ok(p) => p.to_string_lossy().replace('\\', "/"),
            Err(_) => continue, // Not under root
        };

        // Skip .codeindex/ directory
        if rel_path.starts_with(".codeindex/") || rel_path == ".codeindex" {
            continue;
        }

        // Skip hidden files (dotfiles like .mcp.json)
        if rel_path.starts_with('.') {
            continue;
        }

        // Skip if gitignored
        if gitignore.matched(&rel_path, path.is_dir()).is_ignore() {
            continue;
        }

        // Skip directories
        if path.is_dir() {
            continue;
        }

        // Check if file still exists
        if !path.exists() {
            // File was deleted
            tracing::debug!("file deleted: {}", rel_path);
            let db_guard = db
                .lock()
                .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
            if let Err(e) = db_guard.remove_file(&rel_path) {
                tracing::warn!("failed to remove file {}: {}", rel_path, e);
            }
            continue;
        }

        // File was created or modified
        changed_files.push((path.to_path_buf(), rel_path));
    }

    // Process changed files
    for (abs_path, rel_path) in changed_files {
        if let Err(e) = process_file_change(&abs_path, &rel_path, db) {
            tracing::warn!("failed to process file {}: {}", rel_path, e);
        }
    }

    // Rebuild FTS indexes once after all changes
    let db_guard = db
        .lock()
        .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
    db_guard.rebuild_fts()?;

    Ok(())
}

/// Process a single file change (create or modify).
pub fn process_file_change(
    abs_path: &Path,
    rel_path: &str,
    db: &Arc<Mutex<SearchDb>>,
) -> Result<()> {
    // Read file content once
    let content =
        std::fs::read(abs_path).with_context(|| format!("failed to read {}", rel_path))?;

    // Hash the content
    let new_hash = hash_bytes(&content);

    // Check if hash changed
    let db_guard = db
        .lock()
        .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
    if let Some(old_hash) = db_guard.get_file_hash(rel_path)?
        && old_hash == new_hash
    {
        // No change, skip
        return Ok(());
    }
    drop(db_guard);

    tracing::debug!("indexing file: {}", rel_path);

    // Count lines
    let line_count = count_lines(&content);

    // Detect language
    let lang = abs_path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(detect_language)
        .map(String::from);

    let mut symbols = Vec::new();
    let mut texts = Vec::new();

    // Parse source files for symbols and texts
    if let Some(ref lang_name) = lang {
        match parse_file(&content, lang_name, rel_path) {
            Ok((file_symbols, file_texts)) => {
                symbols = file_symbols;
                texts = file_texts;
            }
            Err(e) => {
                tracing::warn!("failed to parse {}: {}", rel_path, e);
            }
        }
    }

    let file_entry = FileEntry {
        path: rel_path.to_string(),
        lang,
        hash: new_hash,
        lines: line_count,
    };

    // Upsert into database
    let db_guard = db
        .lock()
        .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
    db_guard.upsert_file(&file_entry, &symbols, &texts)?;

    Ok(())
}

/// Flush the entire index from memory to disk.
pub fn flush_index_to_disk(root: &Path, db: &Arc<Mutex<SearchDb>>) -> Result<()> {
    let db_guard = db
        .lock()
        .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
    let (files, symbols, texts) = db_guard.export_all()?;
    drop(db_guard);

    // Collect languages
    let mut languages: BTreeSet<String> = BTreeSet::new();
    for f in &files {
        if let Some(ref lang) = f.lang {
            languages.insert(lang.clone());
        }
    }

    // Derive project name from directory name
    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let manifest = IndexManifest {
        version: "1.0".to_string(),
        name,
        root: ".".to_string(),
        languages: languages.into_iter().collect(),
    };

    let output_dir = root.join(".codeindex");
    write_index(&output_dir, &manifest, &files, &symbols, &texts)?;

    tracing::debug!(
        "flushed index to disk: {} files, {} symbols, {} texts",
        files.len(),
        symbols.len(),
        texts.len()
    );

    Ok(())
}

/// Load .gitignore rules from the root directory.
fn load_gitignore(root: &Path) -> Result<Gitignore> {
    let gitignore_path = root.join(".gitignore");
    if gitignore_path.exists() {
        let mut builder = GitignoreBuilder::new(root);
        builder.add(&gitignore_path);
        builder.build().context("failed to build gitignore")
    } else {
        Ok(Gitignore::empty())
    }
}

/// Count the number of lines in a byte buffer.
fn count_lines(content: &[u8]) -> u32 {
    if content.is_empty() {
        return 0;
    }
    let count = content.iter().filter(|&&b| b == b'\n').count() as u32;
    // If file doesn't end with newline, the last line still counts
    if content.last() != Some(&b'\n') {
        count + 1
    } else {
        count
    }
}
