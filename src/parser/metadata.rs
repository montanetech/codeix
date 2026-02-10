//! File metadata extraction (title and description).
//!
//! Extracts file-level metadata from source code for improved search relevance.
//! Each language has specific conventions for documenting files:
//! - Markdown: YAML frontmatter or first heading/paragraph
//! - Python: Module docstring
//! - Rust: `//!` module docs
//! - JavaScript/TypeScript: Top-level JSDoc
//! - Go: Package comment
//! - Java: Top-level Javadoc
//! - C/C++: File header comment
//! - Ruby: Top file comment
//! - C#: Top-level XML doc comment

use tree_sitter::{Parser, Tree};

use crate::parser::helpers::{node_text, strip_string_quotes};
use crate::parser::languages::get_language;

/// File metadata: title and description.
#[derive(Debug, Clone, Default)]
pub struct FileMetadata {
    pub title: Option<String>,
    pub description: Option<String>,
}

impl FileMetadata {
    pub fn new(title: Option<String>, description: Option<String>) -> Self {
        Self { title, description }
    }

    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.description.is_none()
    }
}

/// Extract file metadata (title and description) from source code.
///
/// Returns `FileMetadata` with title and/or description if found.
/// Description is truncated to the first sentence or line.
pub fn extract_file_metadata(source: &[u8], language: &str) -> FileMetadata {
    let result = match language {
        #[cfg(feature = "lang-markdown")]
        "markdown" => extract_markdown_metadata(source),

        #[cfg(feature = "lang-python")]
        "python" => extract_with_tree(source, language, extract_python_metadata),

        #[cfg(feature = "lang-rust")]
        "rust" => extract_with_tree(source, language, extract_rust_metadata),

        #[cfg(feature = "lang-javascript")]
        "javascript" => extract_with_tree(source, language, extract_js_metadata),

        #[cfg(feature = "lang-typescript")]
        "typescript" | "tsx" => extract_with_tree(source, language, extract_js_metadata),

        #[cfg(feature = "lang-go")]
        "go" => extract_with_tree(source, language, extract_go_metadata),

        #[cfg(feature = "lang-java")]
        "java" => extract_with_tree(source, language, extract_java_metadata),

        #[cfg(feature = "lang-c")]
        "c" => extract_with_tree(source, language, extract_c_metadata),

        #[cfg(feature = "lang-cpp")]
        "cpp" => extract_with_tree(source, language, extract_c_metadata),

        #[cfg(feature = "lang-ruby")]
        "ruby" => extract_with_tree(source, language, extract_ruby_metadata),

        #[cfg(feature = "lang-csharp")]
        "csharp" => extract_with_tree(source, language, extract_csharp_metadata),

        _ => FileMetadata::default(),
    };

    // Post-process: truncate description to first sentence/line
    FileMetadata {
        title: result.title.map(|t| truncate_to_line(&t)),
        description: result.description.map(|d| truncate_to_sentence(&d)),
    }
}

/// Helper to parse source and extract metadata using a tree-based extractor.
fn extract_with_tree<F>(source: &[u8], language: &str, extractor: F) -> FileMetadata
where
    F: FnOnce(&Tree, &[u8]) -> FileMetadata,
{
    let lang = match get_language(language) {
        Ok(l) => l,
        Err(_) => return FileMetadata::default(),
    };

    let mut parser = Parser::new();
    if parser.set_language(&lang).is_err() {
        return FileMetadata::default();
    }

    match parser.parse(source, None) {
        Some(tree) => extractor(&tree, source),
        None => FileMetadata::default(),
    }
}

/// Truncate text to the first line (up to newline).
fn truncate_to_line(text: &str) -> String {
    let trimmed = text.trim();
    match trimmed.find('\n') {
        Some(idx) => trimmed.get(..idx).unwrap_or(trimmed).trim().to_string(),
        None => trimmed.to_string(),
    }
}

