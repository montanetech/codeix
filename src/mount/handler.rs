use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use notify::event::EventKind;

use crate::index::format::{FileEntry, IndexManifest};
use crate::index::reader::read_index;
use crate::index::writer::write_index;
use crate::mount::{FsEvent, MountMode, MountTable, MountedEvent, is_removal_event};
use crate::parser::languages::detect_language;
use crate::parser::metadata::extract_file_metadata;
use crate::parser::treesitter::parse_file;
use crate::server::db::SearchDb;
use crate::utils::hasher::hash_bytes;

const DEBOUNCE_DELAY: Duration = Duration::from_millis(500);
const POLL_INTERVAL: Duration = Duration::from_millis(1000);
/// Trigger file name for external flush requests (e.g., from `codeix build` when server holds lock).
/// Written at project root (not inside .codeindex/) so inotify picks it up.
const FLUSH_TRIGGER_FILE: &str = ".codeindex.flush";
/// How long to wait for server to flush before timing out
const FLUSH_TIMEOUT: Duration = Duration::from_secs(30);
/// How often to poll for trigger file deletion
const FLUSH_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Run the main event loop for file watching.
///
/// Receives events from all mounts via `rx` (notify watchers already initialized).
/// Each event includes the mount root, avoiding the need for mount lookup.
/// Uses `tx` for passing to new project discoveries.
pub fn run_event_loop(
    rx: Receiver<MountedEvent>,
    tx: Sender<MountedEvent>,
    mount_table: Arc<Mutex<MountTable>>,
    db: Arc<Mutex<SearchDb>>,
) -> Result<()> {
    let total_watched = {
        let mt = mount_table
            .lock()
            .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;
        mt.iter().map(|(_, m)| m.watched_count()).sum::<usize>()
    };

    tracing::info!("event loop ready ({} directories watched)", total_watched);

    // Debounce state: path -> (last event time, event kind, mount root)
    let mut pending: HashMap<PathBuf, (Instant, EventKind, PathBuf)> = HashMap::new();

    loop {
        // Wait for events with timeout
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok((mount_root, Ok(event))) => {
                let now = Instant::now();
                for path in event.paths {
                    pending.insert(path, (now, event.kind, mount_root.clone()));
                }
            }
            Ok((_, Err(e))) => {
                tracing::warn!("notify error: {}", e);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Check for debounced events ready to process
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Flush all dirty mounts before shutting down
                let flushed = flush_dirty_mounts(&mount_table, &db)?;
                tracing::info!(
                    "event loop channel closed, flushed {} projects, shutting down",
                    flushed
                );
                break;
            }
        }

        // Process debounced events
        let now = Instant::now();
        let ready: Vec<(PathBuf, EventKind, PathBuf)> = pending
            .iter()
            .filter(|&(_, (time, _, _))| now.duration_since(*time) >= DEBOUNCE_DELAY)
            .map(|(path, (_, kind, mount_root))| (path.clone(), *kind, mount_root.clone()))
            .collect();

        if !ready.is_empty() {
            for (path, _, _) in &ready {
                pending.remove(path);
            }

            if let Err(e) = handle_events(&ready, &mount_table, &db, tx.clone()) {
                tracing::error!("error handling watch events: {}", e);
            }
        }

        // Note: Auto-flush disabled (issue #10). Use flush_index MCP tool to flush explicitly.
        // Mounts are still marked dirty and will be flushed on graceful shutdown.
        // External flush requests via .codeindex.flush are handled in handle_events().
    }

    Ok(())
}

