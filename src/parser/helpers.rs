//! Shared helpers for tree-sitter extraction across all languages.

use tree_sitter::Node;

use crate::index::format::{SymbolEntry, TextEntry};

/// Get the text content of a tree-sitter node.
pub fn node_text(node: Node, source: &[u8]) -> String {
    let start = node.start_byte();
    let end = node.end_byte();
    String::from_utf8_lossy(&source[start..end]).to_string()
}

/// Get the 1-based [start, end] line range for a node.
pub fn node_line_range(node: Node) -> [u32; 2] {
    let start = node.start_position().row as u32 + 1; // tree-sitter is 0-based
    let end_pos = node.end_position();
    // When end_position().column == 0, the node ends at the start of the next line
    // (e.g. line_comment includes trailing \n), so the actual end line is the previous row.
    let end = if end_pos.column == 0 && end_pos.row > node.start_position().row {
        end_pos.row as u32 // don't add 1, since it's actually the previous line
    } else {
        end_pos.row as u32 + 1
    };
    [start, end]
}

/// Find a child node by its field name.
pub fn find_child_by_field<'a>(node: Node<'a>, field: &str) -> Option<Node<'a>> {
    node.child_by_field_name(field)
}

/// Check if a text string is too trivial to index.
pub fn is_trivial_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() <= 1 {
        return true;
    }
    // Skip whitespace-only strings
    if trimmed.bytes().all(|b| b.is_ascii_whitespace()) {
        return true;
    }
    // Skip short strings that look like code identifiers rather than prose.
    // Prose typically contains spaces or is substantially longer.
    if !trimmed.contains(' ') && trimmed.len() <= 20 {
        return true;
    }
    false
}

/// Collapse multiple whitespace characters into single spaces.
pub fn collapse_whitespace(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_ws {
                result.push(' ');
            }
            prev_ws = true;
        } else {
            result.push(c);
            prev_ws = false;
        }
    }
    result
}

/// Strip `///` or `//!` prefix from each line of a doc comment.
pub fn strip_doc_comment_prefix(raw: &str) -> String {
    raw.lines()
        .map(|line| {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("///") {
                rest.strip_prefix(' ').unwrap_or(rest)
            } else if let Some(rest) = trimmed.strip_prefix("//!") {
                rest.strip_prefix(' ').unwrap_or(rest)
            } else {
                trimmed
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Strip `/* */` delimiters and leading `*` from block comments.
pub fn strip_block_comment(raw: &str) -> String {
    let s = raw
        .strip_prefix("/**")
        .or_else(|| raw.strip_prefix("/*!"))
        .or_else(|| raw.strip_prefix("/*"))
        .unwrap_or(raw);
    let s = s.strip_suffix("*/").unwrap_or(s);

    s.lines()
        .map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix("* ")
                .or_else(|| trimmed.strip_prefix('*'))
                .unwrap_or(trimmed)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Strip surrounding quotes from string literals.
pub fn strip_string_quotes(raw: &str) -> String {
    // Triple-quoted strings (Python, etc.)
    if raw.starts_with("\"\"\"") && raw.ends_with("\"\"\"") && raw.len() >= 6 {
        return raw[3..raw.len() - 3].to_string();
    }
    if raw.starts_with("'''") && raw.ends_with("'''") && raw.len() >= 6 {
        return raw[3..raw.len() - 3].to_string();
    }
    // Template literals (JS/TS)
    if raw.starts_with('`') && raw.ends_with('`') && raw.len() >= 2 {
        return raw[1..raw.len() - 1].to_string();
    }
    // Raw strings: r"..." or r#"..."#
    if raw.starts_with("r#") || raw.starts_with("r\"") {
        let s = raw.trim_start_matches('r');
        let s = s.trim_start_matches('#');
        let s = s.strip_prefix('"').unwrap_or(s);
        let s = s.strip_suffix('"').unwrap_or(s);
        let s = s.trim_end_matches('#');
        return s.to_string();
    }
    // Byte strings: b"..."
    if raw.starts_with("b\"") || raw.starts_with("b'") {
        let s = raw.trim_start_matches('b');
        return strip_simple_quotes(s);
    }
    // F-strings: f"..." (Python)
    if raw.starts_with("f\"") || raw.starts_with("f'") {
        let s = raw.trim_start_matches('f');
        return strip_simple_quotes(s);
    }
    strip_simple_quotes(raw)
}

fn strip_simple_quotes(s: &str) -> String {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        return s[1..s.len() - 1].to_string();
    }
    if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
        return s[1..s.len() - 1].to_string();
    }
    s.to_string()
}

/// Extract a comment node as a TextEntry (generic for C-style comments).
pub fn extract_comment(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    texts: &mut Vec<TextEntry>,
) {
    let raw = node_text(node, source);
    let line = node_line_range(node);

    let (kind, text) = if raw.starts_with("///") || raw.starts_with("//!") {
        let cleaned = strip_doc_comment_prefix(&raw);
        ("docstring", cleaned)
    } else if raw.starts_with("//") {
        let cleaned = raw.strip_prefix("//").unwrap_or(&raw).trim().to_string();
        ("comment", cleaned)
    } else if raw.starts_with("/*") {
        let cleaned = strip_block_comment(&raw);
        let kind = if raw.starts_with("/**") || raw.starts_with("/*!") {
            "docstring"
        } else {
            "comment"
        };
        (kind, cleaned)
    } else if raw.starts_with('#') {
        // Hash-style comments (Python, Ruby, etc.)
        let cleaned = raw.strip_prefix('#').unwrap_or(&raw).trim().to_string();
        ("comment", cleaned)
    } else {
        ("comment", raw)
    };

    if is_trivial_text(&text) {
        return;
    }

    texts.push(TextEntry {
        file: file_path.to_string(),
        kind: kind.to_string(),
        line,
        text,
        parent: parent_ctx.map(String::from),
        project: String::new(),
    });
}

/// Extract a string literal node as a TextEntry.
pub fn extract_string(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    texts: &mut Vec<TextEntry>,
) {
    let raw = node_text(node, source);
    let line = node_line_range(node);
    let text = strip_string_quotes(&raw);

    if is_trivial_text(&text) {
        return;
    }

    texts.push(TextEntry {
        file: file_path.to_string(),
        kind: "string".to_string(),
        line,
        text,
        parent: parent_ctx.map(String::from),
        project: String::new(),
    });
}

/// Push a symbol entry (convenience builder).
#[allow(clippy::too_many_arguments)]
pub fn push_symbol(
    symbols: &mut Vec<SymbolEntry>,
    file_path: &str,
    name: String,
    kind: &str,
    line: [u32; 2],
    parent: Option<&str>,
    sig: Option<String>,
    alias: Option<String>,
    visibility: Option<String>,
) {
    symbols.push(SymbolEntry {
        file: file_path.to_string(),
        name,
        kind: kind.to_string(),
        line,
        parent: parent.map(String::from),
        sig,
        alias,
        visibility,
        project: String::new(),
    });
}

/// Extract a function/method signature: everything from start to opening `{` or `:`.
/// Collapses whitespace.
pub fn extract_signature_to_brace(node: Node, source: &[u8]) -> String {
    let start = node.start_byte();
    let end = node.end_byte();
    let text = &source[start..end];
    let text = String::from_utf8_lossy(text);

    if let Some(brace_pos) = text.find('{') {
        let sig = text[..brace_pos].trim();
        collapse_whitespace(sig)
    } else if let Some(semi_pos) = text.find(';') {
        let sig = text[..semi_pos].trim();
        collapse_whitespace(sig)
    } else {
        collapse_whitespace(text.trim())
    }
}
