//! Markdown symbol and text extraction using tree-sitter-md.
//!
//! Headings are extracted as symbols with `kind="section"` and parent relationships
//! reflecting the document hierarchy. Fenced code blocks are extracted as text entries.
//!
//! tree-sitter-md uses a two-tree architecture:
//! - Block tree (`LANGUAGE`): document structure (headings, code blocks, paragraphs)
//! - Inline trees (`INLINE_LANGUAGE`): inline content within blocks (text, emphasis, links)

use anyhow::Result;
use tree_sitter::Node;
use tree_sitter_md::MarkdownParser;

use crate::index::format::{SymbolEntry, TextEntry};
use crate::parser::helpers::{node_line_range, node_text, push_symbol};

/// Parse and extract symbols/texts from markdown using tree-sitter-md.
pub fn parse_and_extract(
    source: &[u8],
    file_path: &str,
) -> Result<(Vec<SymbolEntry>, Vec<TextEntry>)> {
    let mut parser = MarkdownParser::default();
    let md_tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("failed to parse markdown: {}", file_path))?;

    let mut symbols = Vec::new();
    let mut texts = Vec::new();

    // Extract from block tree
    let block_tree = md_tree.block_tree();
    let root = block_tree.root_node();

    // Heading stack tracks (level, name) for building parent-child relationships
    let mut heading_stack: Vec<(u32, String)> = Vec::new();

    walk_blocks(
        root,
        source,
        file_path,
        &mut heading_stack,
        &mut symbols,
        &mut texts,
    );

    Ok((symbols, texts))
}

/// Walk the block tree to extract headings and code blocks.
fn walk_blocks(
    node: Node,
    source: &[u8],
    file_path: &str,
    heading_stack: &mut Vec<(u32, String)>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
) {
    match node.kind() {
        "atx_heading" => {
            extract_atx_heading(node, source, file_path, heading_stack, symbols);
        }
        "setext_heading" => {
            extract_setext_heading(node, source, file_path, heading_stack, symbols);
        }
        "fenced_code_block" => {
            extract_code_block(node, source, file_path, heading_stack, texts);
        }
        _ => {
            // Recurse to children
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk_blocks(child, source, file_path, heading_stack, symbols, texts);
            }
        }
    }
}

/// Extract ATX-style heading (# heading).
fn extract_atx_heading(
    node: Node,
    source: &[u8],
    file_path: &str,
    heading_stack: &mut Vec<(u32, String)>,
    symbols: &mut Vec<SymbolEntry>,
) {
    let level = count_atx_level(node, source);
    let text = get_heading_text(node, source);

    if text.is_empty() {
        return;
    }

    let line_range = node_line_range(node);
    let (qualified_name, parent) = compute_qualified_name(heading_stack, level, &text);

    push_symbol(
        symbols,
        file_path,
        qualified_name,
        "section",
        line_range,
        parent.as_deref(),
        None,
        None,
        None,
    );
}

/// Extract Setext-style heading (underlined with === or ---).
fn extract_setext_heading(
    node: Node,
    source: &[u8],
    file_path: &str,
    heading_stack: &mut Vec<(u32, String)>,
    symbols: &mut Vec<SymbolEntry>,
) {
    let level = get_setext_level(node, source);
    let text = get_heading_text(node, source);

    if text.is_empty() {
        return;
    }

    let line_range = node_line_range(node);
    let (qualified_name, parent) = compute_qualified_name(heading_stack, level, &text);

    push_symbol(
        symbols,
        file_path,
        qualified_name,
        "section",
        line_range,
        parent.as_deref(),
        None,
        None,
        None,
    );
}

/// Extract fenced code block as text entry.
fn extract_code_block(
    node: Node,
    source: &[u8],
    file_path: &str,
    heading_stack: &[(u32, String)],
    texts: &mut Vec<TextEntry>,
) {
    // Try to find code_fence_content child first
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "code_fence_content" {
            let text = node_text(child, source);
            // For code blocks, we don't filter as aggressively as other text types
            if text.trim().is_empty() {
                return;
            }

            let line_range = node_line_range(node);
            let parent = heading_stack.last().map(|(_, name)| name.clone());

            texts.push(TextEntry {
                file: file_path.to_string(),
                kind: "sample".to_string(),
                line: line_range,
                text,
                parent,
                project: String::new(),
            });
            return;
        }
    }

    // Fallback: extract content between fence markers from raw text
    let raw = node_text(node, source);
    if let Some(content) = extract_code_content(&raw) {
        if content.trim().is_empty() {
            return;
        }

        let line_range = node_line_range(node);
        let parent = heading_stack.last().map(|(_, name)| name.clone());

        texts.push(TextEntry {
            file: file_path.to_string(),
            kind: "sample".to_string(),
            line: line_range,
            text: content,
            parent,
            project: String::new(),
        });
    }
}

