pub mod handler;

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use anyhow::{Context, Result};
use fs2::FileExt;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::event::{CreateKind, EventKind, RemoveKind};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use walkdir::WalkDir;

/// Built-in gitignore patterns (always applied).
/// These are either internal directories, IDE config, or OS cruft.
const BUILTIN_GITIGNORE: &[&str] = &[
    // Git internals (but .git itself is used for project detection)
    ".git/",
    // Our own output
    ".codeindex/",
    // Editor/IDE directories
    ".vscode/",
    ".idea/",
    ".vs/",
    // OS cruft
    ".DS_Store",
    ".Spotlight-V100/",
    ".Trashes/",
];

/// Mount mode determines whether the index can be written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountMode {
    /// Read-write: acquires exclusive flock, allows modifications.
    ReadWrite,
    /// Read-only: no lock, index data is immutable.
    ReadOnly,
}

/// Check if an EventKind represents a removal operation.
/// Used to determine if we can canonicalize the path (removed files can't be canonicalized).
pub fn is_removal_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Remove(RemoveKind::File) | EventKind::Remove(RemoveKind::Folder)
    )
}

/// Filesystem events emitted to external consumers (handler/DB).
/// These are the result of processing raw notify events through mount rules.
#[derive(Debug, Clone)]
pub enum FsEvent {
    /// A file was discovered or modified.
    FileAdded {
        /// Absolute path to the mount root.
        mount: PathBuf,
        /// Path relative to mount root.
        path: String,
    },
    /// A file was deleted.
    FileRemoved {
        /// Absolute path to the mount root.
        mount: PathBuf,
        /// Path relative to mount root.
        path: String,
    },
    /// A subproject (.git/ directory) was discovered.
    ProjectAdded {
        /// Absolute path to the subproject root (parent of .git/).
        root: PathBuf,
    },
    /// A subproject was removed (its .git/ directory was deleted).
    ProjectRemoved {
        /// Absolute path to the subproject root (parent of .git/).
        root: PathBuf,
    },
    /// Directory should be skipped (gitignore match).
    /// Walker should call skip_current_dir() to avoid descending.
    DirIgnored,
}

/// Event with mount identity attached (avoids lookup in handler).
pub type MountedEvent = (PathBuf, Result<Event, notify::Error>);

/// A single mounted directory with its index.
pub struct Mount {
    /// Root directory of the mount.
    pub root: PathBuf,
    /// Mount mode (read-write or read-only).
    pub mode: MountMode,
    /// File lock handle (only present for ReadWrite mounts).
    lock: Option<File>,
    /// Whether the index has been modified and needs flushing.
    pub dirty: bool,
    /// Gitignore rules for this mount (built during walk, used by watcher).
    /// None until walk() is called.
    gitignore: Option<Gitignore>,
    /// All gitignore files discovered (for rebuilding when new ones are added).
    gitignore_files: Vec<PathBuf>,
    /// File system watcher (only present for ReadWrite mounts).
    watcher: Option<RecommendedWatcher>,
    /// Directories currently being watched.
    watched_dirs: HashSet<PathBuf>,
}

impl std::fmt::Debug for Mount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mount")
            .field("root", &self.root)
            .field("mode", &self.mode)
            .field("dirty", &self.dirty)
            .field("watched_dirs", &self.watched_dirs.len())
            .finish()
    }
}

impl Mount {
    /// Create a new read-only mount (no lock, no watcher).
    fn new_ro(root: PathBuf) -> Result<Self> {
        let mut mount = Self {
            root,
            mode: MountMode::ReadOnly,
            lock: None,
            dirty: false,
            gitignore: None,
            gitignore_files: Vec::new(),
            watcher: None,
            watched_dirs: HashSet::new(),
        };
        mount.init_gitignore()?;
        Ok(mount)
    }

