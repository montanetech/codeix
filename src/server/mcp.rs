use std::sync::{Arc, Mutex};

use anyhow::Result;
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

use super::db::SearchDb;
use super::snippet::SnippetExtractor;
use crate::index::format::SymbolEntry;
use crate::mount::MountTable;
use crate::utils::manifest::{self, ProjectMetadata};

// Parameter structs for each tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchSymbolsParams {
    /// Search query for symbol names. If omitted, lists all symbols matching the filters.
    pub query: Option<String>,
    /// Filter by symbol kind (e.g. function, struct, class, method)
    pub kind: Option<String>,
    /// Filter by file path. Supports glob patterns with * (e.g. "src/utils/*.py")
    pub file: Option<String>,
    /// Filter by project (relative path from workspace root, e.g. "libs/utils")
    pub project: Option<String>,
    /// Maximum number of results to return (default: 100)
    pub limit: Option<u32>,
    /// Number of results to skip for pagination (default: 0)
    pub offset: Option<u32>,
    /// Number of code snippet lines: 0=none, -1=all, N=N lines (default: 10)
    pub snippet_lines: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchFilesParams {
    /// Search query for file paths
    pub query: String,
    /// Filter by language (e.g. python, rust, javascript)
    pub lang: Option<String>,
    /// Filter by project (relative path from workspace root, e.g. "libs/utils")
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchTextsParams {
    /// Search query for text content
    pub query: String,
    /// Filter by text kind (e.g. docstring, comment)
    pub kind: Option<String>,
    /// Filter by file path
    pub file: Option<String>,
    /// Filter by project (relative path from workspace root, e.g. "libs/utils")
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetFileSymbolsParams {
    /// File path to get symbols for
    pub file: String,
    /// Number of code snippet lines: 0=none, -1=all, N=N lines (default: 10)
    pub snippet_lines: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSymbolChildrenParams {
    /// File path containing the parent symbol
    pub file: String,
    /// Name of the parent symbol
    pub parent: String,
    /// Number of code snippet lines: 0=none, -1=all, N=N lines (default: 10)
    pub snippet_lines: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetImportsParams {
    /// File path to get imports for
    pub file: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCallersParams {
    /// Symbol name to find callers for (e.g. "my_function", "MyClass.method")
    pub name: String,
    /// Filter by reference kind (e.g. "call", "import", "type_annotation")
    pub kind: Option<String>,
    /// Filter by project (relative path from workspace root, e.g. "libs/utils")
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCalleesParams {
    /// Symbol name to find callees for (e.g. "my_function", "MyClass.method")
    pub caller: String,
    /// Filter by reference kind (e.g. "call", "import", "type_annotation")
    pub kind: Option<String>,
    /// Filter by project (relative path from workspace root, e.g. "libs/utils")
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchReferencesParams {
    /// Search query for reference names
    pub query: String,
    /// Filter by reference kind (e.g. "call", "import", "type_annotation")
    pub kind: Option<String>,
    /// Filter by file path
    pub file: Option<String>,
    /// Filter by project (relative path from workspace root, e.g. "libs/utils")
    pub project: Option<String>,
    /// Maximum number of results to return (default: 100)
    pub limit: Option<u32>,
    /// Number of results to skip for pagination (default: 0)
    pub offset: Option<u32>,
}

/// Response wrapper for SymbolEntry with optional snippet.
#[derive(Debug, Serialize)]
struct SymbolWithSnippet {
    #[serde(flatten)]
    symbol: SymbolEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
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
    /// Search or list symbols. With query: FTS5 search (BM25-ranked). Without query: list all matching filters.
    #[tool(
        description = "Search or list symbols. Provide query for full-text search, or omit to list all symbols matching filters. File filter supports glob patterns (e.g. 'src/*.py'). Returns code snippets by default."
    )]
    async fn search_symbols(
        &self,
        Parameters(params): Parameters<SearchSymbolsParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let limit = params.limit.unwrap_or(100);
        let offset = params.offset.unwrap_or(0);
        let results = db
            .search_symbols(
                params.query.as_deref(),
                params.kind.as_deref(),
                params.file.as_deref(),
                params.project.as_deref(),
                limit,
                offset,
            )
            .map_err(|e| McpError::internal_error(format!("search_symbols failed: {e}"), None))?;

        drop(db); // Release lock before file I/O

        let snippet_lines = params.snippet_lines.unwrap_or(10);
        let enriched = self.enrich_with_snippets(results, snippet_lines);

        let json = serde_json::to_string_pretty(&enriched)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Search files by path using FTS5 full-text search (BM25-ranked).
    #[tool(description = "Search files by path with optional language filter")]
    async fn search_files(
        &self,
        Parameters(params): Parameters<SearchFilesParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .search_files(
                &params.query,
                params.lang.as_deref(),
                params.project.as_deref(),
            )
            .map_err(|e| McpError::internal_error(format!("search_files failed: {e}"), None))?;

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Search text entries (docstrings, comments) using FTS5 full-text search (BM25-ranked).
    #[tool(
        description = "Search text entries (docstrings, comments) with optional kind/file filters"
    )]
    async fn search_texts(
        &self,
        Parameters(params): Parameters<SearchTextsParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .search_text(
                &params.query,
                params.kind.as_deref(),
                params.file.as_deref(),
                params.project.as_deref(),
            )
            .map_err(|e| McpError::internal_error(format!("search_texts failed: {e}"), None))?;

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Get all symbols in a file, ordered by line number.
    #[tool(
        description = "Get all symbols in a file, ordered by line number. Returns code snippets by default."
    )]
    async fn get_file_symbols(
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
    async fn get_symbol_children(
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
    async fn get_imports(
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
    async fn list_projects(&self) -> Result<CallToolResult, McpError> {
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
    async fn get_callers(
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
    async fn get_callees(
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

    /// Search references by name using FTS5.
    #[tool(
        description = "Search for symbol references (calls, imports, type annotations) using full-text search. Filter by kind, file, or project."
    )]
    async fn search_references(
        &self,
        Parameters(params): Parameters<SearchReferencesParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let limit = params.limit.unwrap_or(100);
        let offset = params.offset.unwrap_or(0);
        let results = db
            .search_references(
                &params.query,
                params.kind.as_deref(),
                params.file.as_deref(),
                params.project.as_deref(),
                limit,
                offset,
            )
            .map_err(|e| {
                McpError::internal_error(format!("search_references failed: {e}"), None)
            })?;

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
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
                "Code index query tools. Use list_projects to see indexed projects. \
                 Use search_symbols, search_files, and search_texts for full-text search \
                 (optionally filtered by project). Use get_file_symbols, get_symbol_children, \
                 and get_imports for structural lookups. Use get_callers to find who calls \
                 a symbol, get_callees to find what a symbol calls, and search_references \
                 for full-text search across all references."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
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