/// Handle when a project is discovered (during walk or watch).
/// Mounts the project and loads/indexes it.
///
/// Parameters:
/// - `load_from_cache`: If true (serve mode), try loading from .codeindex/ first.
///   If false (build mode), always re-index
/// - `tx`: If provided, initializes file watcher during walk
///
/// Flow:
/// 1. If already mounted, skip
/// 2. Try mount RW, fall back to RO if lock is held
/// 3. In build mode with RO: request flush via trigger file and wait
/// 4. If load_from_cache && .codeindex/ exists, load from disk
/// 5. Otherwise, index files (walks and discovers subprojects)
pub fn on_project_discovery(
    project_root: &Path,
    mount_table: &Arc<Mutex<MountTable>>,
    db: &Arc<Mutex<SearchDb>>,
    load_from_cache: bool,
    tx: Option<Sender<MountedEvent>>,
) -> Result<()> {
    let mut mt = mount_table
        .lock()
        .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;

    // Check if already mounted (exact match, not prefix match)
    let canonical = project_root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {:?}", project_root))?;
    if mt.is_mounted(&canonical) {
        return Ok(());
    }

    // Mount the new project (tries RW, falls back to RO if lock held)
    let mount = mt.mount(project_root)?;
    let is_read_only = mount.mode == MountMode::ReadOnly;

    let project_name = project_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let project_str = mt.relative_project(project_root);
    let mode_str = if is_read_only { "RO" } else { "RW" };

    // In build mode (load_from_cache=false), RO means lock is held by another process.
    // Request flush via trigger file and wait for completion.
    if !load_from_cache && is_read_only {
        // Unmount since we won't use it
        let _ = mt.unmount(project_root);
        drop(mt);

        tracing::info!(
            "lock held by another process for '{}', requesting flush",
            project_name
        );
        return request_flush_and_wait(project_root);
    }

    drop(mt);

    // Try loading from .codeindex/ first (only if load_from_cache is true)
    let index_dir = project_root.join(".codeindex");
    if load_from_cache && index_dir.is_dir() {
        match read_index(&index_dir) {
            Ok((manifest, idx_files, idx_symbols, idx_texts, idx_refs)) => {
                let db_guard = db
                    .lock()
                    .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
                db_guard
                    .load(
                        &project_str,
                        &idx_files,
                        &idx_symbols,
                        &idx_texts,
                        &idx_refs,
                    )
                    .with_context(|| format!("failed to load index for '{}'", project_name))?;
                drop(db_guard);

                tracing::info!(
                    "loaded '{}' ({}) from .codeindex/: {} files, {} symbols, {} texts, {} refs",
                    manifest.name,
                    mode_str,
                    idx_files.len(),
                    idx_symbols.len(),
                    idx_texts.len(),
                    idx_refs.len()
                );

                // Walk to set up directory watches and discover subprojects.
                // Subprojects are loaded from their own .codeindex/ directories.
                // Even if tx=None (no watcher), we still need to discover subprojects.
                init_watchers_and_discover_subprojects(project_root, mount_table, db, tx)?;

                // Loaded from cache - no need to index files
                return Ok(());
            }
            Err(e) => {
                if is_read_only {
                    // Can't rebuild in RO mode, just warn
                    tracing::warn!(
                        "failed to read .codeindex/ for '{}' (read-only): {}",
                        project_name,
                        e
                    );
                    return Ok(());
                }
                tracing::warn!(
                    "failed to read .codeindex/ for '{}', rebuilding: {}",
                    project_name,
                    e
                );
                // Fall through to index files
            }
        }
    } else if is_read_only && load_from_cache {
        // No .codeindex/ and read-only in serve mode - nothing to do
        tracing::info!(
            "mounted '{}' ({}) - no .codeindex/, read-only",
            project_name,
            mode_str
        );
        return Ok(());
    }

    tracing::info!("indexing '{}' ({})", project_name, mode_str);

    // Walk and index all files in the new project (also discovers subprojects)
    walk_project(project_root, mount_table, db, load_from_cache, tx)?;

    Ok(())
}

/// Initialize file watchers and discover subprojects without re-indexing files.
/// Used when loading from cache in watch mode - we have the index but need watchers.
/// Subprojects are discovered and loaded from their own .codeindex/ directories.
fn init_watchers_and_discover_subprojects(
    project_root: &Path,
    mount_table: &Arc<Mutex<MountTable>>,
    db: &Arc<Mutex<SearchDb>>,
    tx: Option<Sender<MountedEvent>>,
) -> Result<()> {
    // Collect subprojects to process after releasing lock
    let mut subprojects: Vec<PathBuf> = Vec::new();

    {
        let mut mt = mount_table
            .lock()
            .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;

        let mount = mt
            .find_mount_mut(project_root)
            .ok_or_else(|| anyhow::anyhow!("no mount found for {}", project_root.display()))?;

        // Initialize watcher
        if let Some(ref tx) = tx {
            mount.init_notify(tx.clone())?;
        }

        // Walk to set up directory watches and discover subprojects (ignore file events)
        mount.walk(|event| {
            if let FsEvent::ProjectAdded { root } = event {
                subprojects.push(root);
            }
            Ok(())
        })?;
    } // MountTable lock released here

    // Process discovered subprojects - load from their own .codeindex/
    // Pass load_from_cache=true so subprojects also load from cache
    for root in &subprojects {
        if let Err(e) = on_project_discovery(root, mount_table, db, true, tx.clone()) {
            tracing::warn!("failed to load subproject {}: {}", root.display(), e);
        }
    }

    Ok(())
}