/// Extract code content from fenced block raw text.
/// Strips the opening and closing fence lines.
fn extract_code_content(raw: &str) -> Option<String> {
    let lines: Vec<&str> = raw.lines().collect();
    if lines.len() < 2 {
        return None;
    }

    // Skip first line (opening fence) and last line (closing fence)
    let first = lines.first()?;
    if !first.trim_start().starts_with("```") && !first.trim_start().starts_with("~~~") {
        return None;
    }

    // Find closing fence
    let mut end_idx = lines.len();
    for (i, line) in lines.iter().enumerate().skip(1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            end_idx = i;
            break;
        }
    }

    let content = lines.get(1..end_idx).map(|slice| slice.join("\n"))?;
    Some(content)
}

/// Get heading text by extracting from child nodes.
///
/// For ATX headings, we look for `heading_content` or `inline` children.
/// For Setext headings, we look for `paragraph` children.
fn get_heading_text(node: Node, source: &[u8]) -> String {
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            // ATX heading content - may include trailing # which we need to strip
            "heading_content" | "inline" => {
                let text = node_text(child, source);
                return strip_optional_closing_hashes(text.trim());
            }
            // Setext heading: first paragraph is the heading text
            "paragraph" => {
                let text = node_text(child, source);
                return text.trim().to_string();
            }
            _ => {}
        }
    }

    // Fallback: extract from raw text and strip markdown syntax
    let raw = node_text(node, source);
    strip_heading_markers(&raw)
}

/// Strip optional closing sequence of # from ATX heading.
/// Per CommonMark spec, the closing # sequence must be preceded by a space.
fn strip_optional_closing_hashes(text: &str) -> String {
    // Find the last non-# character
    let trimmed = text.trim_end();

    // Check if it ends with space followed by #
    // The closing sequence is: optional whitespace, one or more #, optional whitespace
    // Use char_indices to safely handle multi-byte UTF-8 characters (e.g., non-breaking space)
    if let Some((last_space_idx, last_space_char)) =
        trimmed.char_indices().rfind(|(_, c)| c.is_whitespace())
    {
        // Calculate the byte offset after the whitespace character
        let after_space_start = last_space_idx + last_space_char.len_utf8();
        let after_space = trimmed.get(after_space_start..).unwrap_or("");
        // Check if everything after the last space is just #
        if !after_space.is_empty() && after_space.chars().all(|c| c == '#') {
            // This is an optional closing sequence - strip it
            return trimmed
                .get(..last_space_idx)
                .unwrap_or(trimmed)
                .trim()
                .to_string();
        }
    }

    trimmed.to_string()
}

/// Strip markdown heading markers (# prefix and underlines).
fn strip_heading_markers(raw: &str) -> String {
    let trimmed = raw.trim();

    // ATX: strip leading # and optional trailing #
    if trimmed.starts_with('#') {
        // Get just the first line (ATX headings are single line)
        let first_line = trimmed.lines().next().unwrap_or(trimmed);
        // Strip leading #
        let after_hashes = first_line.trim_start_matches('#');
        // Strip trailing # (markdown allows optional closing hashes)
        let text = after_hashes.trim_end_matches('#').trim();
        return text.to_string();
    }

    // Setext: strip underline (text is on first line)
    if let Some(first_line) = trimmed.lines().next() {
        return first_line.trim().to_string();
    }

    trimmed.to_string()
}

/// Count ATX heading level (number of leading #).
fn count_atx_level(node: Node, source: &[u8]) -> u32 {
    // Look for atx_h1_marker, atx_h2_marker, etc.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if kind.starts_with("atx_h") && kind.ends_with("_marker") {
            // Extract level from marker name: atx_h1_marker -> 1
            if let Some(level_char) = kind.chars().nth(5)
                && let Some(level) = level_char.to_digit(10)
            {
                return level;
            }
        }
    }

    // Fallback: count leading # characters
    let text = node_text(node, source);
    let count = text.chars().take_while(|&c| c == '#').count();
    count.clamp(1, 6) as u32
}

/// Get Setext heading level (1 for ===, 2 for ---).
fn get_setext_level(node: Node, source: &[u8]) -> u32 {
    // Look for setext_h1_underline or setext_h2_underline
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "setext_h1_underline" => return 1,
            "setext_h2_underline" => return 2,
            _ => {}
        }
    }

    // Fallback: check for underline character in last line
    let text = node_text(node, source);
    if let Some(last_line) = text.lines().last() {
        let trimmed = last_line.trim();
        if trimmed.starts_with('=') {
            return 1;
        } else if trimmed.starts_with('-') {
            return 2;
        }
    }
    1 // Default to h1
}