    /// Create a new read-write mount with exclusive flock.
    /// Does NOT start notify - call `init_notify()` separately.
    fn new_rw(root: PathBuf) -> Result<Self> {
        // Create .codeindex directory if it doesn't exist
        let codeindex_dir = root.join(".codeindex");
        std::fs::create_dir_all(&codeindex_dir).with_context(|| {
            format!(
                "failed to create .codeindex directory at {:?}",
                codeindex_dir
            )
        })?;

        // Acquire exclusive lock on index.json
        let lock_path = codeindex_dir.join("index.json");
        let lock_file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open lock file at {:?}", lock_path))?;

        lock_file
            .try_lock_exclusive()
            .with_context(|| format!("failed to acquire exclusive lock on {:?}", lock_path))?;

        let mut mount = Self {
            root,
            mode: MountMode::ReadWrite,
            lock: Some(lock_file),
            dirty: false,
            gitignore: None,
            gitignore_files: Vec::new(),
            watcher: None,
            watched_dirs: HashSet::new(),
        };
        mount.init_gitignore()?;
        Ok(mount)
    }

    /// Initialize gitignore with .git/info/exclude and root .gitignore.
    fn init_gitignore(&mut self) -> Result<()> {
        self.gitignore_files.clear();

        // Add .git/info/exclude if it exists
        let exclude_path = self.root.join(".git/info/exclude");
        if exclude_path.exists() {
            self.gitignore_files.push(exclude_path);
        }

        // Add root .gitignore - must be loaded first before walking siblings
        let root_gitignore = self.root.join(".gitignore");
        if root_gitignore.exists() {
            self.gitignore_files.push(root_gitignore);
        }

        self.build_gitignore()
    }

    /// Build gitignore from all tracked files plus built-in patterns.
    fn build_gitignore(&mut self) -> Result<()> {
        let mut builder = GitignoreBuilder::new(&self.root);

        // Add built-in patterns first
        for pattern in BUILTIN_GITIGNORE {
            let _ = builder.add_line(None, pattern);
        }

        // Add user gitignore files
        for file in &self.gitignore_files {
            builder.add(file);
        }

        self.gitignore = Some(
            builder
                .build()
                .with_context(|| format!("failed to build gitignore for {:?}", self.root))?,
        );

        Ok(())
    }

    /// Handle a filesystem event (from walker or notify).
    ///
    /// This is the central handler that applies ALL rules:
    /// - Gitignore filtering
    /// - Project detection (.git/)
    /// - .gitignore file updates
    /// - Watch management
    ///
    /// Takes notify's `EventKind` directly - both walker and notify use the same type.
    /// Returns an external event if this should be forwarded to consumers.
    /// Returns None for event types we don't care about.
    pub fn on_fs_event(&mut self, abs_path: &Path, kind: &EventKind) -> Option<FsEvent> {
        // Check if path is under mount root
        if !abs_path.starts_with(&self.root) {
            return None;
        }

        match kind {
            EventKind::Create(CreateKind::Folder) => self.on_dir_added(abs_path),
            EventKind::Remove(RemoveKind::Folder) => self.on_dir_removed(abs_path),
            EventKind::Create(CreateKind::File) | EventKind::Modify(_) => {
                self.on_file_added(abs_path)
            }
            EventKind::Remove(RemoveKind::File) => self.on_file_removed(abs_path),
            _ => None, // Ignore other event types
        }
    }

    fn on_dir_added(&mut self, abs_path: &Path) -> Option<FsEvent> {
        let name = abs_path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // Check if this is a .git directory -> project discovery
        // (must check before gitignore since .git/ is in BUILTIN_GITIGNORE)
        if name == ".git" {
            return abs_path.parent().map(|root| FsEvent::ProjectAdded {
                root: root.to_path_buf(),
            });
        }

        // Check gitignore - return DirIgnored so walker can skip subtree
        if self.is_ignored(abs_path) {
            return Some(FsEvent::DirIgnored);
        }

        // Check if directory contains .git (file or dir) -> it's a subproject root
        // Git submodules use a .git file pointing to parent's .git/modules/
        if abs_path.join(".git").exists() {
            return Some(FsEvent::ProjectAdded {
                root: abs_path.to_path_buf(),
            });
        }

        // Check for .gitignore in this directory : must be loaded first before walking siblings
        let dir_gitignore = abs_path.join(".gitignore");
        if dir_gitignore.exists() {
            self.add_gitignore(&dir_gitignore);
        }

        // Add watch for new directory
        let _ = self.watch_dir(abs_path);
        None // DirAdded is internal only
    }

