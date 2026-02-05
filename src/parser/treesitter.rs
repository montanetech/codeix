use anyhow::Result;
use tree_sitter::{Node, Parser, Tree};

use crate::index::format::{SymbolEntry, TextEntry};
use crate::parser::languages::get_language;

/// Parse a single file using tree-sitter and extract symbols and text blocks.
pub fn parse_file(
    source: &[u8],
    language: &str,
    file_path: &str,
) -> Result<(Vec<SymbolEntry>, Vec<TextEntry>)> {
    let lang = get_language(language)?;
    let mut parser = Parser::new();
    parser.set_language(&lang)?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("failed to parse {file_path}"))?;

    let mut symbols = Vec::new();
    let mut texts = Vec::new();

    match language {
        "rust" => extract_rust(&tree, source, file_path, &mut symbols, &mut texts),
        _ => {
            // For unsupported languages, just extract comments and strings
            extract_texts_generic(&tree, source, file_path, &mut texts);
        }
    }

    // Merge consecutive doc comments (/// lines) into single entries
    texts = merge_consecutive_texts(texts);

    Ok((symbols, texts))
}

// ---------------------------------------------------------------------------
// Rust extraction
// ---------------------------------------------------------------------------

fn extract_rust(
    tree: &Tree,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
) {
    let root = tree.root_node();
    walk_rust_node(root, source, file_path, None, symbols, texts);
}

fn walk_rust_node(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
) {
    let kind = node.kind();

    match kind {
        "function_item" => {
            extract_rust_function(node, source, file_path, parent_ctx, symbols);
        }
        "struct_item" => {
            extract_rust_named_symbol(node, source, file_path, "struct", parent_ctx, symbols);
        }
        "enum_item" => {
            extract_rust_named_symbol(node, source, file_path, "enum", parent_ctx, symbols);
        }
        "trait_item" => {
            extract_rust_named_symbol(node, source, file_path, "interface", parent_ctx, symbols);
        }
        "type_item" => {
            extract_rust_named_symbol(node, source, file_path, "type_alias", parent_ctx, symbols);
        }
        "mod_item" => {
            extract_rust_named_symbol(node, source, file_path, "module", parent_ctx, symbols);
        }
        "const_item" => {
            extract_rust_named_symbol(node, source, file_path, "constant", parent_ctx, symbols);
        }
        "static_item" => {
            extract_rust_named_symbol(node, source, file_path, "constant", parent_ctx, symbols);
        }
        "use_declaration" => {
            extract_rust_use(node, source, file_path, symbols);
        }
        "impl_item" => {
            extract_rust_impl(node, source, file_path, symbols, texts);
            return; // impl is handled recursively inside extract_rust_impl
        }
        "line_comment" | "block_comment" => {
            extract_rust_comment(node, source, file_path, parent_ctx, texts);
            return; // no children to recurse into
        }
        "string_literal" | "raw_string_literal" => {
            extract_rust_string(node, source, file_path, parent_ctx, texts);
            return;
        }
        _ => {}
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_rust_node(child, source, file_path, parent_ctx, symbols, texts);
    }
}

/// Extract a function/method item.
fn extract_rust_function(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
) {
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    let visibility = extract_rust_visibility(node, source);
    let sig = extract_rust_fn_signature(node, source);
    let line = node_line_range(node);

    let kind = if parent_ctx.is_some() {
        "method"
    } else {
        "function"
    };

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name
    };

    symbols.push(SymbolEntry {
        file: file_path.to_string(),
        name: full_name,
        kind: kind.to_string(),
        line,
        parent: parent_ctx.map(String::from),
        sig: Some(sig),
        alias: None,
        visibility: Some(visibility),
    });
}

/// Extract a named symbol (struct, enum, trait, type, mod, const, static).
fn extract_rust_named_symbol(
    node: Node,
    source: &[u8],
    file_path: &str,
    kind: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
) {
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    let visibility = extract_rust_visibility(node, source);
    let line = node_line_range(node);

    symbols.push(SymbolEntry {
        file: file_path.to_string(),
        name,
        kind: kind.to_string(),
        line,
        parent: parent_ctx.map(String::from),
        sig: None,
        alias: None,
        visibility: Some(visibility),
    });
}

