use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use walkdir::WalkDir;

use super::MountEvent;

/// Entries to always skip during walk (never indexed).
/// These are either internal directories, IDE config, or OS cruft.
pub const SKIP_ENTRIES: &[&str] = &[
    // Git internals
    ".git",
    // Our own output
    ".codeindex",
    // Editor/IDE directories
    ".vscode",
    ".idea",
    ".vs",
    // OS cruft
    ".DS_Store",
    ".Spotlight-V100",
    ".Trashes",
];

/// Walk all files in a mount root, emitting events via callback.
///
/// Builds gitignore incrementally during the walk - when entering a directory,
/// checks for `.gitignore` and adds it before processing contents.
///
/// Stops at subproject boundaries (directories containing `.git/`) and
/// emits `MountEvent::Subproject` for each discovered subproject.
///
/// Returns the built gitignore for use by the watcher.
pub fn walk_mount<F>(root: &Path, mut on_event: F) -> Result<Gitignore>
where
    F: FnMut(MountEvent) -> Result<()>,
{
    let mut subprojects: HashSet<std::path::PathBuf> = HashSet::new();
    tracing::debug!("walking mount at {}", root.display());

    // Emit DirCreated for mount root so watcher can add it
    on_event(MountEvent::DirCreated { path: root.into() })?;

    // Initialize gitignore builder
    let mut builder = GitignoreBuilder::new(root);

    // Add .git/info/exclude if it exists
    let exclude_path = root.join(".git/info/exclude");
    if exclude_path.exists() {
        builder.add(&exclude_path);
    }

    // Add root .gitignore if it exists
    let root_gitignore = root.join(".gitignore");
    if root_gitignore.exists() {
        builder.add(&root_gitignore);
    }

    // Build initial gitignore
    let gitignore = builder
        .build()
        .with_context(|| format!("failed to build gitignore for {:?}", root))?;

    // Use RefCell to allow mutation inside filter_entry closure
    let gitignore_cell = RefCell::new(gitignore);
    let builder_cell = RefCell::new(builder);

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let path = e.path();

            // Always allow mount root
            if path == root {
                return true;
            }

            // Skip entries that should never be indexed (see SKIP_ENTRIES)
            let name = e.file_name().to_string_lossy();
            if SKIP_ENTRIES.contains(&name.as_ref()) {
                return false;
            }

            // For directories: check for .git/ (subproject) and .gitignore
            if e.file_type().is_dir() {
                // Check if this is a subproject - don't descend into it
                if path.join(".git").is_dir() {
                    return true; // Allow the dir itself, we'll emit Subproject event
                }

                // Check for .gitignore in this directory and add to builder
                let dir_gitignore = path.join(".gitignore");
                if dir_gitignore.exists() {
                    let mut builder = builder_cell.borrow_mut();
                    builder.add(&dir_gitignore);
                    if let Ok(new_gi) = builder.build() {
                        *gitignore_cell.borrow_mut() = new_gi;
                    }
                }
            }

            // Check gitignore (with updated rules)
            let gi = gitignore_cell.borrow();
            let is_dir = e.file_type().is_dir();
            !gi.matched_path_or_any_parents(path, is_dir).is_ignore()
        })
    {
        let entry = entry?;
        let abs_path = entry.path();

        // Skip mount root itself
        if abs_path == root {
            continue;
        }

        // For directories, check if they're subprojects or emit DirCreated
        if entry.file_type().is_dir() {
            // Check if this directory is a subproject (has .git/)
            if abs_path.join(".git").is_dir() {
                tracing::debug!("found subproject: {}", abs_path.display());
                subprojects.insert(abs_path.to_path_buf());
                on_event(MountEvent::Subproject {
                    root: abs_path.to_path_buf(),
                })?;
            } else {
                // Emit DirCreated so watcher can add this directory
                on_event(MountEvent::DirCreated {
                    path: abs_path.to_path_buf(),
                })?;
            }
            continue;
        }

        // Skip files inside subprojects
        if subprojects.iter().any(|sp| abs_path.starts_with(sp)) {
            continue;
        }

        // Only process regular files
        if !entry.file_type().is_file() {
            continue;
        }

        // Compute relative path
        let rel_path = abs_path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();

        // Skip .codeindex/ files
        if rel_path.starts_with(".codeindex/") || rel_path == ".codeindex" {
            continue;
        }

        on_event(MountEvent::File {
            abs_path: abs_path.to_path_buf(),
            rel_path,
        })?;
    }

    Ok(gitignore_cell.into_inner())
}