/// Truncate text to the first sentence (up to period, or first line).
fn truncate_to_sentence(text: &str) -> String {
    let trimmed = text.trim();

    // First, try to find a sentence boundary (. followed by space or end)
    for (i, c) in trimmed.char_indices() {
        if c == '.' {
            let next_idx = i + 1;
            if next_idx >= trimmed.len() {
                // Period at end
                return trimmed.to_string();
            }
            let next_char = trimmed.get(next_idx..).and_then(|s| s.chars().next());
            if next_char.is_none_or(|c| c.is_whitespace()) {
                // Sentence boundary found
                return trimmed.get(..=i).unwrap_or(trimmed).trim().to_string();
            }
        }
    }

    // No sentence boundary, fall back to first line
    truncate_to_line(trimmed)
}

// ---------------------------------------------------------------------------
// Markdown metadata extraction
// ---------------------------------------------------------------------------

#[cfg(feature = "lang-markdown")]
fn extract_markdown_metadata(source: &[u8]) -> FileMetadata {
    let text = match std::str::from_utf8(source) {
        Ok(s) => s,
        Err(_) => return FileMetadata::default(),
    };

    // Try YAML frontmatter first
    if let Some(meta) = extract_yaml_frontmatter(text)
        && !meta.is_empty()
    {
        return meta;
    }

    // Fallback: first heading as title, first paragraph as description
    let mut title = None;
    let mut description = None;
    let mut in_code_block = false;

    for line in text.lines() {
        let trimmed = line.trim();

        // Skip code blocks
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }

        // Skip empty lines
        if trimmed.is_empty() {
            continue;
        }

        // First ATX heading as title
        if title.is_none() && trimmed.starts_with('#') {
            let heading_text = trimmed.trim_start_matches('#').trim();
            // Strip optional closing hashes
            let heading_text = heading_text.trim_end_matches('#').trim();
            if !heading_text.is_empty() {
                title = Some(heading_text.to_string());
                continue;
            }
        }

        // First non-heading paragraph as description
        if title.is_some() && description.is_none() && !trimmed.starts_with('#') {
            description = Some(trimmed.to_string());
            break;
        }
    }

    FileMetadata::new(title, description)
}

#[cfg(feature = "lang-markdown")]
fn extract_yaml_frontmatter(text: &str) -> Option<FileMetadata> {
    // YAML frontmatter starts with --- on first line
    if !text.starts_with("---") {
        return None;
    }

    // Find closing ---
    let content = text.get(3..)?;
    let end_idx = content.find("\n---")?;
    let yaml_content = content.get(..end_idx)?;

    let mut title = None;
    let mut description = None;

    for line in yaml_content.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("title:") {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                title = Some(value.to_string());
            }
        } else if let Some(value) = trimmed.strip_prefix("description:") {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                description = Some(value.to_string());
            }
        }
    }

    Some(FileMetadata::new(title, description))
}

// ---------------------------------------------------------------------------
// Python metadata extraction
// ---------------------------------------------------------------------------

#[cfg(feature = "lang-python")]
fn extract_python_metadata(tree: &Tree, source: &[u8]) -> FileMetadata {
    let root = tree.root_node();
    let mut cursor = root.walk();

    // Look for module docstring: first expression_statement containing a string
    for child in root.children(&mut cursor) {
        if child.kind() == "expression_statement"
            && let Some(string_node) = child.child(0)
            && (string_node.kind() == "string" || string_node.kind() == "concatenated_string")
        {
            let raw = node_text(string_node, source);
            let text = strip_string_quotes(&raw);
            let text = text.trim();

            if text.is_empty() {
                continue;
            }

            // First line is title, rest is description
            return split_docstring(text);
        }
        // Stop at first non-docstring statement
        if child.kind() != "comment" && child.kind() != "expression_statement" {
            break;
        }
    }

    FileMetadata::default()
}

// ---------------------------------------------------------------------------
// Rust metadata extraction
// ---------------------------------------------------------------------------