/// Walk a project directory, indexing files and discovering subprojects.
///
/// Uses Mount's `walk()` method which handles gitignore filtering and subproject discovery.
/// Parameters:
/// - `load_from_cache`: passed to recursive on_project_discovery calls for subprojects
/// - `tx`: If provided, initializes watcher and adds directories during walk
fn walk_project(
    project_root: &Path,
    mount_table: &Arc<Mutex<MountTable>>,
    db: &Arc<Mutex<SearchDb>>,
    load_from_cache: bool,
    tx: Option<Sender<MountedEvent>>,
) -> Result<()> {
    // Use relative project path from workspace root
    let project_str = {
        let mt = mount_table
            .lock()
            .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;
        mt.relative_project(project_root)
    };

    // Collect events first, then process them
    // This allows us to release the mount table lock before recursive calls
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    let mut subprojects: Vec<PathBuf> = Vec::new();

    {
        let mut mt = mount_table
            .lock()
            .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;

        let mount = mt
            .find_mount_mut(project_root)
            .ok_or_else(|| anyhow::anyhow!("no mount found for {}", project_root.display()))?;

        // Initialize watcher before walk (if tx provided)
        if let Some(ref tx) = tx {
            mount.init_notify(tx.clone())?;
        }

        // Walk the mount, collecting events
        // Watches are added internally by on_fs_event during walk
        mount.walk(|event| {
            match event {
                FsEvent::FileAdded { mount, path } => {
                    // Reconstruct abs_path from mount + path
                    let abs_path = mount.join(&path);
                    files.push((abs_path, path));
                }
                FsEvent::ProjectAdded { root } => {
                    subprojects.push(root);
                }
                FsEvent::FileRemoved { .. } | FsEvent::DirIgnored => {} // Not emitted during walk
            }
            Ok(())
        })?;
    } // MountTable lock released here

    // Process files
    let mut file_count = 0u32;
    for (abs_path, rel_path) in &files {
        file_count += 1;
        if file_count.is_multiple_of(100) {
            tracing::info!(
                "processed {} files so far for project '{}'",
                file_count,
                project_str
            );
        }
        if let Err(e) = process_file_change(abs_path, rel_path, &project_str, db) {
            tracing::warn!("failed to index {}: {}", rel_path, e);
        }
    }

    // Process subprojects (always - this is the single walk strategy)
    // Pass load_from_cache to recursive calls
    for root in &subprojects {
        if let Err(e) = on_project_discovery(root, mount_table, db, load_from_cache, tx.clone()) {
            tracing::warn!("failed to handle subproject {}: {}", root.display(), e);
        }
    }

    tracing::info!(
        "finished indexing project '{}': {} files, {} subprojects",
        project_str,
        file_count,
        subprojects.len()
    );

    // Rebuild FTS after batch indexing
    let db_guard = db
        .lock()
        .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
    db_guard.rebuild_fts()?;

    // Mark mount as dirty
    mount_table
        .lock()
        .ok()
        .map(|mut mt| mt.mark_dirty(project_root));

    Ok(())
}