    fn on_dir_removed(&mut self, abs_path: &Path) -> Option<FsEvent> {
        self.remove_watch(abs_path);

        // Check if this is a .git directory -> project removal
        let name = abs_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == ".git" {
            return abs_path.parent().map(|root| FsEvent::ProjectRemoved {
                root: root.to_path_buf(),
            });
        }

        None // DirRemoved is internal only
    }

    fn on_file_added(&mut self, abs_path: &Path) -> Option<FsEvent> {
        let name = abs_path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // Check gitignore
        if self.is_ignored(abs_path) {
            return None;
        }

        // Check if it's a .gitignore file -> update rules
        if name == ".gitignore" {
            self.add_gitignore(abs_path);
            return None; // Don't index .gitignore files themselves
        }

        // Skip hidden files (dotfiles)
        if name.starts_with('.') {
            return None;
        }

        let rel_path = abs_path
            .strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();

        Some(FsEvent::FileAdded {
            mount: self.root.clone(),
            path: rel_path,
        })
    }

    fn on_file_removed(&mut self, abs_path: &Path) -> Option<FsEvent> {
        let name = abs_path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // Skip hidden files
        if name.starts_with('.') {
            return None;
        }

        let rel_path = abs_path
            .strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();

        Some(FsEvent::FileRemoved {
            mount: self.root.clone(),
            path: rel_path,
        })
    }

    /// Add a .gitignore file to the rules.
    fn add_gitignore(&mut self, gitignore_path: &Path) {
        // Track the file
        if !self.gitignore_files.contains(&gitignore_path.to_path_buf()) {
            self.gitignore_files.push(gitignore_path.to_path_buf());
            // Rebuild gitignore with all known files
            let _ = self.build_gitignore();
        }
    }

    /// Remove watch for a directory.
    fn remove_watch(&mut self, path: &Path) {
        if self.watched_dirs.remove(path)
            && let Some(ref mut watcher) = self.watcher
        {
            let _ = watcher.unwatch(path);
        }
    }

