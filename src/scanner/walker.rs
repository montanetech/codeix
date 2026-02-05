use std::path::{Path, PathBuf};

use anyhow::Result;
use ignore::WalkBuilder;

/// Walk a directory tree using the `ignore` crate (respects `.gitignore`),
/// returning all file paths that are not ignored.
pub fn walk_directory(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    for entry in WalkBuilder::new(root)
        .hidden(true) // skip hidden files/dirs (dotfiles)
        .git_ignore(true) // respect .gitignore
        .git_global(true)
        .git_exclude(true)
        .build()
    {
        let entry = entry?;
        // Only collect files, not directories
        if entry.file_type().map_or(false, |ft| ft.is_file()) {
            files.push(entry.into_path());
        }
    }

    files.sort();
    Ok(files)
}