/// Handle a batch of file system events.
///
/// All logic (gitignore, SKIP_ENTRIES, project detection, watches) is delegated
/// to `Mount::on_fs_event()` - the same rules apply for notify events as for walker.
///
/// Each event includes the mount root, so we can directly get the mount without lookup.
fn handle_events(
    events: &[(PathBuf, EventKind, PathBuf)],
    mount_table: &Arc<Mutex<MountTable>>,
    db: &Arc<Mutex<SearchDb>>,
    tx: Sender<MountedEvent>,
) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }

    tracing::debug!("processing {} file events", events.len());

    // Collect mount events to process
    let mut mount_events: Vec<FsEvent> = Vec::new();

    for (path, kind, mount_root) in events {
        // Check for flush trigger file (.codeindex.flush)
        if path.file_name().is_some_and(|n| n == FLUSH_TRIGGER_FILE) {
            if let Err(e) = handle_flush_trigger(path, mount_table, db) {
                tracing::error!("failed to handle flush trigger: {}", e);
            }
            continue;
        }

        // Early filter: skip .codeindex/ paths before expensive canonicalize()
        if path.components().any(|c| c.as_os_str() == ".codeindex") {
            continue;
        }

        // For file removal, the path may not exist anymore
        // For creation/modification, canonicalize to handle symlinks
        let canonical = if is_removal_event(kind) {
            // Use the path as-is for removal (can't canonicalize deleted files)
            path.clone()
        } else {
            match path.canonicalize() {
                Ok(p) => p,
                Err(_) => continue, // Path doesn't exist
            }
        };

        // Get mount directly using the mount root from the event
        let mut mt = mount_table
            .lock()
            .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;

        // Pass EventKind directly to on_fs_event (same type as walker uses)
        if let Some(mount) = mt
            .iter_mut()
            .find(|(root, _)| *root == mount_root)
            .map(|(_, m)| m)
            && let Some(event) = mount.on_fs_event(&canonical, kind)
        {
            mount_events.push(event);
        }
    }

    // Process mount events
    for event in mount_events {
        match event {
            FsEvent::ProjectAdded { root } => {
                // Discover the new project (watcher is initialized during walk)
                // Watch mode always uses cache (load_from_cache=true)
                if let Err(e) = on_project_discovery(&root, mount_table, db, true, Some(tx.clone()))
                {
                    tracing::warn!("failed to handle project discovery: {}", e);
                }
            }
            FsEvent::FileAdded { mount, path } => {
                let abs_path = mount.join(&path);

                // Compute relative project path from workspace root
                let project_str = {
                    let mt = mount_table
                        .lock()
                        .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;
                    mt.relative_project(&mount)
                };

                if let Err(e) = process_file_change(&abs_path, &path, &project_str, db) {
                    tracing::warn!("failed to process file {}: {}", path, e);
                } else {
                    // Mark mount as dirty
                    mount_table
                        .lock()
                        .ok()
                        .map(|mut mt| mt.mark_dirty_canonical(&abs_path));
                }
            }
            FsEvent::FileRemoved { mount, path } => {
                let abs_path = mount.join(&path);

                // Compute relative project path from workspace root
                let project_str = {
                    let mt = mount_table
                        .lock()
                        .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;
                    mt.relative_project(&mount)
                };

                tracing::debug!("file deleted: {} (project: {})", path, project_str);
                let db_guard = db
                    .lock()
                    .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
                if let Err(e) = db_guard.remove_file(&project_str, &path) {
                    tracing::warn!("failed to remove file {}: {}", path, e);
                }
                // Mark mount as dirty
                mount_table
                    .lock()
                    .ok()
                    .map(|mut mt| mt.mark_dirty_canonical(&abs_path));
            }
            FsEvent::DirIgnored => {} // Not emitted from notify events
        }
    }

    // Rebuild FTS indexes once after all changes
    {
        let db_guard = db
            .lock()
            .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
        db_guard.rebuild_fts()?;
    }

    Ok(())
}

/// Process a single file change (create or modify).
pub fn process_file_change(
    abs_path: &Path,
    rel_path: &str,
    project: &str,
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
    if let Some(old_hash) = db_guard.get_file_hash(project, rel_path)?
        && old_hash == new_hash
    {
        // No change, skip
        tracing::trace!(
            "skipping unchanged file: {} (project: {})",
            rel_path,
            project
        );
        return Ok(());
    }
    drop(db_guard);

    tracing::info!("indexing file: {} (project: {})", rel_path, project);

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
    let mut references = Vec::new();
    let mut title = None;
    let mut description = None;

    // Parse source files for symbols, texts, and references
    if let Some(ref lang_name) = lang {
        match parse_file(&content, lang_name, rel_path) {
            Ok((file_symbols, file_texts, file_refs)) => {
                symbols = file_symbols;
                texts = file_texts;
                references = file_refs;
            }
            Err(e) => {
                tracing::warn!("failed to parse {}: {}", rel_path, e);
            }
        }

        // Extract file metadata (title and description)
        let metadata = extract_file_metadata(&content, lang_name);
        title = metadata.title;
        description = metadata.description;
    }

    let file_entry = FileEntry {
        path: rel_path.to_string(),
        lang,
        hash: new_hash,
        lines: line_count,
        project: project.to_string(),
        title,
        description,
    };

    // Upsert into database
    let db_guard = db
        .lock()
        .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
    db_guard.upsert_file(project, &file_entry, &symbols, &texts, &references)?;

    Ok(())
}

