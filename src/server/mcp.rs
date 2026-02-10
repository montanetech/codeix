use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::Args;
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    schemars,
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};

use super::db::{SearchDb, SearchResult};
use super::snippet::SnippetExtractor;
use crate::index::format::SymbolEntry;
use crate::mount::MountTable;
use crate::mount::handler::flush_dirty_mounts;
use crate::utils::manifest::{self, ProjectMetadata};

// Parameter structs for each tool - shared between MCP and REPL
// NOTE: When adding/removing/renaming tools, also update src/cli/query.rs (QueryCommand enum)

/// Parameters for the unified search tool.
#[derive(Debug, Deserialize, JsonSchema, Args)]
pub struct SearchParams {
    /// Search query (FTS5 syntax, supports * wildcards)
    pub query: String,
    /// Scope: types to search. Comma-separated: "symbol", "file", "text". Default: all.
    #[arg(short, long, value_delimiter = ',')]
    pub scope: Option<Vec<String>>,
    /// Filter by kind (symbol kind, text kind, or file language)
    #[arg(short, long)]
    pub kind: Option<String>,
    /// Filter by file path. Supports glob patterns with * (e.g. "src/*.py")
    #[arg(short = 'f', long)]
    pub path: Option<String>,
    /// Filter by project (relative path from workspace root, e.g. "libs/utils")
    #[arg(short, long)]
    pub project: Option<String>,
    /// Maximum number of results to return (default: 100)
    #[arg(short, long)]
    pub limit: Option<u32>,
    /// Number of results to skip for pagination (default: 0)
    #[arg(short, long)]
    pub offset: Option<u32>,
    /// Number of code snippet lines for symbols: 0=none, -1=all, N=N lines (default: 10)
    #[arg(long)]
    pub snippet_lines: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema, Args)]
pub struct GetFileSymbolsParams {
    /// File path to get symbols for
    pub file: String,
    /// Number of code snippet lines: 0=none, -1=all, N=N lines (default: 10)
    #[arg(long)]
    pub snippet_lines: Option<i32>,
    /// Maximum number of results to return (default: 100)
    #[arg(short, long)]
    pub limit: Option<u32>,
    /// Number of results to skip for pagination (default: 0)
    #[arg(short, long)]
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema, Args)]
pub struct GetChildrenParams {
    /// File path containing the parent symbol
    pub file: String,
    /// Name of the parent symbol
    pub parent: String,
    /// Number of code snippet lines: 0=none, -1=all, N=N lines (default: 10)
    #[arg(long)]
    pub snippet_lines: Option<i32>,
    /// Maximum number of results to return (default: 100)
    #[arg(short, long)]
    pub limit: Option<u32>,
    /// Number of results to skip for pagination (default: 0)
    #[arg(short, long)]
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema, Args)]
pub struct GetImportsParams {
    /// File path to get imports for
    pub file: String,
    /// Maximum number of results to return (default: 100)
    #[arg(short, long)]
    pub limit: Option<u32>,
    /// Number of results to skip for pagination (default: 0)
    #[arg(short, long)]
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema, Args)]
pub struct GetCallersParams {
    /// Symbol name to find callers for (e.g. "my_function", "MyClass.method")
    pub name: String,
    /// Filter by reference kind (e.g. "call", "import", "type_annotation")
    #[arg(short, long)]
    pub kind: Option<String>,
    /// Filter by project (relative path from workspace root, e.g. "libs/utils")
    #[arg(short, long)]
    pub project: Option<String>,
    /// Maximum number of results to return (default: 100)
    #[arg(short, long)]
    pub limit: Option<u32>,
    /// Number of results to skip for pagination (default: 0)
    #[arg(short, long)]
    pub offset: Option<u32>,
    /// Number of code snippet lines: 0=none, -1=all, N=N lines (default: 10)
    #[arg(long)]
    pub snippet_lines: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema, Args)]
