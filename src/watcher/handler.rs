use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use notify::event::{CreateKind, EventKind, RemoveKind};

use crate::index::format::{FileEntry, IndexManifest};
use crate::index::reader::read_index;
use crate::index::writer::write_index;
use crate::parser::languages::detect_language;
use crate::parser::metadata::extract_file_metadata;
use crate::parser::treesitter::parse_file;
use crate::scanner::hasher::hash_bytes;
use crate::scanner::mount::{MountMode, MountTable};
use crate::server::db::SearchDb;
use crate::watcher::GitignoreWatcher;

const DEBOUNCE_DELAY: Duration = Duration::from_millis(500);
const FLUSH_DELAY: Duration = Duration::from_secs(5);

/// Start watching the given directory for file changes and trigger
/// re-indexing when files are modified.
///
/// Uses `GitignoreWatcher` for efficient watching that respects `.gitignore` rules.
/// Routes events to the appropriate mount via `MountTable`.
pub fn start_watcher(
    root: PathBuf,
    mount_table: Arc<Mutex<MountTable>>,
    db: Arc<Mutex<SearchDb>>,
) -> Result<()> {
    let root_canonical = root
        .canonicalize()
        .with_context(|| format!("cannot resolve path: {}", root.display()))?;

    tracing::info!("starting file watcher on {}", root_canonical.display());

    // Channel for receiving events
    let (tx, rx) = std::sync::mpsc::channel();

    // Create gitignore-aware watcher (non-recursive, respects .gitignore)
    let mut watcher = GitignoreWatcher::new(&root_canonical, tx)?;

    tracing::info!(
        "file watcher ready ({} directories watched)",
        watcher.watched_count()
    );

    // Debounce state: path -> (last event time, event kind)
    let mut pending: HashMap<PathBuf, (Instant, EventKind)> = HashMap::new();
    let mut last_flush = Instant::now();

    loop {
        // Wait for events with timeout
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(event)) => {
                let now = Instant::now();
                for path in event.paths {
                    pending.insert(path, (now, event.kind));
                }
            }
            Ok(Err(e)) => {
                tracing::warn!("watcher error: {}", e);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Check for debounced events ready to process
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Flush all dirty mounts before shutting down
                flush_dirty_mounts(&mount_table, &db)?;
                tracing::info!("watcher channel closed, shutting down");
                break;
            }
        }

        // Process debounced events
        let now = Instant::now();
        let ready: Vec<(PathBuf, EventKind)> = pending
            .iter()
            .filter(|&(_, (time, _))| now.duration_since(*time) >= DEBOUNCE_DELAY)
            .map(|(path, (_, kind))| (path.clone(), *kind))
            .collect();

        if !ready.is_empty() {
            for (path, _) in &ready {
                pending.remove(path);
            }

            if let Err(e) = handle_events(&ready, &mount_table, &db, &mut watcher) {
                tracing::error!("error handling watch events: {}", e);
            }
        }

        // Flush dirty mounts to disk after quiet period
        if now.duration_since(last_flush) >= FLUSH_DELAY && pending.is_empty() {
            if let Err(e) = flush_dirty_mounts(&mount_table, &db) {
                tracing::error!("failed to flush index to disk: {}", e);
            }
            last_flush = now;
        }
    }

    Ok(())
}

/// Handle when a .git/ directory is discovered (during walk or watch).
/// Mounts the project (parent of .git/) and loads/indexes it.
///
/// 1. If already mounted, skip
/// 2. Mount in RW mode
/// 3. If .codeindex/ exists, load from disk
/// 4. Otherwise, index files (stopping at subproject boundaries)
pub fn on_project_discovery(
    project_root: &Path,
    mount_table: &Arc<Mutex<MountTable>>,
    db: &Arc<Mutex<SearchDb>>,
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

    // Mount the new project in read-write mode
    mt.mount_rw(project_root)?;

    let project_name = project_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let project_str = mt.relative_project(project_root);

    drop(mt);

    // Try loading from .codeindex/ first
    let index_dir = project_root.join(".codeindex");
    if index_dir.is_dir() {
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
                    "loaded '{}' from .codeindex/: {} files, {} symbols, {} texts, {} refs",
                    manifest.name,
                    idx_files.len(),
                    idx_symbols.len(),
                    idx_texts.len(),
                    idx_refs.len()
                );

                // Still need to discover subprojects (they might not be in .codeindex/)
                discover_subprojects(project_root, mount_table, db)?;

                return Ok(());
            }
            Err(e) => {
                tracing::warn!(
                    "failed to read .codeindex/ for '{}', rebuilding: {}",
                    project_name,
                    e
                );
                // Fall through to index files
            }
        }
    }

    tracing::info!("indexing '{}'", project_name);

    // Index all files in the new project (also discovers subprojects)
    index_project_files(project_root, mount_table, db)?;

    Ok(())
}

