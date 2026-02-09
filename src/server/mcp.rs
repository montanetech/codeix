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
    #[arg(short, long)]
    pub snippet_lines: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema, Args)]
pub struct GetSymbolChildrenParams {
    /// File path containing the parent symbol
    pub file: String,
    /// Name of the parent symbol
    pub parent: String,
    /// Number of code snippet lines: 0=none, -1=all, N=N lines (default: 10)
    #[arg(short, long)]
    pub snippet_lines: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema, Args)]
pub struct GetImportsParams {
    /// File path to get imports for
    pub file: String,
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
}

/// Response wrapper for SymbolEntry with optional snippet.
#[derive(Debug, Serialize)]
struct SymbolWithSnippet {
    #[serde(flatten)]
    symbol: SymbolEntry,
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
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .get_file_symbols(&params.file)
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
    pub async fn get_symbol_children(
        &self,
        Parameters(params): Parameters<GetSymbolChildrenParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .get_symbol_children(&params.file, &params.parent)
            .map_err(|e| {
                McpError::internal_error(format!("get_symbol_children failed: {e}"), None)
            })?;

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
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .get_imports(&params.file)
            .map_err(|e| McpError::internal_error(format!("get_imports failed: {e}"), None))?;

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// List all indexed projects with metadata from package manifests.
    #[tool(
        description = "List all indexed projects with metadata extracted from package manifests (package.json, Cargo.toml, pyproject.toml, go.mod, pom.xml, *.gemspec). Returns name, description, and list of manifest files found."
    )]
    pub async fn list_projects(&self) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let project_paths = db
            .list_projects()
            .map_err(|e| McpError::internal_error(format!("list_projects failed: {e}"), None))?;
        drop(db);

        let mt = self.mount_table.lock().map_err(|e| {
            McpError::internal_error(format!("mount table lock poisoned: {e}"), None)
        })?;

        // For each project path, get its root and extract metadata
        let results: Vec<ProjectInfo> = project_paths
            .into_iter()
            .map(|path| {
                let metadata = match mt.project_root(&path) {
                    Some(project_root) => manifest::extract_metadata(&project_root),
                    None => {
                        // Project in DB but not mounted - use path as name, no manifest files
                        let name = if path.is_empty() {
                            mt.workspace_root()
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("root")
                                .to_string()
                        } else {
                            path.split('/').next_back().unwrap_or(&path).to_string()
                        };
                        ProjectMetadata {
                            name,
                            description: None,
                            manifest_files: Vec::new(),
                        }
                    }
                };
                ProjectInfo { path, metadata }
            })
            .collect();

        drop(mt);

        let json = serde_json::to_string_pretty(&results)
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
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .get_callers(
                &params.name,
                params.kind.as_deref(),
                params.project.as_deref(),
            )
            .map_err(|e| McpError::internal_error(format!("get_callers failed: {e}"), None))?;

        let json = serde_json::to_string_pretty(&results)
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
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .get_callees(
                &params.caller,
                params.kind.as_deref(),
                params.project.as_deref(),
            )
            .map_err(|e| McpError::internal_error(format!("get_callees failed: {e}"), None))?;

        let json = serde_json::to_string_pretty(&results)
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

/// Project info returned by list_projects, combining path with manifest metadata.
#[derive(Debug, Serialize)]
struct ProjectInfo {
    /// Relative path from workspace root (empty string for root project)
    path: String,
    /// Metadata extracted from package manifests
    #[serde(flatten)]
    metadata: ProjectMetadata,
}

#[tool_handler]
impl ServerHandler for CodeIndexServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Code index providing full-text search and structural navigation over indexed codebases.\n\n\
                 **Primary search tool:**\n\
                 - `search`: Unified FTS across symbols (functions, classes, methods), files, and texts (docstrings, comments). \
                 Filter by scope, kind, path (glob), project. Returns BM25-ranked results with code snippets.\n\n\
                 **Structural navigation:**\n\
                 - `get_file_symbols`: All symbols in a file, ordered by line number.\n\
                 - `get_symbol_children`: Direct children of a symbol (e.g., methods of a class).\n\
                 - `get_imports`: Import statements in a file.\n\n\
                 **Reference tracking:**\n\
                 - `get_callers`: Find all places that call/reference a symbol.\n\
                 - `get_callees`: Find all symbols that a function/method calls.\n\n\
                 **Utilities:**\n\
                 - `list_projects`: List indexed projects with metadata.\n\
                 - `flush_index`: Persist pending changes to .codeindex/ files."
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
