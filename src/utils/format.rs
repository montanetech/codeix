//! Output formatting for MCP tools.
//!
//! Provides both JSON (default, for MCP clients) and human-readable text
//! (for CLI/REPL) output formats.

use std::collections::BTreeMap;
use std::fmt::Write;

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::index::format::{FileEntry, ReferenceEntry, SymbolEntry, TextEntry};
use crate::utils::manifest::ProjectMetadata;

/// Output format for tool results.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// JSON output (default, for MCP clients)
    #[default]
    Json,
    /// Human-readable text output (for CLI/REPL)
    Text,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "json" => Ok(OutputFormat::Json),
            "text" => Ok(OutputFormat::Text),
            _ => Err(format!("invalid format '{}', expected 'json' or 'text'", s)),
        }
    }
}

/// Format a list of enriched search results.
pub fn format_search_results(
    results: &[EnrichedSearchResult],
    format: OutputFormat,
) -> Result<String, serde_json::Error> {
    match format {
        OutputFormat::Json => serde_json::to_string_pretty(results),
        OutputFormat::Text => Ok(format_search_results_text(results)),
    }
}

/// Enriched search result with type discriminator and optional context for symbols.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum EnrichedSearchResult {
    Symbol {
        #[serde(flatten)]
        symbol: SymbolEntry,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<String>,
    },
    File(FileEntry),
    Text(TextEntry),
}

fn format_search_results_text(results: &[EnrichedSearchResult]) -> String {
    let mut out = String::new();
    for result in results {
        match result {
            EnrichedSearchResult::Symbol { symbol, context } => {
                // file[line-range] symbol kind name
                let location = format_location(&symbol.file, symbol.line);
                let _ = writeln!(out, "{} symbol {} {}", location, symbol.kind, symbol.name);
                if let Some(snip) = context {
                    write_snippet(&mut out, snip);
                }
            }
            EnrichedSearchResult::File(file) => {
                // path file (lang, lines)
                let lang = file.lang.as_deref().unwrap_or("-");
                let _ = writeln!(out, "{} file ({}, {} lines)", file.path, lang, file.lines);
            }
            EnrichedSearchResult::Text(text) => {
                // file[line] text kind preview
                let location = format_location(&text.file, text.line);
                let preview: String = text.text.chars().take(40).collect();
                let _ = writeln!(out, "{} text {} {}...", location, text.kind, preview);
            }
        }
    }
    out
}

/// Format file location with brackets, using range if start != end.
fn format_location(file: &str, line: [u32; 2]) -> String {
    if line[0] == line[1] {
        format!("{}[{}]", file, line[0])
    } else {
        format!("{}[{}-{}]", file, line[0], line[1])
    }
}

/// Write a snippet indented with "│" prefix, followed by a blank line.
/// Removes common leading whitespace from all lines (dedent).
fn write_snippet(out: &mut String, snippet: &str) {
    let dedented = dedent(snippet);
    for line in dedented.lines() {
        let _ = writeln!(out, "  │ {}", line);
    }
    out.push('\n');
}