pub struct GetCalleesParams {
    /// Symbol name to find callees for (e.g. "my_function", "MyClass.method")
    pub caller: String,
    /// Filter by reference kind (e.g. "call", "import", "type_annotation")
    #[arg(short, long)]
    pub kind: Option<String>,
    /// Filter by project (relative path from workspace root, e.g. "libs/utils")
    #[arg(short, long)]
    pub project: Option<String>,
    /// Maximum number of results to return (default: 100)
    #[arg(short, long)]
    pub limit: Option<u32>,
    /// Number of results to skip for pagination (default: 0)
    #[arg(short, long)]
    pub offset: Option<u32>,
    /// Number of code snippet lines: 0=none, -1=all, N=N lines (default: 10)
    #[arg(long)]
    pub snippet_lines: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema, Args)]
pub struct ExploreParams {
    /// Filter to directory path (relative to project root, e.g. "src/server")
    pub path: Option<String>,
    /// Filter by project (relative path from workspace root, defaults to root project)
    #[arg(short, long)]
    pub project: Option<String>,
    /// Max files to display (default: 200). If exceeded, files are capped per directory with "+N files" indicators.
    #[arg(short, long, default_value = "200")]
    pub max_entries: u32,
}
/// Response wrapper for SymbolEntry with optional snippet.
#[derive(Debug, Serialize)]
struct SymbolWithSnippet {
    #[serde(flatten)]
    symbol: SymbolEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
}

/// Response wrapper for ReferenceEntry with optional snippet.
#[derive(Debug, Serialize)]
struct ReferenceWithSnippet {
    #[serde(flatten)]
    reference: crate::index::format::ReferenceEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
}

/// Enriched search result with type discriminator and optional snippet for symbols.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum EnrichedSearchResult {
    Symbol {
        #[serde(flatten)]
        symbol: SymbolEntry,
        #[serde(skip_serializing_if = "Option::is_none")]
        snippet: Option<String>,
    },
    File(crate::index::format::FileEntry),
    Text(crate::index::format::TextEntry),
}

/// MCP server exposing code-index query tools.
///
/// `SearchDb` wraps a `rusqlite::Connection` which is not `Sync`, so we protect
/// it with a `Mutex` to satisfy rmcp's `Send + Sync` requirements.
#[derive(Clone)]
pub struct CodeIndexServer {
    db: Arc<Mutex<SearchDb>>,
    mount_table: Arc<Mutex<MountTable>>,
    snippet_extractor: SnippetExtractor,
    tool_router: ToolRouter<Self>,
}

impl CodeIndexServer {
    pub fn new(db: Arc<Mutex<SearchDb>>, mount_table: Arc<Mutex<MountTable>>) -> Self {
        let workspace_root = mount_table
            .lock()
            .expect("mount table lock poisoned")
            .workspace_root()
            .to_path_buf();

        Self {
            db,
            mount_table,
            snippet_extractor: SnippetExtractor::new(workspace_root),
            tool_router: Self::tool_router(),
        }
    }

    /// Enrich symbols with snippets, filtering out symbols whose files are missing.
    fn enrich_with_snippets(
        &self,
        symbols: Vec<SymbolEntry>,
        snippet_lines: i32,
    ) -> Vec<SymbolWithSnippet> {
        symbols
            .into_iter()
            .filter_map(|symbol| {
                // Skip symbols whose files are missing
                if !self
                    .snippet_extractor
                    .file_exists(&symbol.project, &symbol.file)
                {
                    return None;
                }

                let snippet = self.snippet_extractor.extract_snippet(
                    &symbol.project,
                    &symbol.file,
                    symbol.line[0],
                    symbol.line[1],
                    snippet_lines,
                );
                Some(SymbolWithSnippet { symbol, snippet })
            })
            .collect()
    }

