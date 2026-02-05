use anyhow::Result;
use tree_sitter::{Node, Parser, Tree};

use crate::index::format::{SymbolEntry, TextEntry};
use crate::parser::helpers::*;
use crate::parser::languages::get_language;
use crate::parser::sfc;

/// Parse a single file using tree-sitter and extract symbols and text blocks.
pub fn parse_file(
    source: &[u8],
    language: &str,
    file_path: &str,
) -> Result<(Vec<SymbolEntry>, Vec<TextEntry>)> {
    // SFC preprocessing: extract script blocks from Vue/Svelte/Astro files
    let sfc_ext = match language {
        "html" => Some("html"),
        "vue" => Some("vue"),
        "svelte" => Some("svelte"),
        "astro" => Some("astro"),
        _ => None,
    };

    if let Some(ext) = sfc_ext {
        return parse_sfc(source, ext, file_path);
    }

    let lang = get_language(language)?;
    let mut parser = Parser::new();
    parser.set_language(&lang)?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("failed to parse {file_path}"))?;

    let mut symbols = Vec::new();
    let mut texts = Vec::new();

    match language {
        #[cfg(feature = "lang-rust")]
        "rust" => {
            crate::parser::rust_lang::extract(&tree, source, file_path, &mut symbols, &mut texts)
        }

        #[cfg(feature = "lang-python")]
        "python" => {
            crate::parser::python::extract(&tree, source, file_path, &mut symbols, &mut texts)
        }

        #[cfg(feature = "lang-javascript")]
        "javascript" => {
            crate::parser::javascript::extract(&tree, source, file_path, &mut symbols, &mut texts)
        }

        #[cfg(feature = "lang-typescript")]
        "typescript" | "tsx" => {
            crate::parser::typescript::extract(&tree, source, file_path, &mut symbols, &mut texts)
        }

        #[cfg(feature = "lang-go")]
        "go" => crate::parser::go::extract(&tree, source, file_path, &mut symbols, &mut texts),

        #[cfg(feature = "lang-java")]
        "java" => crate::parser::java::extract(&tree, source, file_path, &mut symbols, &mut texts),

        #[cfg(feature = "lang-c")]
        "c" => crate::parser::c_lang::extract(&tree, source, file_path, &mut symbols, &mut texts),

        #[cfg(feature = "lang-cpp")]
        "cpp" => crate::parser::cpp::extract(&tree, source, file_path, &mut symbols, &mut texts),

        #[cfg(feature = "lang-ruby")]
        "ruby" => crate::parser::ruby::extract(&tree, source, file_path, &mut symbols, &mut texts),

        #[cfg(feature = "lang-csharp")]
        "csharp" => {
            crate::parser::csharp::extract(&tree, source, file_path, &mut symbols, &mut texts)
        }

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
// Generic text extraction (for unsupported languages)
// ---------------------------------------------------------------------------

fn extract_texts_generic(tree: &Tree, source: &[u8], file_path: &str, texts: &mut Vec<TextEntry>) {
    walk_texts_generic(tree.root_node(), source, file_path, texts);
}

fn walk_texts_generic(node: Node, source: &[u8], file_path: &str, texts: &mut Vec<TextEntry>) {
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
// SFC (Single File Component) handling
// ---------------------------------------------------------------------------

/// Parse an SFC file by extracting script blocks and running them through the
/// appropriate JS/TS parser, adjusting line numbers back to the original file.
fn parse_sfc(
    source: &[u8],
    extension: &str,
    file_path: &str,
) -> Result<(Vec<SymbolEntry>, Vec<TextEntry>)> {
    let blocks = sfc::extract_script_blocks(source, extension);

    if blocks.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut all_symbols = Vec::new();
    let mut all_texts = Vec::new();

    for block in &blocks {
        // Parse each script block with the detected language
        let (mut symbols, mut texts) = match parse_file(&block.content, block.lang, file_path) {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!(
                    "failed to parse {} script block in {}: {}",
                    block.lang,
                    file_path,
                    e
                );
                continue;
            }
        };

        // Adjust line numbers: add the block's start_line offset (minus 1 because
        // the parser already counts from line 1)
        let offset = block.start_line - 1;
        if offset > 0 {
            for sym in &mut symbols {
                sym.line[0] += offset;
                sym.line[1] += offset;
            }
            for txt in &mut texts {
                txt.line[0] += offset;
                txt.line[1] += offset;
            }
        }

        all_symbols.extend(symbols);
        all_texts.extend(texts);
    }

    Ok((all_symbols, all_texts))
}