#[cfg(feature = "lang-rust")]
fn extract_rust_metadata(tree: &Tree, source: &[u8]) -> FileMetadata {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut doc_lines: Vec<String> = Vec::new();

    // Look for //! module doc comments at the start of the file
    for child in root.children(&mut cursor) {
        if child.kind() == "line_comment" {
            let text = node_text(child, source);
            if let Some(doc) = text.strip_prefix("//!") {
                doc_lines.push(doc.trim().to_string());
            } else {
                // Regular comment, stop looking for module docs
                break;
            }
        } else if child.kind() == "block_comment" {
            let text = node_text(child, source);
            if text.starts_with("/*!") {
                // Block doc comment
                let content = text
                    .strip_prefix("/*!")
                    .and_then(|s| s.strip_suffix("*/"))
                    .unwrap_or(&text);
                let cleaned = clean_block_comment(content);
                if !cleaned.is_empty() {
                    return split_docstring(&cleaned);
                }
            }
            break;
        } else {
            // Non-comment node, stop
            break;
        }
    }

    if doc_lines.is_empty() {
        return FileMetadata::default();
    }

    let combined = doc_lines.join("\n");
    split_docstring(&combined)
}

// ---------------------------------------------------------------------------
// JavaScript/TypeScript metadata extraction
// ---------------------------------------------------------------------------

#[cfg(feature = "lang-javascript")]
fn extract_js_metadata(tree: &Tree, source: &[u8]) -> FileMetadata {
    let root = tree.root_node();
    let mut cursor = root.walk();

    // Look for top-level JSDoc comment (/** ... */)
    for child in root.children(&mut cursor) {
        if child.kind() == "comment" {
            let text = node_text(child, source);
            if text.starts_with("/**") && !text.starts_with("/***") {
                // JSDoc comment
                let content = text
                    .strip_prefix("/**")
                    .and_then(|s| s.strip_suffix("*/"))
                    .unwrap_or(&text);
                let cleaned = clean_jsdoc_comment(content);

                if !cleaned.is_empty() {
                    return split_docstring(&cleaned);
                }
            }
        } else if child.kind() != "hash_bang_line" {
            // Stop at first non-comment (except shebang)
            break;
        }
    }

    FileMetadata::default()
}

// ---------------------------------------------------------------------------
// Go metadata extraction
// ---------------------------------------------------------------------------

#[cfg(feature = "lang-go")]
fn extract_go_metadata(tree: &Tree, source: &[u8]) -> FileMetadata {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut doc_lines: Vec<String> = Vec::new();

    // Look for package comment (comment block before package clause)
    for child in root.children(&mut cursor) {
        if child.kind() == "comment" {
            let text = node_text(child, source);
            if text.starts_with("//") {
                let line = text.strip_prefix("//").unwrap_or(&text).trim();
                doc_lines.push(line.to_string());
            } else if text.starts_with("/*") {
                let content = text
                    .strip_prefix("/*")
                    .and_then(|s| s.strip_suffix("*/"))
                    .unwrap_or(&text);
                let cleaned = clean_block_comment(content);
                if !cleaned.is_empty() {
                    return split_docstring(&cleaned);
                }
            }
        } else if child.kind() == "package_clause" {
            // Found package clause, use collected comments
            break;
        } else {
            // Non-comment, non-package node
            break;
        }
    }

    if doc_lines.is_empty() {
        return FileMetadata::default();
    }

    let combined = doc_lines.join("\n");
    split_docstring(&combined)
}

// ---------------------------------------------------------------------------
// Java metadata extraction
// ---------------------------------------------------------------------------

#[cfg(feature = "lang-java")]
fn extract_java_metadata(tree: &Tree, source: &[u8]) -> FileMetadata {
    let root = tree.root_node();
    let mut cursor = root.walk();

    // Look for top-level Javadoc comment (/** ... */)
    for child in root.children(&mut cursor) {
        if child.kind() == "block_comment" {
            let text = node_text(child, source);
            if text.starts_with("/**") && !text.starts_with("/***") {
                let content = text
                    .strip_prefix("/**")
                    .and_then(|s| s.strip_suffix("*/"))
                    .unwrap_or(&text);
                let cleaned = clean_jsdoc_comment(content); // Same format as JSDoc

                if !cleaned.is_empty() {
                    return split_docstring(&cleaned);
                }
            }
        } else if child.kind() != "line_comment" {
            // Stop at first non-comment
            break;
        }
    }

    FileMetadata::default()
}