/// Remove common leading whitespace from all lines.
fn dedent(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return String::new();
    }

    // Find minimum indentation (ignoring empty lines)
    let min_indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);

    if min_indent == 0 {
        return text.to_string();
    }

    // Remove the common indentation
    lines
        .iter()
        .map(|line| {
            if line.len() >= min_indent {
                &line[min_indent..]
            } else {
                *line // Empty or whitespace-only line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Response wrapper for SymbolEntry with optional context.
#[derive(Debug, Serialize)]
pub struct SymbolWithSnippet {
    #[serde(flatten)]
    pub symbol: SymbolEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Format a list of symbols (for get_file_symbols, get_children).
pub fn format_symbols(
    symbols: &[SymbolWithSnippet],
    format: OutputFormat,
) -> Result<String, serde_json::Error> {
    match format {
        OutputFormat::Json => serde_json::to_string_pretty(symbols),
        OutputFormat::Text => Ok(format_symbols_text(symbols)),
    }
}

fn format_symbols_text(symbols: &[SymbolWithSnippet]) -> String {
    if symbols.is_empty() {
        return String::new();
    }

    let mut out = String::new();

    for sym in symbols {
        // file[line-range] symbol kind name [tokens]
        let location = format_location(&sym.symbol.file, sym.symbol.line);
        let tokens = sym
            .symbol
            .tokens
            .as_ref()
            .map(|s| format!(" {}", s))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{} {} {}{}",
            location, sym.symbol.kind, sym.symbol.name, tokens
        );

        // Snippet if present
        if let Some(snip) = &sym.context {
            write_snippet(&mut out, snip);
        }
    }
    out
}

/// Response wrapper for ReferenceEntry with optional context.
#[derive(Debug, Serialize)]
pub struct ReferenceWithSnippet {
    #[serde(flatten)]
    pub reference: ReferenceEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Format references (for get_callers, get_callees).
pub fn format_references(
    refs: &[ReferenceWithSnippet],
    format: OutputFormat,
) -> Result<String, serde_json::Error> {
    match format {
        OutputFormat::Json => serde_json::to_string_pretty(refs),
        OutputFormat::Text => Ok(format_references_text(refs)),
    }
}

fn format_references_text(refs: &[ReferenceWithSnippet]) -> String {
    let mut out = String::new();
    for r in refs {
        // file[line] ref kind caller -> name
        let location = format_location(&r.reference.file, r.reference.line);
        let caller = r.reference.caller.as_deref().unwrap_or("(top-level)");
        let _ = writeln!(
            out,
            "{} ref {} {} -> {}",
            location, r.reference.kind, caller, r.reference.name
        );

        // Snippet if present
        if let Some(snip) = &r.context {
            write_snippet(&mut out, snip);
        }
    }
    out
}

/// Result of explore tool: project metadata + files grouped by directory.
#[derive(Debug, Serialize)]
pub struct ExploreResult {
    /// Relative path from workspace root (omitted for root project)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Metadata extracted from package manifests
    #[serde(flatten)]
    pub metadata: ProjectMetadata,
    /// Subprojects (direct children only, omitted if none)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subprojects: Option<Vec<String>>,
    /// Files grouped by directory path ("." for root, "src/server" for nested)
    pub directories: BTreeMap<String, Vec<String>>,
}

/// Format explore result.
pub fn format_explore(
    result: &ExploreResult,
    format: OutputFormat,
) -> Result<String, serde_json::Error> {
    match format {
        OutputFormat::Json => serde_json::to_string_pretty(result),
        OutputFormat::Text => Ok(format_explore_text(result)),
    }
}

/// A node in the directory tree for rendering.
/// Each node represents a directory path segment (possibly collapsed like "src/main/java").
#[derive(Debug, Default)]
struct TreeNode {
    /// Files directly in this directory
    files: Vec<String>,
    /// Child directories (path segment -> subtree)
    /// The key can be a single component "foo" or collapsed "foo/bar/baz"
    children: BTreeMap<String, TreeNode>,
}

impl TreeNode {
    /// Insert a path into the tree, collapsing single-child chains.
    fn insert(&mut self, path: &str, files: Vec<String>) {
        if path.is_empty() {
            self.files = files;
            return;
        }

        // Find existing child that shares a prefix with this path
        let mut matching_key: Option<String> = None;
        let mut common_len = 0;

        for key in self.children.keys() {
            let prefix_len = common_prefix_len(key, path);
            if prefix_len > 0 && (matching_key.is_none() || prefix_len > common_len) {
                matching_key = Some(key.clone());
                common_len = prefix_len;
            }
        }

        if let Some(key) = matching_key {
            // We found a child with common prefix
            if path == key {
                // Exact match - add files to existing node
                self.children.get_mut(&key).unwrap().files = files;
            } else if path.starts_with(&format!("{}/", key)) {
                // path is under key (e.g., key="src", path="src/main")
                let remaining = &path[key.len() + 1..];
                self.children
                    .get_mut(&key)
                    .unwrap()
                    .insert(remaining, files);
            } else if key.starts_with(&format!("{}/", path)) {
                // key is under path (e.g., path="src", key="src/main")
                // Need to split: create new node for path, move old child under it
                let old_child = self.children.remove(&key).unwrap();
                let remaining = &key[path.len() + 1..];
                let mut new_node = TreeNode {
                    files,
                    children: BTreeMap::new(),
                };
                new_node.children.insert(remaining.to_string(), old_child);
                self.children.insert(path.to_string(), new_node);
            } else {
                // Partial overlap - need to split at common prefix
                // e.g., key="src/main/java", path="src/main/resources"
                // common="src/main", need to restructure
                let common = &path[..common_len];
                // Make sure we split at a / boundary
                let common = if let Some(pos) = common.rfind('/') {
                    &common[..pos]
                } else {
                    // No common directory boundary, treat as separate
                    self.children.insert(
                        path.to_string(),
                        TreeNode {
                            files,
                            children: BTreeMap::new(),
                        },
                    );
                    return;
                };

                let old_child = self.children.remove(&key).unwrap();
                let key_remaining = &key[common.len() + 1..];
                let path_remaining = &path[common.len() + 1..];

                let mut new_node = TreeNode::default();
                new_node
                    .children
                    .insert(key_remaining.to_string(), old_child);
                new_node.insert(path_remaining, files);
                self.children.insert(common.to_string(), new_node);
            }
        } else {
            // No matching child, insert as new
            self.children.insert(
                path.to_string(),
                TreeNode {
                    files,
                    children: BTreeMap::new(),
                },
            );
        }
    }

    /// Render this node and its children as a tree.
    fn render(&self, out: &mut String, prefix: &str, is_last: bool, name: Option<&str>) {
        // Write directory name if we have one (not for root)
        if let Some(dir_name) = name {
            let connector = if is_last { "└── " } else { "├── " };
            let _ = writeln!(out, "{}{}{}/", prefix, connector, dir_name);
        }

        // Calculate child prefix for our children
        let child_prefix = if name.is_some() {
            format!("{}{}", prefix, if is_last { "    " } else { "│   " })
        } else {
            prefix.to_string()
        };

        // Collect all items: child directories and files
        let child_dirs: Vec<_> = self.children.keys().collect();
        let total_items = child_dirs.len() + self.files.len();
        let mut item_idx = 0;

        // Render child directories first
        for dir_name in &child_dirs {
            let is_last_item = item_idx == total_items - 1;
            if let Some(child) = self.children.get(*dir_name) {
                child.render(out, &child_prefix, is_last_item, Some(dir_name));
            }
            item_idx += 1;
        }

        // Render files
        for file in &self.files {
            let is_last_item = item_idx == total_items - 1;
            let connector = if is_last_item {
                "└── "
            } else {
                "├── "
            };
            let _ = writeln!(out, "{}{}{}", child_prefix, connector, file);
            item_idx += 1;
        }
    }
}

/// Find length of common prefix between two strings.
fn common_prefix_len(a: &str, b: &str) -> usize {
    a.chars()
        .zip(b.chars())
        .take_while(|(ca, cb)| ca == cb)
        .count()
}

fn format_explore_text(result: &ExploreResult) -> String {
    let mut out = String::new();

    // Header line: name - description
    let name = if result.metadata.name.is_empty() {
        result.project.as_deref().unwrap_or(".")
    } else {
        &result.metadata.name
    };
    if let Some(desc) = &result.metadata.description {
        let _ = writeln!(out, "{}: {}", name, desc);
    } else {
        let _ = writeln!(out, "{}", name);
    }

    // Subprojects on separate line if present
    if let Some(subs) = &result.subprojects {
        let _ = writeln!(out, "subprojects: {}", subs.join(", "));
    }

    // Build tree structure from flat directory paths
    let mut root = TreeNode::default();
    for (dir, files) in &result.directories {
        if dir == "." {
            // Root files go directly in the root node
            root.files = files.clone();
        } else {
            // Insert path into tree (will collapse single-child chains)
            root.insert(dir, files.clone());
        }
    }

    // Render the tree (no name for root, starts at column 0)
    root.render(&mut out, "", true, None);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_parse() {
        assert_eq!("json".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
        assert_eq!("text".parse::<OutputFormat>().unwrap(), OutputFormat::Text);
        assert_eq!("JSON".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
        assert_eq!("TEXT".parse::<OutputFormat>().unwrap(), OutputFormat::Text);
        assert!("invalid".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn test_output_format_default() {
        assert_eq!(OutputFormat::default(), OutputFormat::Json);
    }
}
