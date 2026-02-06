use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs2::FileExt;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// Mount mode determines whether the index can be written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountMode {
    /// Read-write: acquires exclusive flock, allows modifications.
    ReadWrite,
    /// Read-only: no lock, index data is immutable.
    ReadOnly,
}

/// A single mounted directory with its index.
#[derive(Debug)]
pub struct Mount {
    /// Root directory of the mount.
    pub root: PathBuf,
    /// Mount mode (read-write or read-only).
    pub mode: MountMode,
    /// File lock handle (only present for ReadWrite mounts).
    lock: Option<File>,
    /// Whether the index has been modified and needs flushing.
    pub dirty: bool,
    /// Gitignore rules for this mount.
    gitignore: Gitignore,
}

impl Mount {
    /// Create a new read-only mount (no lock acquired).
    fn new_ro(root: PathBuf) -> Result<Self> {
        let gitignore = build_gitignore(&root)?;
        Ok(Self {
            root,
            mode: MountMode::ReadOnly,
            lock: None,
            dirty: false,
            gitignore,
        })
    }

    /// Create a new read-write mount with exclusive flock.
    fn new_rw(root: PathBuf) -> Result<Self> {
        let gitignore = build_gitignore(&root)?;

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

        Ok(Self {
            root,
            mode: MountMode::ReadWrite,
            lock: Some(lock_file),
            dirty: false,
            gitignore,
        })
    }

    /// Check if a path should be ignored according to gitignore rules.
    pub fn is_ignored(&self, path: &Path) -> bool {
        let is_dir = path.is_dir();
        self.gitignore.matched(path, is_dir).is_ignore()
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

/// Build gitignore rules for a directory.
fn build_gitignore(root: &Path) -> Result<Gitignore> {
    let mut builder = GitignoreBuilder::new(root);

    // Add .gitignore if it exists
    let gitignore_path = root.join(".gitignore");
    if gitignore_path.exists() {
        builder.add(&gitignore_path);
    }

    // Add .git/info/exclude if it exists
    let exclude_path = root.join(".git/info/exclude");
    if exclude_path.exists() {
        builder.add(&exclude_path);
    }

    builder
        .build()
        .with_context(|| format!("failed to build gitignore for {:?}", root))
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

    /// Check if a path is already mounted (exact match, not prefix).
    pub fn is_mounted(&self, path: &Path) -> bool {
        self.mounts.contains_key(path)
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
}
