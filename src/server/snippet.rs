use std::fs;
use std::path::PathBuf;

/// Extracts code snippets from source files at query time.
///
/// Resolves file paths relative to workspace root and reads
/// the specified line ranges from disk.
#[derive(Clone)]
pub struct SnippetExtractor {
    workspace_root: PathBuf,
}

impl SnippetExtractor {
    /// Create a new snippet extractor with the given workspace root.
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }

    /// Resolve absolute file path from project and relative file path.
    fn resolve_file_path(&self, project: &str, file: &str) -> PathBuf {
        if project.is_empty() {
            self.workspace_root.join(file)
        } else {
            self.workspace_root.join(project).join(file)
        }
    }

    /// Extract a code snippet from a file.
    ///
    /// # Arguments
    /// * `project` - Relative project path from workspace root (e.g., "libs/utils", "" for root)
    /// * `file` - File path relative to project root
    /// * `line_start` - Starting line number (1-indexed, inclusive)
    /// * `line_end` - Ending line number (1-indexed, inclusive)
    /// * `context_lines` - Number of lines to include:
    ///   - `0`: No snippet (returns None immediately)
    ///   - `-1`: All lines in range (blank lines skipped)
    ///   - `N > 0`: First N non-blank lines
    ///
    /// Returns `None` if file cannot be read or context_lines is 0.
    /// Adds "..." suffix when truncated.
    pub fn extract_snippet(
        &self,
        project: &str,
        file: &str,
        line_start: u32,
        line_end: u32,
        context_lines: i32,
    ) -> Option<String> {
        // Early return if snippets disabled
        if context_lines == 0 {
            return None;
        }

        // Resolve absolute path and read file content
        let file_path = self.resolve_file_path(project, file);
        let content = fs::read_to_string(&file_path).ok()?;

        // Split into lines and extract range
        let all_lines: Vec<&str> = content.lines().collect();

        // Convert 1-indexed to 0-indexed, clamp to file bounds
        let start_idx = (line_start.saturating_sub(1) as usize).min(all_lines.len());
        let end_idx = (line_end as usize).min(all_lines.len());

        if start_idx >= end_idx {
            return None;
        }

        let range_lines = &all_lines[start_idx..end_idx];

        // Filter blank lines (empty or whitespace-only)
        let non_blank_lines: Vec<&str> = range_lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .copied()
            .collect();

        // Apply line limit
        let (final_lines, truncated) = if context_lines < 0 {
            // All non-blank lines
            (non_blank_lines, false)
        } else {
            let limit = context_lines as usize;
            let truncated = non_blank_lines.len() > limit;
            let lines: Vec<&str> = non_blank_lines.into_iter().take(limit).collect();
            (lines, truncated)
        };

        // Join lines and add ellipsis if truncated
        let mut result = final_lines.join("\n");
        if truncated {
            result.push_str("\n...");
        }

        Some(result)
    }

    /// Check if a file exists (for filtering symbols with missing files).
    pub fn file_exists(&self, project: &str, file: &str) -> bool {
        self.resolve_file_path(project, file).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_file(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
        let file_path = dir.join(name);
        fs::write(&file_path, content).unwrap();
        file_path
    }

    #[test]
    fn test_extract_snippet_basic() {
        let tmp = TempDir::new().unwrap();
        create_test_file(
            tmp.path(),
            "test.rs",
            "fn main() {\n    println!(\"hello\");\n}\n",
        );

        let extractor = SnippetExtractor::new(tmp.path().to_path_buf());
        let snippet = extractor.extract_snippet("", "test.rs", 1, 3, -1);

        assert_eq!(
            snippet,
            Some("fn main() {\n    println!(\"hello\");\n}".to_string())
        );
    }

    #[test]
    fn test_extract_snippet_limit_lines() {
        let tmp = TempDir::new().unwrap();
        create_test_file(tmp.path(), "test.rs", "line1\nline2\nline3\nline4\nline5\n");

        let extractor = SnippetExtractor::new(tmp.path().to_path_buf());
        let snippet = extractor.extract_snippet("", "test.rs", 1, 5, 2);

        assert_eq!(snippet, Some("line1\nline2\n...".to_string()));
    }

    #[test]
    fn test_extract_snippet_skip_blank_lines() {
        let tmp = TempDir::new().unwrap();
        create_test_file(tmp.path(), "test.rs", "line1\n\n  \nline2\n\nline3\n");

        let extractor = SnippetExtractor::new(tmp.path().to_path_buf());
        let snippet = extractor.extract_snippet("", "test.rs", 1, 6, -1);

        assert_eq!(snippet, Some("line1\nline2\nline3".to_string()));
    }

    #[test]
    fn test_extract_snippet_disabled() {
        let tmp = TempDir::new().unwrap();
        create_test_file(tmp.path(), "test.rs", "fn main() {}");

        let extractor = SnippetExtractor::new(tmp.path().to_path_buf());
        let snippet = extractor.extract_snippet("", "test.rs", 1, 1, 0);

        assert_eq!(snippet, None);
    }

    #[test]
    fn test_extract_snippet_file_not_found() {
        let tmp = TempDir::new().unwrap();
        let extractor = SnippetExtractor::new(tmp.path().to_path_buf());
        let snippet = extractor.extract_snippet("", "missing.rs", 1, 1, 10);

        assert_eq!(snippet, None);
    }

    #[test]
    fn test_extract_snippet_with_project() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("myproject");
        fs::create_dir(&project_dir).unwrap();
        fs::write(project_dir.join("lib.rs"), "pub fn test() {}").unwrap();

        let extractor = SnippetExtractor::new(tmp.path().to_path_buf());
        let snippet = extractor.extract_snippet("myproject", "lib.rs", 1, 1, -1);

        assert_eq!(snippet, Some("pub fn test() {}".to_string()));
    }

    #[test]
    fn test_file_exists() {
        let tmp = TempDir::new().unwrap();
        create_test_file(tmp.path(), "exists.rs", "content");

        let extractor = SnippetExtractor::new(tmp.path().to_path_buf());

        assert!(extractor.file_exists("", "exists.rs"));
        assert!(!extractor.file_exists("", "missing.rs"));
    }

    #[test]
    fn test_file_exists_with_project() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("proj");
        fs::create_dir(&project_dir).unwrap();
        fs::write(project_dir.join("lib.rs"), "content").unwrap();

        let extractor = SnippetExtractor::new(tmp.path().to_path_buf());

        assert!(extractor.file_exists("proj", "lib.rs"));
        assert!(!extractor.file_exists("proj", "missing.rs"));
        assert!(!extractor.file_exists("other", "lib.rs"));
    }
}