/// Extract an impl block: emit the impl itself plus methods inside it.
fn extract_rust_impl(
    node: Node,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
) {
    // Determine the type being implemented.
    // For `impl Foo`, the type child is directly available.
    // For `impl Trait for Foo`, we need to find the type after `for`.
    let impl_type_name = extract_impl_type_name(node, source);

    let line = node_line_range(node);
    let visibility = extract_rust_visibility(node, source);

    // Check if this is a trait impl (has `trait` child)
    let trait_name = find_child_by_field(node, "trait").map(|n| node_text(n, source));

    let kind = if trait_name.is_some() {
        "trait_impl"
    } else {
        "impl"
    };

    // Build a signature like "impl Trait for Type" or "impl Type"
    let sig = if let Some(ref trait_n) = trait_name {
        Some(format!("impl {trait_n} for {impl_type_name}"))
    } else {
        Some(format!("impl {impl_type_name}"))
    };

    symbols.push(SymbolEntry {
        file: file_path.to_string(),
        name: impl_type_name.clone(),
        kind: kind.to_string(),
        line,
        parent: None,
        sig,
        alias: None,
        visibility: Some(visibility),
    });

    // Now walk children of the body to find methods
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            match child.kind() {
                "function_item" => {
                    extract_rust_function(
                        child,
                        source,
                        file_path,
                        Some(&impl_type_name),
                        symbols,
                    );
                }
                "const_item" => {
                    extract_rust_named_symbol(
                        child,
                        source,
                        file_path,
                        "constant",
                        Some(&impl_type_name),
                        symbols,
                    );
                }
                "type_item" => {
                    extract_rust_named_symbol(
                        child,
                        source,
                        file_path,
                        "type_alias",
                        Some(&impl_type_name),
                        symbols,
                    );
                }
                "line_comment" | "block_comment" => {
                    extract_rust_comment(
                        child,
                        source,
                        file_path,
                        Some(&impl_type_name),
                        texts,
                    );
                }
                "string_literal" | "raw_string_literal" => {
                    extract_rust_string(
                        child,
                        source,
                        file_path,
                        Some(&impl_type_name),
                        texts,
                    );
                }
                _ => {
                    // Recurse for any other nodes that might contain comments/strings
                    walk_rust_node(
                        child,
                        source,
                        file_path,
                        Some(&impl_type_name),
                        symbols,
                        texts,
                    );
                }
            }
        }
    }
}

/// Extract the type name from an impl block.
fn extract_impl_type_name(node: Node, source: &[u8]) -> String {
    // tree-sitter-rust: impl item has field "type" for the target type
    if let Some(type_node) = find_child_by_field(node, "type") {
        return node_text(type_node, source);
    }
    // Fallback: just use a generic name
    "Unknown".to_string()
}

/// Extract a use declaration as an import symbol.
fn extract_rust_use(
    node: Node,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
) {
    let line = node_line_range(node);
    let visibility = extract_rust_visibility(node, source);

    // Get the full use path text
    // The argument field contains the use tree
    if let Some(arg) = find_child_by_field(node, "argument") {
        extract_use_paths(arg, source, file_path, &line, &visibility, symbols);
    }
}

/// Recursively extract paths from a use tree.
fn extract_use_paths(
    node: Node,
    source: &[u8],
    file_path: &str,
    line: &[u32; 2],
    visibility: &str,
    symbols: &mut Vec<SymbolEntry>,
) {
    match node.kind() {
        "use_as_clause" => {
            // `foo::bar as baz`
            if let Some(path_node) = find_child_by_field(node, "path") {
                let name = node_text(path_node, source);
                let alias = find_child_by_field(node, "alias").map(|n| node_text(n, source));
                symbols.push(SymbolEntry {
                    file: file_path.to_string(),
                    name,
                    kind: "import".to_string(),
                    line: *line,
                    parent: None,
                    sig: None,
                    alias,
                    visibility: Some(visibility.to_string()),
                });
            }
        }
        "use_list" => {
            // `{Foo, Bar}` — recurse into children
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_use_paths(child, source, file_path, line, visibility, symbols);
            }
        }
        "use_wildcard" | "scoped_use_list" => {
            // `foo::*` or `foo::{bar, baz}` — emit the whole text
            let name = node_text(node, source);
            symbols.push(SymbolEntry {
                file: file_path.to_string(),
                name,
                kind: "import".to_string(),
                line: *line,
                parent: None,
                sig: None,
                alias: None,
                visibility: Some(visibility.to_string()),
            });
        }
        "scoped_identifier" | "identifier" => {
            // Simple path like `std::path::Path`
            let name = node_text(node, source);
            symbols.push(SymbolEntry {
                file: file_path.to_string(),
                name,
                kind: "import".to_string(),
                line: *line,
                parent: None,
                sig: None,
                alias: None,
                visibility: Some(visibility.to_string()),
            });
        }
        _ => {
            // For other node types, try recursing
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_use_paths(child, source, file_path, line, visibility, symbols);
            }
        }
    }
}

/// Extract visibility from a node's children.
fn extract_rust_visibility(node: Node, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            let text = node_text(child, source);
            if text.contains("pub(crate)") || text.contains("pub(super)") || text.contains("pub(in")
            {
                return "internal".to_string();
            }
            // plain `pub`
            return "public".to_string();
        }
    }
    "private".to_string()
}