/// Discover subprojects without indexing files (used when loading from .codeindex/).
/// Walks to find .git/ directories and calls on_project_discovery for each.
fn discover_subprojects(
    project_root: &Path,
    mount_table: &Arc<Mutex<MountTable>>,
    db: &Arc<Mutex<SearchDb>>,
) -> Result<()> {
    use ignore::WalkBuilder;

    let mut subprojects: HashSet<PathBuf> = HashSet::new();

    for entry in WalkBuilder::new(project_root)
        .hidden(false) // Need to see .git/ directories
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build()
    {
        let entry = entry?;
        let abs_path = entry.path();
        let file_name = abs_path.file_name().and_then(|n| n.to_str());

        // Check if this is a .git directory (subproject marker)
        if file_name == Some(".git")
            && abs_path.is_dir()
            && let Some(subproject_root) = abs_path.parent()
            // Don't treat root's own .git as a subproject
            && subproject_root != project_root
            && !subprojects.iter().any(|sp| subproject_root.starts_with(sp))
        {
            subprojects.insert(subproject_root.to_path_buf());
            // Recursively handle the subproject
            if let Err(e) = on_project_discovery(subproject_root, mount_table, db) {
                tracing::warn!(
                    "failed to handle subproject {}: {}",
                    subproject_root.display(),
                    e
                );
            }
        }
    }

    Ok(())
}