    /// Enrich references with snippets, filtering out refs whose files are missing.
    fn enrich_refs_with_snippets(
        &self,
        references: Vec<crate::index::format::ReferenceEntry>,
        snippet_lines: i32,
    ) -> Vec<ReferenceWithSnippet> {
        references
            .into_iter()
            .filter_map(|reference| {
                // Skip refs whose files are missing
                if !self
                    .snippet_extractor
                    .file_exists(&reference.project, &reference.file)
                {
                    return None;
                }

                let snippet = self.snippet_extractor.extract_snippet(
                    &reference.project,
                    &reference.file,
                    reference.line[0],
                    reference.line[1],
                    snippet_lines,
                );
                Some(ReferenceWithSnippet { reference, snippet })
            })
            .collect()
    }
}

#[tool_router]
impl CodeIndexServer {
    /// Unified search across symbols, files, and texts.
    #[tool(
        description = "Search symbols, files, and texts. FTS5 with BM25 ranking.\n\n\
**Query:** FTS5 syntax — `foo`, `foo OR bar`, `foo*` (prefix), `\"exact phrase\"`, `foo -exclude`\n\n\
**Symbol kinds:** `function`, `method`, `class`, `struct`, `interface`, `enum`, `constant`, `variable`, `property`, `module`, `import`, `impl`, `section`\n\
- Go/Rust/C: use `struct` not `class`\n\
- Rust: use `interface` for traits\n\n\
**Text kinds:** `docstring`, `comment`, `string`, `sample`\n\n\
**Params:** query (required), scope ([\"symbol\"]/[\"file\"]/[\"text\"]), kind, path (glob), project, limit (default 10), offset, snippet_lines (default 10)"
    )]
    pub async fn search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;

        let scope = params.scope.unwrap_or_default();
        let limit = params.limit.unwrap_or(10);
        let offset = params.offset.unwrap_or(0);

        let results = db
            .search(
                &params.query,
                &scope,
                params.kind.as_deref(),
                params.path.as_deref(),
                params.project.as_deref(),
                limit,
                offset,
            )
            .map_err(|e| McpError::internal_error(format!("search failed: {e}"), None))?;

        drop(db); // Release lock before file I/O

        // Enrich symbol results with snippets
        let snippet_lines = params.snippet_lines.unwrap_or(10);
        let enriched: Vec<EnrichedSearchResult> = results
            .into_iter()
            .filter_map(|result| match result {
                SearchResult::Symbol(symbol) => {
                    // Filter out symbols whose files are missing
                    if !self
                        .snippet_extractor
                        .file_exists(&symbol.project, &symbol.file)
                    {
                        return None;
                    }
                    let snippet = self.snippet_extractor.extract_snippet(
                        &symbol.project,
                        &symbol.file,
                        symbol.line[0],
                        symbol.line[1],
                        snippet_lines,
                    );
                    Some(EnrichedSearchResult::Symbol { symbol, snippet })
                }
                SearchResult::File(file) => Some(EnrichedSearchResult::File(file)),
                SearchResult::Text(text) => Some(EnrichedSearchResult::Text(text)),
            })
            .collect();

        let json = serde_json::to_string_pretty(&enriched)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Get all symbols in a file, ordered by line number.
    #[tool(
        description = "Get all symbols in a file, ordered by line number. Returns code snippets by default."
    )]
    pub async fn get_file_symbols(
        &self,
        Parameters(params): Parameters<GetFileSymbolsParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(100);
        let offset = params.offset.unwrap_or(0);

        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .get_file_symbols(&params.file, limit, offset)
            .map_err(|e| McpError::internal_error(format!("get_file_symbols failed: {e}"), None))?;

        drop(db); // Release lock before file I/O

        let snippet_lines = params.snippet_lines.unwrap_or(10);
        let enriched = self.enrich_with_snippets(results, snippet_lines);

        let json = serde_json::to_string_pretty(&enriched)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Get direct children of a symbol (e.g. methods of a class).
    #[tool(
        description = "Get direct children of a symbol (e.g. methods of a class). Returns code snippets by default."
    )]
    pub async fn get_children(
        &self,
        Parameters(params): Parameters<GetChildrenParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(100);
        let offset = params.offset.unwrap_or(0);

        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .get_children(&params.file, &params.parent, limit, offset)
            .map_err(|e| McpError::internal_error(format!("get_children failed: {e}"), None))?;

        drop(db); // Release lock before file I/O

        let snippet_lines = params.snippet_lines.unwrap_or(10);
        let enriched = self.enrich_with_snippets(results, snippet_lines);

        let json = serde_json::to_string_pretty(&enriched)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Get all import statements for a file.
    #[tool(description = "Get all import statements for a file")]
    pub async fn get_imports(
        &self,
        Parameters(params): Parameters<GetImportsParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(100);
        let offset = params.offset.unwrap_or(0);

        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .get_imports(&params.file, limit, offset)
            .map_err(|e| McpError::internal_error(format!("get_imports failed: {e}"), None))?;

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Explore project structure: files grouped by directory with metadata.
    #[tool(
        description = "Explore a project's file structure. Returns project metadata, subprojects (if any), and files grouped by directory. Use 'path' to scope to a subdirectory. Files are capped per directory if total exceeds max_entries (default: 200)."
    )]
    pub async fn explore(
        &self,
        Parameters(params): Parameters<ExploreParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut project_path = params.project.as_deref().unwrap_or("").to_string();
        let mut path_filter = params.path.clone();

        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;

        // If path is specified but no project, check if path matches a subproject
        // e.g., `explore flask` should explore the flask subproject, not flask/ in root
        if params.project.is_none()
            && let Some(ref path) = path_filter
        {
            let all_projects = db.list_projects().map_err(|e| {
                McpError::internal_error(format!("list_projects failed: {e}"), None)
            })?;

            // Check if path exactly matches a subproject, or starts with one
            for proj in &all_projects {
                if proj.is_empty() {
                    continue;
                }
                if path == proj {
                    // Exact match: explore the subproject root
                    project_path = proj.clone();
                    path_filter = None;
                    break;
                } else if path.starts_with(&format!("{}/", proj)) {
                    // Path is inside a subproject: explore that subproject with remaining path
                    project_path = proj.clone();
                    path_filter = Some(path[proj.len() + 1..].to_string());
                    break;
                }
            }
        }

        drop(db);

        let mt = self.mount_table.lock().map_err(|e| {
            McpError::internal_error(format!("mount table lock poisoned: {e}"), None)
        })?;

        // Resolve project root for metadata extraction
        let project_root = mt.project_root(&project_path).ok_or_else(|| {
            McpError::invalid_params(format!("Project not found: '{}'", project_path), None)
        })?;

        // Extract project metadata
        let metadata = manifest::extract_metadata(&project_root);

        drop(mt);

        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;

        // Find subprojects when exploring without path filter
        let subprojects: Vec<String> = if path_filter.is_none() {
            db.list_projects()
                .map_err(|e| McpError::internal_error(format!("list_projects failed: {e}"), None))?
                .into_iter()
                .filter(|p| {
                    if project_path.is_empty() {
                        // Root project: include all non-empty projects (subprojects at any depth)
                        !p.is_empty()
                    } else {
                        // Subproject: include projects that start with this path + '/'
                        p.starts_with(&format!("{}/", project_path))
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        // Get overview: count per (parent_path, lang)
        let max_entries = params.max_entries as usize;
        let overview = db
            .explore_dir_overview(&project_path, path_filter.as_deref())
            .map_err(|e| {
                McpError::internal_error(format!("explore_dir_overview failed: {e}"), None)
            })?;

        // Count files with known language (code + markdown), excluding lang=NULL
        let mut total_known_files = 0usize;
        let mut num_known_groups = 0usize;
        // Store overview by (dir, lang) -> count
        let mut overview_map: BTreeMap<(String, Option<String>), usize> = BTreeMap::new();
        for (dir, lang, count) in &overview {
            overview_map.insert((dir.clone(), lang.clone()), *count);
            // Count files with known language (lang is set)
            if lang.is_some() {
                total_known_files += count;
                num_known_groups += 1;
            }
        }

        // Compute cap: if total known files fit, no cap needed (use large number)
        let cap = if total_known_files <= max_entries {
            usize::MAX
        } else {
            // Distribute budget evenly across known groups, minimum 1
            (max_entries / num_known_groups.max(1)).max(1)
        };

        // Fetch files with known language, capped at cap per (dir, lang)
        let files = db
            .explore_files_capped(&project_path, path_filter.as_deref(), cap)
            .map_err(|e| {
                McpError::internal_error(format!("explore_files_capped failed: {e}"), None)
            })?;

        drop(db);

        // Group files by directory
        let mut files_by_dir: BTreeMap<String, Vec<String>> = BTreeMap::new();
        // Track how many files we got per (dir, lang) to compute remainder
        let mut fetched_counts: BTreeMap<(String, Option<String>), usize> = BTreeMap::new();
        for (dir, filename, lang) in files {
            files_by_dir.entry(dir.clone()).or_default().push(filename);
            *fetched_counts.entry((dir, lang)).or_default() += 1;
        }

        // Build result with "+N lang files" indicators for truncated groups
        let mut result_dirs: BTreeMap<String, Vec<String>> = BTreeMap::new();

        // First, collect all directories from overview (including those with only markdown)
        let mut all_dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (dir, _lang, _count) in &overview {
            all_dirs.insert(dir.clone());
        }

        for dir in all_dirs {
            let mut entries = files_by_dir.remove(&dir).unwrap_or_default();

            // Check each (dir, lang) group for remainder
            let mut remainders: BTreeMap<String, usize> = BTreeMap::new(); // lang -> remaining count
            for ((d, lang), total) in &overview_map {
                if d != &dir {
                    continue;
                }
                let fetched = fetched_counts
                    .get(&(d.clone(), lang.clone()))
                    .copied()
                    .unwrap_or(0);
                let remaining = total.saturating_sub(fetched);
                if remaining > 0 {
                    let lang_name = lang.as_deref().unwrap_or("other");
                    *remainders.entry(lang_name.to_string()).or_default() += remaining;
                }
            }

            // Add remainder indicators
            for (lang, count) in remainders {
                entries.push(format!("+{} {} files", count, lang));
            }

            if !entries.is_empty() {
                result_dirs.insert(dir, entries);
            }
        }

        // Build response
        let result = ExploreResult {
            project: if project_path.is_empty() {
                None
            } else {
                Some(project_path.to_string())
            },
            metadata,
            subprojects: if subprojects.is_empty() {
                None
            } else {
                Some(subprojects)
            },
            directories: result_dirs,
        };

        let json = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Get all references TO a symbol (who calls/uses this symbol).
    #[tool(
        description = "Find all places that call or reference a symbol. Returns references sorted by file and line. Useful for understanding symbol usage and finding callers of a function."
    )]
    pub async fn get_callers(
        &self,
        Parameters(params): Parameters<GetCallersParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(100);
        let offset = params.offset.unwrap_or(0);

        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .get_callers(
                &params.name,
                params.kind.as_deref(),
                params.project.as_deref(),
                limit,
                offset,
            )
            .map_err(|e| McpError::internal_error(format!("get_callers failed: {e}"), None))?;

        drop(db); // Release lock before file I/O

        let snippet_lines = params.snippet_lines.unwrap_or(10);
        let enriched = self.enrich_refs_with_snippets(results, snippet_lines);

        let json = serde_json::to_string_pretty(&enriched)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Get all references FROM a symbol (what does this symbol call/use).
    #[tool(
        description = "Find all symbols that a given function/method calls or references. Returns references sorted by file and line. Useful for understanding dependencies and call chains."
    )]
    pub async fn get_callees(
        &self,
        Parameters(params): Parameters<GetCalleesParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(100);
        let offset = params.offset.unwrap_or(0);

        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .get_callees(
                &params.caller,
                params.kind.as_deref(),
                params.project.as_deref(),
                limit,
                offset,
            )
            .map_err(|e| McpError::internal_error(format!("get_callees failed: {e}"), None))?;

        drop(db); // Release lock before file I/O

        let snippet_lines = params.snippet_lines.unwrap_or(10);
        let enriched = self.enrich_refs_with_snippets(results, snippet_lines);

        let json = serde_json::to_string_pretty(&enriched)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Flush pending index changes to disk.
    #[tool(
        description = "Flush pending index changes to .codeindex/ files on disk. Call this when you need the index persisted (e.g., before git operations). Returns the number of projects flushed."
    )]
    pub async fn flush_index(&self) -> Result<CallToolResult, McpError> {
        let flushed = flush_dirty_mounts(&self.mount_table, &self.db)
            .map_err(|e| McpError::internal_error(format!("flush_index failed: {e}"), None))?;

        let message = if flushed == 0 {
            "No pending changes to flush.".to_string()
        } else {
            format!("Flushed {} project(s) to disk.", flushed)
        };

        Ok(CallToolResult::success(vec![Content::text(message)]))
    }
}

