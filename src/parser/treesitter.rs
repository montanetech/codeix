use anyhow::Result;
use tree_sitter::{Parser, Tree};

use crate::index::format::{ReferenceEntry, SymbolEntry, TextEntry};
use crate::parser::helpers::*;
use crate::parser::languages::get_language;
use crate::parser::sfc;

/// Maximum recursion depth for AST traversal to prevent stack overflow on deeply nested code.
pub const MAX_DEPTH: usize = 150;

/// Parse a single file using tree-sitter and extract symbols, text blocks, and references.
pub fn parse_file(
    source: &[u8],
    language: &str,
    file_path: &str,
) -> Result<(Vec<SymbolEntry>, Vec<TextEntry>, Vec<ReferenceEntry>)> {
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

    // Markdown uses a custom two-pass parser (tree-sitter-md with MarkdownParser)
    #[cfg(feature = "lang-markdown")]
    if language == "markdown" {
        let (symbols, texts) = crate::parser::markdown::parse_and_extract(source, file_path)?;
        return Ok((symbols, texts, Vec::new()));
    }

    let lang = get_language(language)?;
    let mut parser = Parser::new();
    parser.set_language(&lang)?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("failed to parse {file_path}"))?;

    let mut symbols = Vec::new();
    let mut texts = Vec::new();
    let mut references = Vec::new();

    match language {
        #[cfg(feature = "lang-rust")]
        "rust" => crate::parser::rust_lang::extract(
            &tree,
            source,
            file_path,
            &mut symbols,
            &mut texts,
            &mut references,
        ),

        #[cfg(feature = "lang-python")]
        "python" => crate::parser::python::extract(
            &tree,
            source,
            file_path,
            &mut symbols,
            &mut texts,
            &mut references,
        ),

        #[cfg(feature = "lang-javascript")]
        "javascript" => crate::parser::javascript::extract(
            &tree,
            source,
            file_path,
            &mut symbols,
            &mut texts,
            &mut references,
        ),

        #[cfg(feature = "lang-typescript")]
        "typescript" | "tsx" => crate::parser::typescript::extract(
            &tree,
            source,
            file_path,
            &mut symbols,
            &mut texts,
            &mut references,
        ),

        #[cfg(feature = "lang-go")]
        "go" => crate::parser::go::extract(
            &tree,
            source,
            file_path,
            &mut symbols,
            &mut texts,
            &mut references,
        ),

        #[cfg(feature = "lang-java")]
        "java" => crate::parser::java::extract(
            &tree,
            source,
            file_path,
            &mut symbols,
            &mut texts,
            &mut references,
        ),

        #[cfg(feature = "lang-c")]
        "c" => crate::parser::c_lang::extract(
            &tree,
            source,
            file_path,
            &mut symbols,
            &mut texts,
            &mut references,
        ),

        #[cfg(feature = "lang-cpp")]
        "cpp" => crate::parser::cpp::extract(
            &tree,
            source,
            file_path,
            &mut symbols,
            &mut texts,
            &mut references,
        ),

        #[cfg(feature = "lang-ruby")]
        "ruby" => crate::parser::ruby::extract(
            &tree,
            source,
            file_path,
            &mut symbols,
            &mut texts,
            &mut references,
        ),

        #[cfg(feature = "lang-csharp")]
        "csharp" => crate::parser::csharp::extract(
            &tree,
            source,
            file_path,
            &mut symbols,
            &mut texts,
            &mut references,
        ),

        _ => {
            // For unsupported languages, just extract comments and strings
            extract_texts_generic(&tree, source, file_path, &mut texts);
        }
    }

    // Merge consecutive doc comments (/// lines) into single entries
    texts = merge_consecutive_texts(texts);

    Ok((symbols, texts, references))
}

// ---------------------------------------------------------------------------
// Generic text extraction (for unsupported languages)
// ---------------------------------------------------------------------------

fn extract_texts_generic(tree: &Tree, source: &[u8], file_path: &str, texts: &mut Vec<TextEntry>) {
    // Use iterative traversal with explicit stack to avoid stack overflow
    let mut stack = vec![tree.root_node()];

    while let Some(node) = stack.pop() {
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
                    project: String::new(),
                });
            }
            continue;
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
                    project: String::new(),
                });
            }
            continue;
        }

        // Push children in reverse order so they're processed in forward order
        let mut cursor = node.walk();
        let children: Vec<_> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
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
) -> Result<(Vec<SymbolEntry>, Vec<TextEntry>, Vec<ReferenceEntry>)> {
    let blocks = sfc::extract_script_blocks(source, extension);

    if blocks.is_empty() {
        return Ok((Vec::new(), Vec::new(), Vec::new()));
    }

    let mut all_symbols = Vec::new();
    let mut all_texts = Vec::new();
    let mut all_refs = Vec::new();

    for block in &blocks {
        // Parse each script block with the detected language
        let (mut symbols, mut texts, mut refs) =
            match parse_file(&block.content, block.lang, file_path) {
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
            for r in &mut refs {
                r.line[0] += offset;
                r.line[1] += offset;
            }
        }

        all_symbols.extend(symbols);
        all_texts.extend(texts);
        all_refs.extend(refs);
    }

    Ok((all_symbols, all_texts, all_refs))
}