    /// Walk all files in this mount, emitting events via callback.
    ///
    /// This is a dumb walker - it just iterates and calls on_fs_event for each entry.
    /// All the smart logic (gitignore, skip entries, project detection) is in on_fs_event.
    ///
    /// After walk completes, the built gitignore is stored for use by the watcher.
    pub fn walk<F>(&mut self, mut on_event: F) -> Result<()>
    where
        F: FnMut(FsEvent) -> Result<()>,
    {
        let root = self.root.clone();

        tracing::debug!("walking mount at {}", root.display());

        // Add watch for mount root
        let _ = self.watch_dir(&root);

        let mut iter = WalkDir::new(&root).follow_links(false).into_iter();

        while let Some(result) = iter.next() {
            let entry = result?;
            let abs_path = entry.path();

            // Skip mount root itself
            if abs_path == root {
                continue;
            }

            // Skip entries inside subprojects: if parent has .git (file or dir), skip this entry
            // (but the subproject dir itself is allowed since its parent won't have .git)
            // Git submodules use a .git file pointing to parent's .git/modules/
            if let Some(parent) = abs_path.parent()
                && parent != root
                && parent.join(".git").exists()
            {
                if entry.file_type().is_dir() {
                    iter.skip_current_dir();
                }
                continue;
            }

            // Determine event kind (same as notify would emit)
            let kind = if entry.file_type().is_dir() {
                EventKind::Create(CreateKind::Folder)
            } else if entry.file_type().is_file() {
                EventKind::Create(CreateKind::File)
            } else {
                continue; // Skip symlinks, etc.
            };

            // Let on_fs_event handle all the logic
            if let Some(event) = self.on_fs_event(abs_path, &kind) {
                match event {
                    FsEvent::DirIgnored => {
                        // Skip the entire subtree
                        iter.skip_current_dir();
                    }
                    _ => {
                        on_event(event)?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Initialize the notify file system watcher for this mount.
    ///
    /// Must be called BEFORE walk() so that DirCreated events can add directories.
    /// Does nothing for read-only mounts.
    ///
    /// Events are sent to the provided channel with mount root attached,
    /// avoiding the need for mount lookup in the handler.
    pub fn init_notify(&mut self, tx: Sender<MountedEvent>) -> Result<()> {
        if self.mode == MountMode::ReadOnly {
            return Ok(()); // No watcher for read-only mounts
        }

        // Capture mount root for the callback
        let mount_root = self.root.clone();
        let config = Config::default();
        let watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send((mount_root.clone(), res));
            },
            config,
        )
        .context("failed to create watcher")?;

        self.watcher = Some(watcher);
        self.watched_dirs.clear();

        Ok(())
    }

    /// Watch a directory (non-recursive).
    pub fn watch_dir(&mut self, path: &Path) -> Result<()> {
        if !self.watched_dirs.contains(path)
            && let Some(ref mut watcher) = self.watcher
        {
            watcher
                .watch(path, RecursiveMode::NonRecursive)
                .with_context(|| format!("failed to watch {}", path.display()))?;
            self.watched_dirs.insert(path.to_path_buf());
        }
        Ok(())
    }

    /// Get the number of watched directories.
    pub fn watched_count(&self) -> usize {
        self.watched_dirs.len()
    }

    /// Check if a path should be ignored according to gitignore rules.
    ///
    /// Returns false if gitignore hasn't been built yet (walk() not called).
    pub fn is_ignored(&self, path: &Path) -> bool {
        if let Some(ref gi) = self.gitignore {
            let is_dir = path.is_dir();
            gi.matched_path_or_any_parents(path, is_dir).is_ignore()
        } else {
            false
        }
    }

    /// Get a reference to the gitignore matcher (if built).
    pub fn gitignore(&self) -> Option<&Gitignore> {
        self.gitignore.as_ref()
    }

    /// Mark this mount as dirty (needs flushing).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Clear the dirty flag (after flushing).
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        // Release the lock when the mount is dropped
        if let Some(ref lock) = self.lock {
            let _ = lock.unlock();
        }
    }
}

/// Table of mounted directories.
#[derive(Debug)]
pub struct MountTable {
    /// Root of the workspace (where codeix was launched).
    workspace_root: PathBuf,
    mounts: HashMap<PathBuf, Mount>,
}

impl MountTable {
    /// Create a new mount table with the given workspace root.
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            workspace_root,
            mounts: HashMap::new(),
        }
    }