/// Index all files in a project directory, stopping at subproject boundaries.
///
/// When a `.git/` directory is found in a subdirectory (indicating a subproject),
/// calls `on_project_discovery` for that subproject and skips its files.
fn index_project_files(
    project_root: &Path,
    mount_table: &Arc<Mutex<MountTable>>,
    db: &Arc<Mutex<SearchDb>>,
) -> Result<()> {
    use ignore::WalkBuilder;

    // Use relative project path from workspace root
    let project_str = {
        let mt = mount_table
            .lock()
            .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;
        mt.relative_project(project_root)
    };

    // Track discovered subprojects to skip their files
    let mut subprojects: HashSet<PathBuf> = HashSet::new();
    let mut file_count = 0u32;

    for entry in WalkBuilder::new(project_root)
        .hidden(true) // Skip hidden files, we'll check for .git manually
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build()
    {
        let entry = entry?;
        let abs_path = entry.path();

        // For directories, check if they contain .git (subproject marker)
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            // Skip project root itself
            if abs_path == project_root {
                continue;
            }

            // Check if this directory is a subproject (has .git/)
            let git_dir = abs_path.join(".git");
            if git_dir.is_dir() {
                tracing::info!("discovered subproject: {}", abs_path.display());
                subprojects.insert(abs_path.to_path_buf());
                // Recursively handle the subproject
                if let Err(e) = on_project_discovery(abs_path, mount_table, db) {
                    tracing::warn!("failed to handle subproject {}: {}", abs_path.display(), e);
                }
            }
            continue;
        }

        // Skip files inside subprojects (they're handled by their own index_project_files)
        let in_subproject = subprojects.iter().any(|sp| abs_path.starts_with(sp));
        if in_subproject {
            continue;
        }

        // Only process regular files
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }

        let rel_path = abs_path
            .strip_prefix(project_root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();

        // Skip .codeindex/ files
        if rel_path.starts_with(".codeindex/") || rel_path == ".codeindex" {
            continue;
        }

        file_count += 1;
        if file_count.is_multiple_of(100) {
            tracing::info!(
                "processed {} files so far for project '{}'",
                file_count,
                project_str
            );
        }
        if let Err(e) = process_file_change(abs_path, &rel_path, &project_str, db) {
            tracing::warn!("failed to index {}: {}", rel_path, e);
        }
    }

    tracing::info!(
        "finished walking project '{}': {} files total",
        project_str,
        file_count
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
fn handle_events(
    events: &[(PathBuf, EventKind)],
    mount_table: &Arc<Mutex<MountTable>>,
    db: &Arc<Mutex<SearchDb>>,
    watcher: &mut GitignoreWatcher,
) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }

    tracing::debug!("processing {} file events", events.len());

    let mut changed_files: Vec<(PathBuf, String, String)> = Vec::new(); // (abs_path, rel_path, project)

    for (path, kind) in events {
        // Handle directory creation/deletion for watcher updates
        match kind {
            EventKind::Create(CreateKind::Folder) => {
                // Check if this is a .git directory (new project discovered)
                if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
                    if let Some(project_root) = path.parent()
                        && let Err(e) = on_project_discovery(project_root, mount_table, db)
                    {
                        tracing::warn!("failed to handle project discovery: {}", e);
                    }
                    continue;
                }

                // Canonicalize once for this path
                let canonical = match path.canonicalize() {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // Add watch for new directory if not ignored
                let mt = mount_table
                    .lock()
                    .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;
                if let Some(mount) = mt.find_mount_canonical(&canonical)
                    && !mount.is_ignored(path)
                    && !path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('.'))
                    && let Err(e) = watcher.watch_dir_if_valid(path)
                {
                    tracing::debug!("failed to watch new dir: {}", e);
                }
                continue;
            }
            EventKind::Remove(RemoveKind::Folder) => {
                watcher.on_dir_removed(path);
                continue;
            }
            _ => {}
        }

        // Early filter: skip .codeindex/ paths before expensive canonicalize()
        if path.components().any(|c| c.as_os_str() == ".codeindex") {
            continue;
        }

        // Canonicalize once for this path (avoids repeated readlink syscalls)
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => continue, // File may have been deleted
        };

        // Find the mount for this path
        let mt = mount_table
            .lock()
            .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;

        let Some(mount) = mt.find_mount_canonical(&canonical) else {
            continue; // Not under any mount
        };

        // Get mount info before dropping lock
        let mount_root = mount.root.clone();
        let is_ignored = mount.is_ignored(path);
        drop(mt);

        if is_ignored {
            continue;
        }

        // Compute relative path
        let rel_path = match canonical.strip_prefix(&mount_root) {
            Ok(p) => p.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };

        // Skip .codeindex/ directory
        if rel_path.starts_with(".codeindex/") || rel_path == ".codeindex" {
            continue;
        }

        // Skip hidden files (dotfiles like .mcp.json)
        if rel_path.starts_with('.') {
            continue;
        }

        // Skip directories
        if canonical.is_dir() {
            continue;
        }

        // Compute relative project path from workspace root
        let project_str = {
            let mt = mount_table
                .lock()
                .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;
            mt.relative_project(&mount_root)
        };

        // Check if file still exists (use original path for exists check)
        if !path.exists() {
            // File was deleted
            tracing::debug!("file deleted: {} (project: {})", rel_path, project_str);
            let db_guard = db
                .lock()
                .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
            if let Err(e) = db_guard.remove_file(&project_str, &rel_path) {
                tracing::warn!("failed to remove file {}: {}", rel_path, e);
            }
            // Mark mount as dirty (use canonical path)
            mount_table
                .lock()
                .ok()
                .map(|mut mt| mt.mark_dirty_canonical(&canonical));
            continue;
        }

        // File was created or modified (store canonical path)
        changed_files.push((canonical, rel_path, project_str));
    }

    // Process changed files
    for (canonical_path, rel_path, project) in &changed_files {
        if let Err(e) = process_file_change(canonical_path, rel_path, project, db) {
            tracing::warn!("failed to process file {}: {}", rel_path, e);
        } else {
            // Mark mount as dirty on successful change (path is already canonical)
            mount_table
                .lock()
                .ok()
                .map(|mut mt| mt.mark_dirty_canonical(canonical_path));
        }
    }

    // Rebuild FTS indexes once after all changes
    if !changed_files.is_empty() {
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

/// Flush all dirty mounts to disk.
fn flush_dirty_mounts(
    mount_table: &Arc<Mutex<MountTable>>,
    db: &Arc<Mutex<SearchDb>>,
) -> Result<()> {
    let mut mt = mount_table
        .lock()
        .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;

    // Collect dirty RW mounts
    let dirty_mounts: Vec<PathBuf> = mt
        .iter()
        .filter(|(_, mount)| mount.dirty && mount.mode == MountMode::ReadWrite)
        .map(|(root, _)| root.clone())
        .collect();

    for root in dirty_mounts {
        if let Err(e) = flush_mount_to_disk(&root, &mt, db) {
            tracing::error!("failed to flush {}: {}", root.display(), e);
        } else {
            // Clear dirty flag
            if let Some(mount) = mt.find_mount_mut(&root) {
                mount.clear_dirty();
            }
        }
    }

    Ok(())
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
        let root = tmp.path();

        // Create a simple project structure
        create_git_marker(root);
        create_source_file(
            &root.join("src/main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        );
        create_source_file(
            &root.join("src/lib.rs"),
            "pub fn greet() -> String {\n    \"hello\".to_string()\n}\n",
        );

        // Index the project
        let mount_table = Arc::new(Mutex::new(MountTable::new(root.to_path_buf())));
        let db = Arc::new(Mutex::new(SearchDb::new().unwrap()));

        on_project_discovery(root, &mount_table, &db).unwrap();

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
        let root = tmp.path();

        // Create root project
        create_git_marker(root);
        create_source_file(&root.join("app.rs"), "fn app_main() {}\n");

        // Create a subproject
        let subproject = root.join("libs/utils");
        create_git_marker(&subproject);
        create_source_file(&subproject.join("src/lib.rs"), "pub fn utility() {}\n");

        // Index from root
        let mount_table = Arc::new(Mutex::new(MountTable::new(root.to_path_buf())));
        let db = Arc::new(Mutex::new(SearchDb::new().unwrap()));

        on_project_discovery(root, &mount_table, &db).unwrap();

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
        let root = tmp.path();

        // Create root project
        create_git_marker(root);
        create_source_file(&root.join("root.rs"), "fn root_fn() {}\n");

        // Create nested subprojects: root > libs/core > libs/core/nested
        let core = root.join("libs/core");
        create_git_marker(&core);
        create_source_file(&core.join("core.rs"), "fn core_fn() {}\n");

        let nested = core.join("nested");
        create_git_marker(&nested);
        create_source_file(&nested.join("nested.rs"), "fn nested_fn() {}\n");

        // Index from root
        let mount_table = Arc::new(Mutex::new(MountTable::new(root.to_path_buf())));
        let db = Arc::new(Mutex::new(SearchDb::new().unwrap()));

        on_project_discovery(root, &mount_table, &db).unwrap();

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
        let root = tmp.path();

        // Create root with a subproject
        create_git_marker(root);
        create_source_file(&root.join("root.rs"), "fn root_fn() {}\n");

        let sub = root.join("sub");
        create_git_marker(&sub);
        create_source_file(&sub.join("sub.rs"), "fn sub_fn() {}\n");

        // Index
        let mount_table = Arc::new(Mutex::new(MountTable::new(root.to_path_buf())));
        let db = Arc::new(Mutex::new(SearchDb::new().unwrap()));

        on_project_discovery(root, &mount_table, &db).unwrap();

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
        let root = tmp.path();

        // Create root with two subprojects
        create_git_marker(root);
        create_source_file(&root.join("main.rs"), "fn main() {}\n");

        let lib_a = root.join("libs/a");
        create_git_marker(&lib_a);
        create_source_file(&lib_a.join("a.rs"), "fn a() {}\n");

        let lib_b = root.join("libs/b");
        create_git_marker(&lib_b);
        create_source_file(&lib_b.join("b.rs"), "fn b() {}\n");

        // Index
        let mount_table = Arc::new(Mutex::new(MountTable::new(root.to_path_buf())));
        let db = Arc::new(Mutex::new(SearchDb::new().unwrap()));

        on_project_discovery(root, &mount_table, &db).unwrap();

        // Verify mount table has all 3 mounts
        let mt = mount_table.lock().unwrap();
        let mounts: Vec<_> = mt.iter().collect();
        assert_eq!(mounts.len(), 3);

        // All should be mounted
        assert!(mt.is_mounted(&root.canonicalize().unwrap()));
        assert!(mt.is_mounted(&lib_a.canonicalize().unwrap()));
        assert!(mt.is_mounted(&lib_b.canonicalize().unwrap()));
    }

    #[test]
    fn test_project_filter_in_search() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Create two projects with same-named function
        create_git_marker(root);
        create_source_file(&root.join("util.rs"), "fn helper() {}\n");

        let sub = root.join("sub");
        create_git_marker(&sub);
        create_source_file(&sub.join("util.rs"), "fn helper() {}\n");

        // Index
        let mount_table = Arc::new(Mutex::new(MountTable::new(root.to_path_buf())));
        let db = Arc::new(Mutex::new(SearchDb::new().unwrap()));

        on_project_discovery(root, &mount_table, &db).unwrap();

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
        let root = tmp.path();

        // Create deeply nested subproject
        create_git_marker(root);
        let deep = root.join("path/to/deep/project");
        create_git_marker(&deep);
        create_source_file(&deep.join("deep.rs"), "fn deep_fn() {}\n");

        // Index
        let mount_table = Arc::new(Mutex::new(MountTable::new(root.to_path_buf())));
        let db = Arc::new(Mutex::new(SearchDb::new().unwrap()));

        on_project_discovery(root, &mount_table, &db).unwrap();

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