/// Extract a function signature: everything from `fn` to the opening `{` (exclusive).
fn extract_rust_fn_signature(node: Node, source: &[u8]) -> String {
    let start = node.start_byte();
    let end = node.end_byte();
    let text = &source[start..end];
    let text = String::from_utf8_lossy(text);

    // Find the opening brace and take everything before it
    if let Some(brace_pos) = text.find('{') {
        let sig = text[..brace_pos].trim();
        // Collapse multiple whitespace into single space
        collapse_whitespace(sig)
    } else if let Some(semi_pos) = text.find(';') {
        // For declarations without a body (e.g., trait methods)
        let sig = text[..semi_pos].trim();
        collapse_whitespace(sig)
    } else {
        collapse_whitespace(text.trim())
    }
}

/// Extract a comment node as a TextEntry.
fn extract_rust_comment(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    texts: &mut Vec<TextEntry>,
) {
    let raw = node_text(node, source);
    let line = node_line_range(node);

    let (kind, text) = if raw.starts_with("///") || raw.starts_with("//!") {
        // Doc comment
        let cleaned = strip_doc_comment_prefix(&raw);
        ("docstring", cleaned)
    } else if raw.starts_with("//") {
        let cleaned = raw
            .strip_prefix("//")
            .unwrap_or(&raw)
            .trim()
            .to_string();
        ("comment", cleaned)
    } else if raw.starts_with("/*") {
        // Block comment
        let cleaned = strip_block_comment(&raw);
        let kind = if raw.starts_with("/**") || raw.starts_with("/*!") {
            "docstring"
        } else {
            "comment"
        };
        (kind, cleaned)
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
    });
}

/// Extract a string literal as a TextEntry.
fn extract_rust_string(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    texts: &mut Vec<TextEntry>,
) {
    let raw = node_text(node, source);
    let line = node_line_range(node);

    // Strip surrounding quotes
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
    });
}

// ---------------------------------------------------------------------------
// Generic text extraction (for unsupported languages)
// ---------------------------------------------------------------------------

fn extract_texts_generic(
    tree: &Tree,
    source: &[u8],
    file_path: &str,
    texts: &mut Vec<TextEntry>,
) {
    walk_texts_generic(tree.root_node(), source, file_path, texts);
}

fn walk_texts_generic(
    node: Node,
    source: &[u8],
    file_path: &str,
    texts: &mut Vec<TextEntry>,
) {
    let kind = node.kind();

    if kind.contains("comment") {
        let raw = node_text(node, source);
        let text = raw.trim().to_string();
        if !is_trivial_text(&text) {
            texts.push(TextEntry {
                file: file_path.to_string(),
                kind: "comment".to_string(),
                line: node_line_range(node),
                text,
                parent: None,
            });
        }
        return;
    }

    if kind.contains("string") {
        let raw = node_text(node, source);
        let text = strip_string_quotes(&raw);
        if !is_trivial_text(&text) {
            texts.push(TextEntry {
                file: file_path.to_string(),
                kind: "string".to_string(),
                line: node_line_range(node),
                text,
                parent: None,
            });
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_texts_generic(child, source, file_path, texts);
    }
}

// ---------------------------------------------------------------------------
// Post-processing: merge consecutive doc comments
// ---------------------------------------------------------------------------

/// Merge consecutive text entries of the same kind on adjacent lines.
///
/// Tree-sitter emits each `/// ...` line as a separate `line_comment` node.
/// This pass merges them into a single `TextEntry` with joined text and
/// a line range spanning the full block.
fn merge_consecutive_texts(texts: Vec<TextEntry>) -> Vec<TextEntry> {
    let mut merged: Vec<TextEntry> = Vec::with_capacity(texts.len());

    for entry in texts {
        let should_merge = if let Some(last) = merged.last() {
            last.file == entry.file
                && last.kind == entry.kind
                && last.parent == entry.parent
                && (last.kind == "docstring" || last.kind == "comment")
                && entry.line[0] == last.line[1] + 1
        } else {
            false
        };

        if should_merge {
            let last = merged.last_mut().unwrap();
            last.line[1] = entry.line[1];
            last.text.push('\n');
            last.text.push_str(&entry.text);
        } else {
            merged.push(entry);
        }
    }

    merged
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_child_by_field<'a>(node: Node<'a>, field: &str) -> Option<Node<'a>> {
    node.child_by_field_name(field)
}

fn node_text(node: Node, source: &[u8]) -> String {
    let start = node.start_byte();
    let end = node.end_byte();
    String::from_utf8_lossy(&source[start..end]).to_string()
}

fn node_line_range(node: Node) -> [u32; 2] {
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

fn is_trivial_text(text: &str) -> bool {
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

fn collapse_whitespace(s: &str) -> String {
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
fn strip_doc_comment_prefix(raw: &str) -> String {
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
fn strip_block_comment(raw: &str) -> String {
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
fn strip_string_quotes(raw: &str) -> String {
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