/// Result of explore tool: project metadata + files grouped by directory.
#[derive(Debug, Serialize)]
struct ExploreResult {
    /// Relative path from workspace root (omitted for root project)
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    /// Metadata extracted from package manifests
    #[serde(flatten)]
    metadata: ProjectMetadata,
    /// Subprojects (direct children only, omitted if none)
    #[serde(skip_serializing_if = "Option::is_none")]
    subprojects: Option<Vec<String>>,
    /// Files grouped by directory path ("." for root, "src/server" for nested)
    directories: BTreeMap<String, Vec<String>>,
}

#[tool_handler]
impl ServerHandler for CodeIndexServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
"Code index providing full-text search and structural navigation over indexed codebases.

**Tools:**
- `explore`: Project structure — metadata, subprojects, files grouped by directory.
- `search`: Unified FTS across symbols, files, and texts. BM25-ranked results.
- `get_file_symbols`: All symbols in a file, ordered by line number.
- `get_children`: Direct children of a symbol (e.g., methods of a class).
- `get_imports`: Import statements in a file.
- `get_callers`: Find all places that call/reference a symbol.
- `get_callees`: Find all symbols that a function/method calls.
- `flush_index`: Persist pending changes to .codeindex/ files.

**Common parameters:**
- `limit` (default 100): Maximum results to return
- `offset` (default 0): Skip N results for pagination
- `snippet_lines` (default 10): Code context lines (0=none, -1=all, N=lines)
- `kind`: Filter by symbol kind (function, method, class, struct, interface, enum, constant, variable, property, module, import, impl) or reference kind (call, import, type_annotation)
- `project`: Filter by project path (relative from workspace root)"
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

/// Extract text content from a CallToolResult.
pub fn extract_result_text(result: &CallToolResult) -> String {
    use rmcp::model::RawContent;
    result
        .content
        .iter()
        .filter_map(|c| match &c.raw {
            RawContent::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Start the MCP server over stdio with the given search database and mount table.
pub async fn start_server(
    db: Arc<Mutex<SearchDb>>,
    mount_table: Arc<Mutex<MountTable>>,
) -> Result<()> {
    let server = CodeIndexServer::new(db, mount_table);
    let service = server
        .serve(stdio())
        .await
        .map_err(|e| anyhow::anyhow!("MCP serve error: {e}"))?;
    service
        .waiting()
        .await
        .map_err(|e| anyhow::anyhow!("MCP runtime error: {e}"))?;
    Ok(())
}
