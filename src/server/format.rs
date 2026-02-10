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

/// Enriched search result with type discriminator and optional snippet for symbols.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum EnrichedSearchResult {
    Symbol {
        #[serde(flatten)]
        symbol: SymbolEntry,
        #[serde(skip_serializing_if = "Option::is_none")]
        snippet: Option<String>,
    },
    File(FileEntry),
    Text(TextEntry),
}

fn format_search_results_text(results: &[EnrichedSearchResult]) -> String {
    let mut out = String::new();
    for result in results {
        match result {
            EnrichedSearchResult::Symbol { symbol, snippet } => {
                // file[line-range] symbol kind name
                let location = format_location(&symbol.file, symbol.line);
                let _ = writeln!(out, "{} symbol {} {}", location, symbol.kind, symbol.name);
                if let Some(snip) = snippet {
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

/// Write a snippet indented with "│" prefix.
fn write_snippet(out: &mut String, snippet: &str) {
    for line in snippet.lines() {
        let _ = writeln!(out, "  │ {}", line);
    }
}

/// Response wrapper for SymbolEntry with optional snippet.
#[derive(Debug, Serialize)]
pub struct SymbolWithSnippet {
    #[serde(flatten)]
    pub symbol: SymbolEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
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
    let mut current_file: Option<&str> = None;

    for sym in symbols {
        // Print file header if changed
        if current_file != Some(&sym.symbol.file) {
            if current_file.is_some() {
                out.push('\n');
            }
            let _ = writeln!(out, "{}", sym.symbol.file);
            current_file = Some(&sym.symbol.file);
        }

        // Indent based on parent (simple: 2 spaces if has parent)
        let indent = if sym.symbol.parent.is_some() {
            "    "
        } else {
            "  "
        };

        // line  kind  name  [tokens/signature]
        let tokens = sym
            .symbol
            .tokens
            .as_ref()
            .map(|s| format!("  {}", s))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{}{:>4}  {} {}{}",
            indent, sym.symbol.line[0], sym.symbol.kind, sym.symbol.name, tokens
        );

        // Snippet if present
        if let Some(snip) = &sym.snippet {
            write_snippet(&mut out, snip);
        }
    }
    out
}

/// Format imports (simplified symbols).
pub fn format_imports(
    imports: &[SymbolEntry],
    format: OutputFormat,
) -> Result<String, serde_json::Error> {
    match format {
        OutputFormat::Json => serde_json::to_string_pretty(imports),
        OutputFormat::Text => Ok(format_imports_text(imports)),
    }
}

fn format_imports_text(imports: &[SymbolEntry]) -> String {
    let mut out = String::new();
    for imp in imports {
        let _ = writeln!(out, "{:>4}  {}", imp.line[0], imp.name);
    }
    out
}

/// Response wrapper for ReferenceEntry with optional snippet.
#[derive(Debug, Serialize)]
pub struct ReferenceWithSnippet {
    #[serde(flatten)]
    pub reference: ReferenceEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
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
        // file:line  kind  caller -> name (callee)
        let caller = r.reference.caller.as_deref().unwrap_or("(top-level)");
        let _ = writeln!(
            out,
            "{}:{}  {}  {} -> {}",
            r.reference.file, r.reference.line[0], r.reference.kind, caller, r.reference.name
        );

        // Snippet if present
        if let Some(snip) = &r.snippet {
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

fn format_explore_text(result: &ExploreResult) -> String {
    let mut out = String::new();

    // Project name/path
    let name = if result.metadata.name.is_empty() {
        result.project.as_deref().unwrap_or("(root)")
    } else {
        &result.metadata.name
    };
    let _ = writeln!(out, "{}/", name);

    // Description if present
    if let Some(desc) = &result.metadata.description {
        let _ = writeln!(out, "  {}", desc);
        out.push('\n');
    }

    // Subprojects
    if let Some(subs) = &result.subprojects {
        let _ = writeln!(out, "Subprojects: {}", subs.join(", "));
        out.push('\n');
    }

    // Directories with tree-like structure
    for (dir, files) in &result.directories {
        let dir_display = if dir == "." { "(root)" } else { dir.as_str() };
        let _ = writeln!(out, "{}/ ({} files)", dir_display, files.len());
        for file in files.iter().take(10) {
            let _ = writeln!(out, "  {}", file);
        }
        if files.len() > 10 {
            let _ = writeln!(out, "  ... and {} more", files.len() - 10);
        }
    }

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