    /// Get the workspace root.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Compute the relative project path from workspace root.
    /// Returns empty string for the root project.
    pub fn relative_project(&self, project_root: &Path) -> String {
        project_root
            .strip_prefix(&self.workspace_root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default()
    }

    /// Get the absolute path for a relative project path.
    /// Returns None if the project is not mounted.
    pub fn project_root(&self, relative_path: &str) -> Option<PathBuf> {
        let abs_path = if relative_path.is_empty() {
            self.workspace_root.clone()
        } else {
            self.workspace_root.join(relative_path)
        };
        // Verify the project is actually mounted
        let canonical = abs_path.canonicalize().ok()?;
        if self.mounts.contains_key(&canonical) {
            Some(canonical)
        } else {
            None
        }
    }

    /// Check if a path is already mounted (exact match, not prefix).
    pub fn is_mounted(&self, path: &Path) -> bool {
        self.mounts.contains_key(path)
    }

    /// Mount a directory, trying RW first, falling back to RO if lock is held.
    ///
    /// Returns the mount and its mode. Use this when you want best-effort RW.
    pub fn mount(&mut self, root: impl AsRef<Path>) -> Result<&Mount> {
        let root = root
            .as_ref()
            .canonicalize()
            .with_context(|| format!("failed to canonicalize path {:?}", root.as_ref()))?;

        if self.mounts.contains_key(&root) {
            anyhow::bail!("directory already mounted: {:?}", root);
        }

        // Try RW first, fall back to RO if lock fails
        let mount = match Mount::new_rw(root.clone()) {
            Ok(m) => m,
            Err(e) => {
                // Check if it's a lock error (contains "lock" in message)
                if e.to_string().to_lowercase().contains("lock") {
                    tracing::info!(
                        "could not acquire lock on {}, mounting read-only: {}",
                        root.display(),
                        e
                    );
                    Mount::new_ro(root.clone())?
                } else {
                    return Err(e);
                }
            }
        };

        self.mounts.insert(root.clone(), mount);
        Ok(self.mounts.get(&root).unwrap())
    }

    /// Mount a directory in read-write mode (acquires exclusive lock).
    pub fn mount_rw(&mut self, root: impl AsRef<Path>) -> Result<&Mount> {
        let root = root
            .as_ref()
            .canonicalize()
            .with_context(|| format!("failed to canonicalize path {:?}", root.as_ref()))?;

        if self.mounts.contains_key(&root) {
            anyhow::bail!("directory already mounted: {:?}", root);
        }

        let mount = Mount::new_rw(root.clone())?;
        self.mounts.insert(root.clone(), mount);
        Ok(self.mounts.get(&root).unwrap())
    }

    /// Mount a directory in read-only mode (no lock).
    pub fn mount_ro(&mut self, root: impl AsRef<Path>) -> Result<&Mount> {
        let root = root
            .as_ref()
            .canonicalize()
            .with_context(|| format!("failed to canonicalize path {:?}", root.as_ref()))?;

        if self.mounts.contains_key(&root) {
            anyhow::bail!("directory already mounted: {:?}", root);
        }

        let mount = Mount::new_ro(root.clone())?;
        self.mounts.insert(root.clone(), mount);
        Ok(self.mounts.get(&root).unwrap())
    }

    /// Find the mount that contains a given path (longest prefix match).
    /// Canonicalizes the path first to handle symlinks.
    pub fn find_mount(&self, path: &Path) -> Option<&Mount> {
        let path = path.canonicalize().ok()?;
        self.find_mount_canonical(&path)
    }

    /// Find the mount that contains a given path (longest prefix match).
    /// Assumes the path is already canonical - use for hot paths to avoid
    /// repeated canonicalization overhead.
    pub fn find_mount_canonical(&self, path: &Path) -> Option<&Mount> {
        self.mounts
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, mount)| mount)
    }

    /// Find the mount that contains a given path (mutable, longest prefix match).
    /// Canonicalizes the path first to handle symlinks.
    pub fn find_mount_mut(&mut self, path: &Path) -> Option<&mut Mount> {
        let path = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => return None,
        };
        self.find_mount_mut_canonical(&path)
    }

    /// Find the mount that contains a given path (mutable, longest prefix match).
    /// Assumes the path is already canonical - use for hot paths to avoid
    /// repeated canonicalization overhead.
    pub fn find_mount_mut_canonical(&mut self, path: &Path) -> Option<&mut Mount> {
        // Find the key with longest prefix match
        let key = self
            .mounts
            .keys()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
            .cloned();

        key.and_then(move |k| self.mounts.get_mut(&k))
    }

    /// Unmount a directory (releases lock if held).
    pub fn unmount(&mut self, root: impl AsRef<Path>) -> Result<()> {
        let root = root
            .as_ref()
            .canonicalize()
            .with_context(|| format!("failed to canonicalize path {:?}", root.as_ref()))?;

        if self.mounts.remove(&root).is_none() {
            anyhow::bail!("directory not mounted: {:?}", root);
        }
        Ok(())
    }

    /// Unmount a directory by its exact path (no canonicalization).
    /// Use this when the directory may have been deleted.
    pub fn unmount_path(&mut self, root: &Path) -> bool {
        self.mounts.remove(root).is_some()
    }

    /// Iterate over all mounts.
    pub fn iter(&self) -> impl Iterator<Item = (&PathBuf, &Mount)> {
        self.mounts.iter()
    }

    /// Iterate over all mounts (mutable).
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&PathBuf, &mut Mount)> {
        self.mounts.iter_mut()
    }

    /// Mark a path's mount as dirty.
    pub fn mark_dirty(&mut self, path: &Path) -> bool {
        if let Some(mount) = self.find_mount_mut(path) {
            mount.mark_dirty();
            true
        } else {
            false
        }
    }

    /// Mark a path's mount as dirty (assumes path is already canonical).
    pub fn mark_dirty_canonical(&mut self, path: &Path) -> bool {
        if let Some(mount) = self.find_mount_mut_canonical(path) {
            mount.mark_dirty();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_mount_rw() {
        let tmp = TempDir::new().unwrap();
        let mut table = MountTable::new(tmp.path().to_path_buf());

        let mount = table.mount_rw(tmp.path()).unwrap();
        assert_eq!(mount.mode, MountMode::ReadWrite);
        assert!(!mount.dirty);

        // .codeindex/index.json should exist
        assert!(tmp.path().join(".codeindex/index.json").exists());
    }

    #[test]
    fn test_mount_ro() {
        let tmp = TempDir::new().unwrap();
        let mut table = MountTable::new(tmp.path().to_path_buf());

        let mount = table.mount_ro(tmp.path()).unwrap();
        assert_eq!(mount.mode, MountMode::ReadOnly);
        assert!(!mount.dirty);
    }

    #[test]
    fn test_find_mount_longest_prefix() {
        let tmp = TempDir::new().unwrap();
        let subdir = tmp.path().join("sub");
        fs::create_dir(&subdir).unwrap();

        let mut table = MountTable::new(tmp.path().to_path_buf());
        table.mount_ro(tmp.path()).unwrap();
        table.mount_ro(&subdir).unwrap();

        // File in subdir should match subdir mount
        let file_in_sub = subdir.join("file.rs");
        fs::write(&file_in_sub, "").unwrap();

        let mount = table.find_mount(&file_in_sub).unwrap();
        assert_eq!(
            mount.root.canonicalize().unwrap(),
            subdir.canonicalize().unwrap()
        );

        // File in root should match root mount
        let file_in_root = tmp.path().join("file.rs");
        fs::write(&file_in_root, "").unwrap();

        let mount = table.find_mount(&file_in_root).unwrap();
        assert_eq!(
            mount.root.canonicalize().unwrap(),
            tmp.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn test_unmount() {
        let tmp = TempDir::new().unwrap();
        let mut table = MountTable::new(tmp.path().to_path_buf());

        table.mount_rw(tmp.path()).unwrap();
        assert!(table.find_mount(tmp.path()).is_some());

        table.unmount(tmp.path()).unwrap();
        assert!(table.find_mount(tmp.path()).is_none());
    }

    #[test]
    fn test_double_mount_fails() {
        let tmp = TempDir::new().unwrap();
        let mut table = MountTable::new(tmp.path().to_path_buf());

        table.mount_rw(tmp.path()).unwrap();
        assert!(table.mount_rw(tmp.path()).is_err());
    }

    #[test]
    fn test_mark_dirty() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("file.rs");
        fs::write(&file, "").unwrap();

        let mut table = MountTable::new(tmp.path().to_path_buf());
        table.mount_rw(tmp.path()).unwrap();

        assert!(table.mark_dirty(&file));

        let mount = table.find_mount(&file).unwrap();
        assert!(mount.dirty);
    }

    #[test]
    fn test_gitignore_respected() {
        let tmp = TempDir::new().unwrap();

        // Create .gitignore
        fs::write(tmp.path().join(".gitignore"), "ignored/\n*.log\n").unwrap();

        // Create files
        fs::create_dir(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/main.rs"), "fn main() {}").unwrap();
        fs::create_dir(tmp.path().join("ignored")).unwrap();
        fs::write(tmp.path().join("ignored/secret.txt"), "secret").unwrap();
        fs::write(tmp.path().join("debug.log"), "log").unwrap();

        let mut table = MountTable::new(tmp.path().to_path_buf());
        table.mount_ro(tmp.path()).unwrap();

        let mount = table.find_mount_mut(tmp.path()).unwrap();

        // Collect walked files
        let mut files = Vec::new();
        mount
            .walk(|event| {
                if let FsEvent::FileAdded { path, .. } = event {
                    files.push(path);
                }
                Ok(())
            })
            .unwrap();

        // Should include src/main.rs
        assert!(files.contains(&"src/main.rs".to_string()));

        // Should NOT include ignored files
        assert!(!files.iter().any(|f| f.contains("ignored")));
        assert!(!files.iter().any(|f| f.ends_with(".log")));
    }

    #[test]
    fn test_symlinks_not_followed() {
        // Issue #36: symlinks in node_modules caused CPU spin
        // Verify that follow_links(false) prevents infinite loops
        let tmp = TempDir::new().unwrap();

        // Create a directory structure with symlinks
        fs::create_dir(tmp.path().join("real_dir")).unwrap();
        fs::write(tmp.path().join("real_dir/file.txt"), "content").unwrap();

        // Create a symlink pointing to parent (would cause infinite loop if followed)
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            // Symlink to parent directory
            symlink(tmp.path(), tmp.path().join("real_dir/parent_link")).unwrap();
            // Symlink to self
            symlink(
                tmp.path().join("real_dir"),
                tmp.path().join("real_dir/self_link"),
            )
            .unwrap();
        }

        let mut table = MountTable::new(tmp.path().to_path_buf());
        table.mount_ro(tmp.path()).unwrap();

        let mount = table.find_mount_mut(tmp.path()).unwrap();

        // Walk should complete without hanging (symlinks not followed)
        let mut files = Vec::new();
        mount
            .walk(|event| {
                if let FsEvent::FileAdded { path, .. } = event {
                    files.push(path);
                }
                Ok(())
            })
            .unwrap();

        // Should find the real file
        assert!(files.contains(&"real_dir/file.txt".to_string()));

        // Should NOT have followed symlinks (no duplicates or infinite recursion)
        assert_eq!(
            files.iter().filter(|f| *f == "real_dir/file.txt").count(),
            1
        );
    }

    #[test]
    fn test_walk_adds_watches_internally() {
        // Verify walk adds watches internally (no DirAdded events emitted)
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().canonicalize().unwrap();

        fs::create_dir_all(tmp_path.join("src/components")).unwrap();
        fs::create_dir_all(tmp_path.join("tests")).unwrap();
        fs::write(tmp_path.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(tmp_path.join("src/components/button.rs"), "// button").unwrap();

        let mut table = MountTable::new(tmp_path.clone());
        table.mount_rw(&tmp_path).unwrap(); // RW to get watcher

        let mount = table.find_mount_mut(&tmp_path).unwrap();

        // Init watcher
        let (tx, _rx) = std::sync::mpsc::channel::<MountedEvent>();
        mount.init_notify(tx).unwrap();

        let mut file_count = 0;
        mount
            .walk(|event| {
                match event {
                    FsEvent::FileAdded { .. } => file_count += 1,
                    FsEvent::ProjectAdded { .. } => {}
                    FsEvent::FileRemoved { .. }
                    | FsEvent::ProjectRemoved { .. }
                    | FsEvent::DirIgnored => {}
                }
                Ok(())
            })
            .unwrap();

        // Should have files
        assert!(file_count >= 2);

        // Should have watches registered internally
        assert!(mount.watched_count() >= 4); // root, src, components, tests
    }

    #[test]
    fn test_gitignore_built_on_mount() {
        // Verify gitignore is available immediately after mounting
        let tmp = TempDir::new().unwrap();
        // Canonicalize for macOS where /var -> /private/var
        let tmp_path = tmp.path().canonicalize().unwrap();

        fs::write(tmp_path.join(".gitignore"), "target/\n*.log\n").unwrap();
        fs::create_dir(tmp_path.join("src")).unwrap();
        fs::write(tmp_path.join("src/main.rs"), "fn main() {}").unwrap();

        let mut table = MountTable::new(tmp_path.clone());
        table.mount_ro(&tmp_path).unwrap();

        let mount = table.find_mount_mut(&tmp_path).unwrap();

        // Gitignore is built during mount creation
        assert!(mount.gitignore().is_some());

        // Can use is_ignored() for watcher filtering
        assert!(mount.is_ignored(&tmp_path.join("target/debug/foo")));
        assert!(mount.is_ignored(&tmp_path.join("app.log")));
        assert!(!mount.is_ignored(&tmp_path.join("src/main.rs")));
    }

    #[test]
    fn test_nested_gitignore_files() {
        // Verify nested .gitignore files are respected
        let tmp = TempDir::new().unwrap();

        // Root .gitignore
        fs::write(tmp.path().join(".gitignore"), "*.log\n").unwrap();

        // Nested directory with its own .gitignore
        fs::create_dir_all(tmp.path().join("subdir")).unwrap();
        fs::write(tmp.path().join("subdir/.gitignore"), "local_ignore/\n").unwrap();

        // Create files
        fs::write(tmp.path().join("root.log"), "log").unwrap();
        fs::write(tmp.path().join("subdir/code.rs"), "// code").unwrap();
        fs::write(tmp.path().join("subdir/sub.log"), "log").unwrap();
        fs::create_dir(tmp.path().join("subdir/local_ignore")).unwrap();
        fs::write(tmp.path().join("subdir/local_ignore/secret.txt"), "secret").unwrap();

        let mut table = MountTable::new(tmp.path().to_path_buf());
        table.mount_ro(tmp.path()).unwrap();

        let mount = table.find_mount_mut(tmp.path()).unwrap();

        let mut files = Vec::new();
        mount
            .walk(|event| {
                if let FsEvent::FileAdded { path, .. } = event {
                    files.push(path);
                }
                Ok(())
            })
            .unwrap();

        // Should include code.rs
        assert!(files.contains(&"subdir/code.rs".to_string()));

        // Should NOT include .log files (from root .gitignore)
        assert!(!files.iter().any(|f| f.ends_with(".log")));

        // Should NOT include local_ignore/ (from nested .gitignore)
        assert!(!files.iter().any(|f| f.contains("local_ignore")));
    }

    #[test]
    fn test_on_dir_removed_emits_project_removed() {
        use notify::event::{EventKind, RemoveKind};

        let tmp = TempDir::new().unwrap();
        // Create a .git directory to simulate a subproject
        let git_dir = tmp.path().join("subproject/.git");
        fs::create_dir_all(&git_dir).unwrap();

        let mut table = MountTable::new(tmp.path().to_path_buf());
        table.mount_ro(tmp.path()).unwrap();
        let mount = table.find_mount_mut(tmp.path()).unwrap();

        // Simulate .git directory removal event
        let event = mount.on_fs_event(&git_dir, &EventKind::Remove(RemoveKind::Folder));

        // Should emit ProjectRemoved for the subproject root
        match event {
            Some(FsEvent::ProjectRemoved { root }) => {
                assert_eq!(root, tmp.path().join("subproject"));
            }
            other => panic!("expected ProjectRemoved, got {:?}", other),
        }
    }

    #[test]
    fn test_on_dir_removed_regular_dir_no_event() {
        use notify::event::{EventKind, RemoveKind};

        let tmp = TempDir::new().unwrap();
        let regular_dir = tmp.path().join("some_dir");
        fs::create_dir_all(&regular_dir).unwrap();

        let mut table = MountTable::new(tmp.path().to_path_buf());
        table.mount_ro(tmp.path()).unwrap();
        let mount = table.find_mount_mut(tmp.path()).unwrap();

        // Simulate regular directory removal event
        let event = mount.on_fs_event(&regular_dir, &EventKind::Remove(RemoveKind::Folder));

        // Should not emit any event for regular directories
        assert!(event.is_none());
    }
}
