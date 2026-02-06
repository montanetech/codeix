pub mod handler;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use ignore::gitignore::Gitignore;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

/// A file watcher that respects .gitignore rules.
/// Uses non-recursive watching on each directory to avoid watching ignored paths.
pub struct GitignoreWatcher {
    watcher: RecommendedWatcher,
    watched_dirs: HashSet<PathBuf>,
}

impl GitignoreWatcher {
    /// Create a new watcher for the given root directory.
    /// Walks the tree respecting .gitignore and watches each non-ignored directory.
    pub fn new(root: &Path, tx: Sender<Result<Event, notify::Error>>) -> Result<Self> {
        let config = Config::default();
        let watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            config,
        )
        .context("failed to create watcher")?;

        let watched_dirs = HashSet::new();
        let mut instance = Self {
            watcher,
            watched_dirs,
        };

        // Walk with gitignore awareness, watch only non-ignored directories
        for entry in WalkBuilder::new(root)
            .hidden(false) // see .git/ for project discovery
            .git_ignore(true) // respect .gitignore
            .git_global(true)
            .git_exclude(true)
            .build()
        {
            let entry = entry?;
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                let path = entry.path();
                // Skip .git internals but watch .git itself (for project discovery)
                let is_inside_git = path.components().any(|c| {
                    if let std::path::Component::Normal(s) = c {
                        s == ".git"
                    } else {
                        false
                    }
                }) && path.file_name() != Some(std::ffi::OsStr::new(".git"));

                if !is_inside_git {
                    instance.watch_dir(path)?;
                }
            }
        }

        Ok(instance)
    }

    /// Watch a directory (non-recursive).
    fn watch_dir(&mut self, path: &Path) -> Result<()> {
        if !self.watched_dirs.contains(path) {
            self.watcher
                .watch(path, RecursiveMode::NonRecursive)
                .with_context(|| format!("failed to watch {}", path.display()))?;
            self.watched_dirs.insert(path.to_path_buf());
        }
        Ok(())
    }

    /// Called when a new directory is created - check if it should be watched.
    pub fn on_dir_created(&mut self, path: &Path, gitignore: &Gitignore) -> Result<()> {
        // Check if this directory should be ignored
        if gitignore
            .matched_path_or_any_parents(path, true)
            .is_ignore()
        {
            return Ok(()); // ignored, don't watch
        }
        self.watch_dir(path)
    }

    /// Watch a directory if it's valid (exists and not already watched).
    /// This is used for dynamically adding watches when directories are created.
    pub fn watch_dir_if_valid(&mut self, path: &Path) -> Result<()> {
        if path.is_dir() {
            self.watch_dir(path)
        } else {
            Ok(())
        }
    }

    /// Called when a directory is deleted - stop watching.
    pub fn on_dir_removed(&mut self, path: &Path) {
        if self.watched_dirs.remove(path) {
            let _ = self.watcher.unwatch(path); // may fail if already gone
        }
    }

    /// Get the number of watched directories.
    pub fn watched_count(&self) -> usize {
        self.watched_dirs.len()
    }
}