// ---------------------------------------------------------------------------
// C/C++ metadata extraction
// ---------------------------------------------------------------------------

#[cfg(feature = "lang-c")]
fn extract_c_metadata(tree: &Tree, source: &[u8]) -> FileMetadata {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut doc_lines: Vec<String> = Vec::new();

    // Look for file header comment at the start
    for child in root.children(&mut cursor) {
        if child.kind() == "comment" {
            let text = node_text(child, source);
            if text.starts_with("/*") {
                let content = text
                    .strip_prefix("/*")
                    .and_then(|s| s.strip_suffix("*/"))
                    .unwrap_or(&text);
                let cleaned = clean_block_comment(content);

                // Skip copyright-only headers
                if !cleaned.is_empty() && !is_copyright_only(&cleaned) {
                    return split_docstring(&cleaned);
                }
            } else if text.starts_with("//") {
                let line = text.strip_prefix("//").unwrap_or(&text).trim();
                // Skip copyright lines
                if !line.to_lowercase().contains("copyright") {
                    doc_lines.push(line.to_string());
                }
            }
        } else {
            // Stop at first non-comment
            break;
        }
    }

    if doc_lines.is_empty() {
        return FileMetadata::default();
    }

    let combined = doc_lines.join("\n");
    split_docstring(&combined)
}

// ---------------------------------------------------------------------------
// Ruby metadata extraction
// ---------------------------------------------------------------------------

#[cfg(feature = "lang-ruby")]
fn extract_ruby_metadata(tree: &Tree, source: &[u8]) -> FileMetadata {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut doc_lines: Vec<String> = Vec::new();

    // Look for top file comment block
    for child in root.children(&mut cursor) {
        if child.kind() == "comment" {
            let text = node_text(child, source);
            let line = text.strip_prefix('#').unwrap_or(&text).trim();
            // Skip shebang and encoding lines
            if !line.starts_with('!') && !line.to_lowercase().contains("encoding:") {
                doc_lines.push(line.to_string());
            }
        } else {
            // Stop at first non-comment
            break;
        }
    }

    if doc_lines.is_empty() {
        return FileMetadata::default();
    }

    let combined = doc_lines.join("\n");
    split_docstring(&combined)
}

// ---------------------------------------------------------------------------
// C# metadata extraction
// ---------------------------------------------------------------------------

#[cfg(feature = "lang-csharp")]
fn extract_csharp_metadata(tree: &Tree, source: &[u8]) -> FileMetadata {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut doc_lines: Vec<String> = Vec::new();

    // Look for top-level XML doc comments (///)
    for child in root.children(&mut cursor) {
        if child.kind() == "comment" {
            let text = node_text(child, source);
            if let Some(doc) = text.strip_prefix("///") {
                doc_lines.push(doc.trim().to_string());
            } else if text.starts_with("//") {
                // Regular comment, keep looking
                continue;
            }
        } else if child.kind() != "using_directive" {
            // Stop at first non-comment, non-using
            break;
        }
    }

    if doc_lines.is_empty() {
        return FileMetadata::default();
    }

    // Parse XML-like content for <summary>
    let combined = doc_lines.join("\n");
    if let Some(summary) = extract_xml_summary(&combined) {
        return split_docstring(&summary);
    }

    split_docstring(&combined)
}

#[cfg(feature = "lang-csharp")]
fn extract_xml_summary(text: &str) -> Option<String> {
    // Simple extraction of <summary>...</summary> content
    let start = text.find("<summary>")?;
    let end = text.find("</summary>")?;
    if start >= end {
        return None;
    }
    let content = text.get(start + 9..end)?;
    let cleaned = content
        .lines()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Split a docstring into title (first line) and description (rest).
fn split_docstring(text: &str) -> FileMetadata {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return FileMetadata::default();
    }

    // First non-empty line is title
    let mut lines = trimmed.lines();
    let title = lines.next().map(|s| s.trim().to_string());

    // Rest is description: skip decoration lines (e.g., RST underlines like ~~~, ===, ---)
    // and empty lines, take first real paragraph
    let desc_lines: Vec<&str> = lines
        .skip_while(|l| {
            let t = l.trim();
            t.is_empty() || is_decoration_line(t)
        })
        .take_while(|l| {
            let t = l.trim();
            !t.is_empty() && !is_decoration_line(t)
        })
        .collect();

    let description = if desc_lines.is_empty() {
        None
    } else {
        let desc = desc_lines.join(" ").trim().to_string();
        if desc.is_empty() { None } else { Some(desc) }
    };

    FileMetadata::new(title, description)
}