/// Compute qualified name and parent for a heading based on the heading stack.
///
/// The stack maintains (level, qualified_name) pairs. When we encounter a heading at level N:
/// 1. Pop all headings at level >= N (they're no longer ancestors)
/// 2. The parent is the top of the stack (if any)
/// 3. Build qualified name as "parent/name" (or just "name" if no parent)
/// 4. Push (N, qualified_name) onto the stack
///
/// Returns (qualified_name, parent_qualified_name).
fn compute_qualified_name(
    heading_stack: &mut Vec<(u32, String)>,
    level: u32,
    name: &str,
) -> (String, Option<String>) {
    // Pop all headings at same or deeper level
    while let Some(&(top_level, _)) = heading_stack.last() {
        if top_level >= level {
            heading_stack.pop();
        } else {
            break;
        }
    }

    // Parent is the top of the stack (the nearest ancestor heading)
    let parent = heading_stack.last().map(|(_, qname)| qname.clone());

    // Build qualified name
    let qualified_name = match &parent {
        Some(parent_name) => format!("{}/{}", parent_name, name),
        None => name.to_string(),
    };

    // Push current heading onto the stack
    heading_stack.push((level, qualified_name.clone()));

    (qualified_name, parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atx_headings_basic() {
        let source = b"# Top Level\n\nSome content.\n\n## Sub Section\n\nMore content.\n";
        let (symbols, _) = parse_and_extract(source, "test.md").unwrap();

        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "Top Level");
        assert_eq!(symbols[0].kind, "section");
        assert_eq!(symbols[0].parent, None);

        // Qualified name includes parent path
        assert_eq!(symbols[1].name, "Top Level/Sub Section");
        assert_eq!(symbols[1].kind, "section");
        assert_eq!(symbols[1].parent, Some("Top Level".to_string()));
    }

    #[test]
    fn test_atx_headings_hierarchy() {
        let source =
            b"# Chapter 1\n## Section A\n### Detail 1\n### Detail 2\n## Section B\n# Chapter 2\n";
        let (symbols, _) = parse_and_extract(source, "test.md").unwrap();

        assert_eq!(symbols.len(), 6);

        // Chapter 1 has no parent
        assert_eq!(symbols[0].name, "Chapter 1");
        assert_eq!(symbols[0].parent, None);

        // Section A's parent is Chapter 1
        assert_eq!(symbols[1].name, "Chapter 1/Section A");
        assert_eq!(symbols[1].parent, Some("Chapter 1".to_string()));

        // Detail 1's parent is Section A (qualified)
        assert_eq!(symbols[2].name, "Chapter 1/Section A/Detail 1");
        assert_eq!(symbols[2].parent, Some("Chapter 1/Section A".to_string()));

        // Detail 2's parent is Section A (same level as Detail 1)
        assert_eq!(symbols[3].name, "Chapter 1/Section A/Detail 2");
        assert_eq!(symbols[3].parent, Some("Chapter 1/Section A".to_string()));

        // Section B's parent is Chapter 1 (goes back up)
        assert_eq!(symbols[4].name, "Chapter 1/Section B");
        assert_eq!(symbols[4].parent, Some("Chapter 1".to_string()));

        // Chapter 2 has no parent (new top-level)
        assert_eq!(symbols[5].name, "Chapter 2");
        assert_eq!(symbols[5].parent, None);
    }

    #[test]
    fn test_setext_headings() {
        let source = b"Heading One\n===========\n\nSome text.\n\nHeading Two\n-----------\n";
        let (symbols, _) = parse_and_extract(source, "test.md").unwrap();

        assert!(symbols.len() >= 2);
        assert_eq!(symbols[0].name, "Heading One");
        assert_eq!(symbols[0].kind, "section");

        // Setext with --- is level 2, so parent is level 1
        assert_eq!(symbols[1].name, "Heading One/Heading Two");
        assert_eq!(symbols[1].parent, Some("Heading One".to_string()));
    }

    #[test]
    fn test_code_blocks() {
        let source = b"# Setup\n\n```rust\nfn main() {\n    println!(\"Hello\");\n}\n```\n";
        let (symbols, texts) = parse_and_extract(source, "test.md").unwrap();

        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Setup");

        assert!(!texts.is_empty());
        assert_eq!(texts[0].kind, "sample");
        assert!(texts[0].text.contains("fn main"));
        assert_eq!(texts[0].parent, Some("Setup".to_string()));
    }

    #[test]
    fn test_mixed_heading_styles() {
        let source = b"Main Title\n==========\n\n## ATX Subsection\n\nContent here.\n";
        let (symbols, _) = parse_and_extract(source, "test.md").unwrap();

        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "Main Title");
        assert_eq!(symbols[1].name, "Main Title/ATX Subsection");
        assert_eq!(symbols[1].parent, Some("Main Title".to_string()));
    }

    #[test]
    fn test_heading_with_trailing_hashes() {
        // Per CommonMark: optional closing sequence is space + one or more # at end of line
        // The "###" in the middle is content (followed by space), only the final " #" is stripped
        let source = b"# Heading with trailing ### #\n";
        let (symbols, _) = parse_and_extract(source, "test.md").unwrap();

        assert_eq!(symbols.len(), 1);
        // The middle "###" is content, final " #" is stripped
        assert_eq!(symbols[0].name, "Heading with trailing ###");
    }

    #[test]
    fn test_heading_with_only_closing_hashes() {
        // When the entire trailing part is the closing sequence
        let source = b"# Simple heading ##\n";
        let (symbols, _) = parse_and_extract(source, "test.md").unwrap();

        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Simple heading");
    }

    #[test]
    fn test_heading_with_non_breaking_space() {
        // Non-breaking space (\u{a0}) is a 2-byte UTF-8 character (0xC2 0xA0)
        // This tests that we handle multi-byte whitespace characters correctly
        // when stripping optional closing hashes
        let source = "# Prototype in\u{a0}a nutshell\n".as_bytes();
        let (symbols, _) = parse_and_extract(source, "test.md").unwrap();

        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Prototype in\u{a0}a nutshell");
    }

    #[test]
    fn test_heading_with_non_breaking_space_and_closing_hashes() {
        // Non-breaking space followed by closing hashes - should strip the hashes
        let source = "# Title with\u{a0}nbsp ##\n".as_bytes();
        let (symbols, _) = parse_and_extract(source, "test.md").unwrap();

        assert_eq!(symbols.len(), 1);
        // The trailing " ##" should be stripped, but the nbsp is kept
        assert_eq!(symbols[0].name, "Title with\u{a0}nbsp");
    }

    #[test]
    fn test_empty_heading_skipped() {
        let source = b"# \n## Real Heading\n";
        let (symbols, _) = parse_and_extract(source, "test.md").unwrap();

        // Empty heading should be skipped
        assert!(symbols.iter().all(|s| !s.name.is_empty()));
        assert!(symbols.iter().any(|s| s.name == "Real Heading"));
    }

    #[test]
    fn test_code_block_without_heading() {
        let source = b"```python\nprint('hello')\n```\n";
        let (symbols, texts) = parse_and_extract(source, "test.md").unwrap();

        assert!(symbols.is_empty());
        assert!(!texts.is_empty());
        assert_eq!(texts[0].kind, "sample");
        assert_eq!(texts[0].parent, None); // No heading context
    }

    #[test]
    fn test_deeply_nested_headings() {
        let source = b"# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6\n";
        let (symbols, _) = parse_and_extract(source, "test.md").unwrap();

        assert_eq!(symbols.len(), 6);
        assert_eq!(symbols[5].name, "H1/H2/H3/H4/H5/H6");
        assert_eq!(symbols[5].parent, Some("H1/H2/H3/H4/H5".to_string()));
    }

    #[test]
    fn test_duplicate_heading_names() {
        // Multiple "Features" sections under different parents - qualified names disambiguate
        let source = b"# v1.0\n## Features\n## Bug Fixes\n# v2.0\n## Features\n## Bug Fixes\n";
        let (symbols, _) = parse_and_extract(source, "test.md").unwrap();

        assert_eq!(symbols.len(), 6);
        assert_eq!(symbols[0].name, "v1.0");
        assert_eq!(symbols[1].name, "v1.0/Features");
        assert_eq!(symbols[2].name, "v1.0/Bug Fixes");
        assert_eq!(symbols[3].name, "v2.0");
        assert_eq!(symbols[4].name, "v2.0/Features");
        assert_eq!(symbols[5].name, "v2.0/Bug Fixes");
    }

    #[test]
    fn test_code_block_with_qualified_parent() {
        // Code block under a nested heading should have qualified parent
        let source = b"# Guide\n## Installation\n```bash\nnpm install\n```\n";
        let (symbols, texts) = parse_and_extract(source, "test.md").unwrap();

        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "Guide");
        assert_eq!(symbols[1].name, "Guide/Installation");

        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0].kind, "sample");
        // Parent should be the qualified name
        assert_eq!(texts[0].parent, Some("Guide/Installation".to_string()));
    }
}