/// Request a flush from a running server by creating a trigger file.
/// Waits for the server to delete the file (confirming flush) or times out.
fn request_flush_and_wait(project_root: &Path) -> Result<()> {
    let trigger_path = project_root.join(FLUSH_TRIGGER_FILE);

    // Create trigger file
    std::fs::write(&trigger_path, "").with_context(|| {
        format!(
            "failed to create flush trigger at {}",
            trigger_path.display()
        )
    })?;

    tracing::info!(
        "requesting flush from server (trigger: {})",
        trigger_path.display()
    );

    // Wait for server to delete the trigger file
    let start = std::time::Instant::now();
    while trigger_path.exists() {
        if start.elapsed() > FLUSH_TIMEOUT {
            // Clean up trigger file on timeout
            let _ = std::fs::remove_file(&trigger_path);
            anyhow::bail!(
                "timeout waiting for server to flush ({}s). Is codeix serve running?",
                FLUSH_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(FLUSH_POLL_INTERVAL);
    }

    tracing::info!("flush completed by server");
    Ok(())
}

/// Handle flush trigger file (.codeindex.flush) - flush and delete to signal completion.
fn handle_flush_trigger(
    trigger_path: &Path,
    mount_table: &Arc<Mutex<MountTable>>,
    db: &Arc<Mutex<SearchDb>>,
) -> Result<()> {
    let mount_root = trigger_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("trigger has no parent"))?;

    tracing::info!(
        "flush requested via trigger file for {}",
        mount_root.display()
    );

    // Flush the mount
    {
        let mt = mount_table
            .lock()
            .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;
        flush_mount_to_disk(mount_root, &mt, db)?;
    }

    // Clear dirty flag
    {
        let mut mt = mount_table
            .lock()
            .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;
        if let Some(mount) = mt.find_mount_mut(mount_root) {
            mount.clear_dirty();
        }
    }

    // Delete trigger file to signal completion
    std::fs::remove_file(trigger_path)?;
    tracing::info!("flush completed for {}", mount_root.display());

    Ok(())
}

/// Flush all dirty mounts to disk.
/// Returns the number of mounts that were flushed.
pub fn flush_dirty_mounts(
    mount_table: &Arc<Mutex<MountTable>>,
    db: &Arc<Mutex<SearchDb>>,
) -> Result<usize> {
    let mut mt = mount_table
        .lock()
        .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;

    // Collect dirty RW mounts
    let dirty_mounts: Vec<PathBuf> = mt
        .iter()
        .filter(|(_, mount)| mount.dirty && mount.mode == MountMode::ReadWrite)
        .map(|(root, _)| root.clone())
        .collect();

    let mut flushed_count = 0usize;
    for root in dirty_mounts {
        if let Err(e) = flush_mount_to_disk(&root, &mt, db) {
            tracing::error!("failed to flush {}: {}", root.display(), e);
        } else {
            flushed_count += 1;
            // Clear dirty flag
            if let Some(mount) = mt.find_mount_mut(&root) {
                mount.clear_dirty();
            }
        }
    }

    Ok(flushed_count)
}

/// Flush a single mount's index from memory to disk.
pub fn flush_mount_to_disk(
    mount_root: &Path,
    mount_table: &MountTable,
    db: &Arc<Mutex<SearchDb>>,
) -> Result<()> {
    // Use relative project path from workspace root
    let project_str = mount_table.relative_project(mount_root);

    let db_guard = db
        .lock()
        .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
    let (mut files, mut symbols, mut texts, mut refs) =
        db_guard.export_for_project(&project_str)?;
    drop(db_guard);

    // Clear project field for disk export - the .codeindex/ location implies the project
    for f in &mut files {
        f.project = String::new();
    }
    for s in &mut symbols {
        s.project = String::new();
    }
    for t in &mut texts {
        t.project = String::new();
    }
    for r in &mut refs {
        r.project = String::new();
    }

    if files.is_empty() {
        tracing::debug!("no files to flush for {}", mount_root.display());
        return Ok(());
    }

    // Collect languages
    let mut languages: BTreeSet<String> = BTreeSet::new();
    for f in &files {
        if let Some(ref lang) = f.lang {
            languages.insert(lang.clone());
        }
    }

    // Derive project name from directory name
    let name = mount_root
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

    let output_dir = mount_root.join(".codeindex");
    write_index(&output_dir, &manifest, &files, &symbols, &texts, &refs)?;

    tracing::debug!(
        "flushed index to disk for {}: {} files, {} symbols, {} texts, {} refs",
        mount_root.display(),
        files.len(),
        symbols.len(),
        texts.len(),
        refs.len()
    );

    Ok(())
}

/// Flush the entire index from memory to disk (legacy single-project).
/// This is used during initial index building before MountTable is set up.
pub fn flush_index_to_disk(root: &Path, db: &Arc<Mutex<SearchDb>>) -> Result<()> {
    let db_guard = db
        .lock()
        .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
    let (files, symbols, texts, refs) = db_guard.export_all()?;
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
    write_index(&output_dir, &manifest, &files, &symbols, &texts, &refs)?;

    tracing::debug!(
        "flushed index to disk: {} files, {} symbols, {} texts, {} refs",
        files.len(),
        symbols.len(),
        texts.len(),
        refs.len()
    );

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Helper to create a minimal .git directory (just the directory, not a real repo)
    fn create_git_marker(path: &Path) {
        fs::create_dir_all(path.join(".git")).unwrap();
    }

    /// Helper to create a source file
    fn create_source_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn test_single_project_indexing() {
        let tmp = TempDir::new().unwrap();
        // Canonicalize for macOS where /var -> /private/var
        let root = tmp.path().canonicalize().unwrap();

        // Create a simple project structure
        create_git_marker(&root);
        create_source_file(
            &root.join("src/main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        );
        create_source_file(
            &root.join("src/lib.rs"),
            "pub fn greet() -> String {\n    \"hello\".to_string()\n}\n",
        );

        // Index the project (load_from_cache=false to force indexing)
        let mount_table = Arc::new(Mutex::new(MountTable::new(root.clone())));
        let db = Arc::new(Mutex::new(SearchDb::new().unwrap()));

        on_project_discovery(&root, &mount_table, &db, false, None).unwrap();

        // Verify: should have 2 files indexed
        let db_guard = db.lock().unwrap();
        let projects = db_guard.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0], ""); // Root project has empty string

        // Search for the main function
        let symbols = db_guard
            .search_symbols(Some("main"), None, None, None, 100, 0)
            .unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "main" && s.kind == "function")
        );

        // Search for greet function
        let symbols = db_guard
            .search_symbols(Some("greet"), None, None, None, 100, 0)
            .unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "greet" && s.kind == "function")
        );
    }

    #[test]
    fn test_subproject_discovery() {
        let tmp = TempDir::new().unwrap();
        // Canonicalize for macOS where /var -> /private/var
        let root = tmp.path().canonicalize().unwrap();

        // Create root project
        create_git_marker(&root);
        create_source_file(&root.join("app.rs"), "fn app_main() {}\n");

        // Create a subproject
        let subproject = root.join("libs/utils");
        create_git_marker(&subproject);
        create_source_file(&subproject.join("src/lib.rs"), "pub fn utility() {}\n");

        // Index from root (load_from_cache=false to force indexing)
        let mount_table = Arc::new(Mutex::new(MountTable::new(root.clone())));
        let db = Arc::new(Mutex::new(SearchDb::new().unwrap()));

        on_project_discovery(&root, &mount_table, &db, false, None).unwrap();

        // Verify: should have 2 projects
        let db_guard = db.lock().unwrap();
        let projects = db_guard.list_projects().unwrap();
        assert_eq!(projects.len(), 2);
        assert!(projects.contains(&"".to_string())); // Root
        assert!(projects.contains(&"libs/utils".to_string())); // Subproject

        // Root project should have app_main (search without filter, check project field)
        let symbols = db_guard
            .search_symbols(Some("app_main"), None, None, None, 100, 0)
            .unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].project, "");

        // Subproject should have utility
        let symbols = db_guard
            .search_symbols(Some("utility"), None, None, Some("libs/utils"), 100, 0)
            .unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].project, "libs/utils");
    }

    #[test]
    fn test_nested_subprojects() {
        let tmp = TempDir::new().unwrap();
        // Canonicalize for macOS where /var -> /private/var
        let root = tmp.path().canonicalize().unwrap();

        // Create root project
        create_git_marker(&root);
        create_source_file(&root.join("root.rs"), "fn root_fn() {}\n");

        // Create nested subprojects: root > libs/core > libs/core/nested
        let core = root.join("libs/core");
        create_git_marker(&core);
        create_source_file(&core.join("core.rs"), "fn core_fn() {}\n");

        let nested = core.join("nested");
        create_git_marker(&nested);
        create_source_file(&nested.join("nested.rs"), "fn nested_fn() {}\n");

        // Index from root (load_from_cache=false to force indexing)
        let mount_table = Arc::new(Mutex::new(MountTable::new(root.clone())));
        let db = Arc::new(Mutex::new(SearchDb::new().unwrap()));

        on_project_discovery(&root, &mount_table, &db, false, None).unwrap();

        // Verify: should have 3 projects
        let db_guard = db.lock().unwrap();
        let projects = db_guard.list_projects().unwrap();
        assert_eq!(projects.len(), 3);

        // Each function should be in its respective project (search without filter, verify project)
        let root_syms = db_guard
            .search_symbols(Some("root_fn"), None, None, None, 100, 0)
            .unwrap();
        assert_eq!(root_syms.len(), 1);
        assert_eq!(root_syms[0].project, "");

        let core_syms = db_guard
            .search_symbols(Some("core_fn"), None, None, Some("libs/core"), 100, 0)
            .unwrap();
        assert_eq!(core_syms.len(), 1);

        let nested_syms = db_guard
            .search_symbols(
                Some("nested_fn"),
                None,
                None,
                Some("libs/core/nested"),
                100,
                0,
            )
            .unwrap();
        assert_eq!(nested_syms.len(), 1);
    }

    #[test]
    fn test_files_not_duplicated_across_projects() {
        let tmp = TempDir::new().unwrap();
        // Canonicalize for macOS where /var -> /private/var
        let root = tmp.path().canonicalize().unwrap();

        // Create root with a subproject
        create_git_marker(&root);
        create_source_file(&root.join("root.rs"), "fn root_fn() {}\n");

        let sub = root.join("sub");
        create_git_marker(&sub);
        create_source_file(&sub.join("sub.rs"), "fn sub_fn() {}\n");

        // Index (load_from_cache=false to force indexing)
        let mount_table = Arc::new(Mutex::new(MountTable::new(root.clone())));
        let db = Arc::new(Mutex::new(SearchDb::new().unwrap()));

        on_project_discovery(&root, &mount_table, &db, false, None).unwrap();

        // Verify: sub.rs should NOT appear in root project
        let db_guard = db.lock().unwrap();

        // Search without project filter - should find both
        let all_symbols = db_guard
            .search_symbols(Some("fn"), None, None, None, 100, 0)
            .unwrap();
        let root_fn_count = all_symbols.iter().filter(|s| s.name == "root_fn").count();
        let sub_fn_count = all_symbols.iter().filter(|s| s.name == "sub_fn").count();

        assert_eq!(root_fn_count, 1, "root_fn should appear exactly once");
        assert_eq!(sub_fn_count, 1, "sub_fn should appear exactly once");

        // Verify project assignment
        let root_fn = all_symbols.iter().find(|s| s.name == "root_fn").unwrap();
        let sub_fn = all_symbols.iter().find(|s| s.name == "sub_fn").unwrap();

        assert_eq!(root_fn.project, "");
        assert_eq!(sub_fn.project, "sub");
    }

    #[test]
    fn test_mount_table_tracks_all_projects() {
        let tmp = TempDir::new().unwrap();
        // Canonicalize for macOS where /var -> /private/var
        let root = tmp.path().canonicalize().unwrap();

        // Create root with two subprojects
        create_git_marker(&root);
        create_source_file(&root.join("main.rs"), "fn main() {}\n");

        let lib_a = root.join("libs/a");
        create_git_marker(&lib_a);
        create_source_file(&lib_a.join("a.rs"), "fn a() {}\n");

        let lib_b = root.join("libs/b");
        create_git_marker(&lib_b);
        create_source_file(&lib_b.join("b.rs"), "fn b() {}\n");

        // Index (load_from_cache=false to force indexing)
        let mount_table = Arc::new(Mutex::new(MountTable::new(root.clone())));
        let db = Arc::new(Mutex::new(SearchDb::new().unwrap()));

        on_project_discovery(&root, &mount_table, &db, false, None).unwrap();

        // Verify mount table has all 3 mounts
        let mt = mount_table.lock().unwrap();
        let mounts: Vec<_> = mt.iter().collect();
        assert_eq!(mounts.len(), 3);

        // All should be mounted (lib_a/lib_b are already based on canonicalized root)
        assert!(mt.is_mounted(&root));
        assert!(mt.is_mounted(&lib_a));
        assert!(mt.is_mounted(&lib_b));
    }

    #[test]
    fn test_project_filter_in_search() {
        let tmp = TempDir::new().unwrap();
        // Canonicalize for macOS where /var -> /private/var
        let root = tmp.path().canonicalize().unwrap();

        // Create two projects with same-named function
        create_git_marker(&root);
        create_source_file(&root.join("util.rs"), "fn helper() {}\n");

        let sub = root.join("sub");
        create_git_marker(&sub);
        create_source_file(&sub.join("util.rs"), "fn helper() {}\n");

        // Index (load_from_cache=false to force indexing)
        let mount_table = Arc::new(Mutex::new(MountTable::new(root.clone())));
        let db = Arc::new(Mutex::new(SearchDb::new().unwrap()));

        on_project_discovery(&root, &mount_table, &db, false, None).unwrap();

        let db_guard = db.lock().unwrap();

        // Without filter: should find 2 helpers
        let all = db_guard
            .search_symbols(Some("helper"), None, None, None, 100, 0)
            .unwrap();
        assert_eq!(all.len(), 2);

        // One should be root (empty project), one should be sub
        let root_helpers: Vec<_> = all.iter().filter(|s| s.project.is_empty()).collect();
        let sub_helpers: Vec<_> = all.iter().filter(|s| s.project == "sub").collect();
        assert_eq!(root_helpers.len(), 1);
        assert_eq!(sub_helpers.len(), 1);

        // With sub filter: should find 1
        let sub_only = db_guard
            .search_symbols(Some("helper"), None, None, Some("sub"), 100, 0)
            .unwrap();
        assert_eq!(sub_only.len(), 1);
        assert_eq!(sub_only[0].project, "sub");
    }

    #[test]
    fn test_relative_project_paths() {
        let tmp = TempDir::new().unwrap();
        // Canonicalize for macOS where /var -> /private/var
        let root = tmp.path().canonicalize().unwrap();

        // Create deeply nested subproject
        create_git_marker(&root);
        let deep = root.join("path/to/deep/project");
        create_git_marker(&deep);
        create_source_file(&deep.join("deep.rs"), "fn deep_fn() {}\n");

        // Index (load_from_cache=false to force indexing)
        let mount_table = Arc::new(Mutex::new(MountTable::new(root.clone())));
        let db = Arc::new(Mutex::new(SearchDb::new().unwrap()));

        on_project_discovery(&root, &mount_table, &db, false, None).unwrap();

        // Verify relative path is correct
        let db_guard = db.lock().unwrap();
        let projects = db_guard.list_projects().unwrap();

        assert!(projects.contains(&"path/to/deep/project".to_string()));

        // Symbol should have correct project
        let symbols = db_guard
            .search_symbols(Some("deep_fn"), None, None, None, 100, 0)
            .unwrap();
        assert_eq!(symbols[0].project, "path/to/deep/project");
    }
}