/// Check if a line is a decoration/underline (e.g., RST ~~~, ===, ---, etc.)
fn is_decoration_line(line: &str) -> bool {
    if line.len() < 3 {
        return false;
    }
    let first_char = line.chars().next().unwrap();
    // Common RST/markdown decoration characters
    matches!(first_char, '~' | '=' | '-' | '*' | '#' | '^' | '+')
        && line.chars().all(|c| c == first_char)
}

/// Clean a block comment by stripping leading * from each line.
fn clean_block_comment(text: &str) -> String {
    text.lines()
        .map(|line| {
            let trimmed = line.trim();
            // Strip leading * (common in block comments)
            trimmed.strip_prefix('*').unwrap_or(trimmed).trim()
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Clean a JSDoc/Javadoc comment by stripping * and @ tags.
fn clean_jsdoc_comment(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        // Strip leading *
        let content = trimmed.strip_prefix('*').unwrap_or(trimmed).trim();

        // Skip empty lines and @ tags (except @description/@fileoverview)
        if content.is_empty() {
            continue;
        }
        if content.starts_with('@') {
            // Extract content from @description or @fileoverview
            if let Some(rest) = content
                .strip_prefix("@description")
                .or_else(|| content.strip_prefix("@fileoverview"))
            {
                let desc = rest.trim();
                if !desc.is_empty() {
                    lines.push(desc.to_string());
                }
            }
            // Skip other @ tags
            continue;
        }

        lines.push(content.to_string());
    }

    lines.join("\n")
}

/// Check if a comment is only copyright/license boilerplate.
fn is_copyright_only(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("copyright")
        && !lower.contains("description")
        && !lower.contains("purpose")
        && !lower.contains("overview")
        && text.lines().count() <= 5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_to_sentence() {
        assert_eq!(
            truncate_to_sentence("Hello world. This is more."),
            "Hello world."
        );
        assert_eq!(truncate_to_sentence("No period here"), "No period here");
        assert_eq!(
            truncate_to_sentence("File v1.2.3 description. More info."),
            "File v1.2.3 description."
        );
    }

    #[test]
    fn test_truncate_to_line() {
        assert_eq!(truncate_to_line("First line\nSecond line"), "First line");
        assert_eq!(truncate_to_line("Single line"), "Single line");
    }

    #[test]
    fn test_split_docstring() {
        let meta = split_docstring("Title here\n\nDescription follows.");
        assert_eq!(meta.title, Some("Title here".to_string()));
        assert_eq!(meta.description, Some("Description follows.".to_string()));

        let meta = split_docstring("Just a title");
        assert_eq!(meta.title, Some("Just a title".to_string()));
        assert_eq!(meta.description, None);
    }

    #[test]
    fn test_clean_block_comment() {
        let input = "* First line\n * Second line\n * Third";
        let result = clean_block_comment(input);
        assert_eq!(result, "First line\nSecond line\nThird");
    }

    #[cfg(feature = "lang-markdown")]
    #[test]
    fn test_yaml_frontmatter() {
        let text = "---\ntitle: My Title\ndescription: My description\n---\n# Heading";
        let meta = extract_yaml_frontmatter(text).unwrap();
        assert_eq!(meta.title, Some("My Title".to_string()));
        assert_eq!(meta.description, Some("My description".to_string()));
    }

    #[cfg(feature = "lang-markdown")]
    #[test]
    fn test_markdown_heading_fallback() {
        let source = b"# Main Title\n\nFirst paragraph here.";
        let meta = extract_markdown_metadata(source);
        assert_eq!(meta.title, Some("Main Title".to_string()));
        assert_eq!(meta.description, Some("First paragraph here.".to_string()));
    }
}
