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
use serde::Deserialize;

use super::db::SearchDb;

// Parameter structs for each tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchSymbolsParams {
    /// Search query for symbol names
    pub query: String,
    /// Filter by symbol kind (e.g. function, struct, class, method)
    pub kind: Option<String>,
    /// Filter by file path
    pub file: Option<String>,
    /// Filter by project (relative path from workspace root, e.g. "libs/utils")
    pub project: Option<String>,
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
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSymbolChildrenParams {
    /// File path containing the parent symbol
    pub file: String,
    /// Name of the parent symbol
    pub parent: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetImportsParams {
    /// File path to get imports for
    pub file: String,
}

/// MCP server exposing code-index query tools.
///
/// `SearchDb` wraps a `rusqlite::Connection` which is not `Sync`, so we protect
/// it with a `Mutex` to satisfy rmcp's `Send + Sync` requirements.
#[derive(Clone)]
pub struct CodeIndexServer {
    db: Arc<Mutex<SearchDb>>,
    tool_router: ToolRouter<Self>,
}

impl CodeIndexServer {
    pub fn new(db: Arc<Mutex<SearchDb>>) -> Self {
        Self {
            db,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl CodeIndexServer {
    /// Search symbols by name using FTS5 full-text search (BM25-ranked).
    #[tool(description = "Search symbols by name with optional kind/file filters")]
    async fn search_symbols(
        &self,
        Parameters(params): Parameters<SearchSymbolsParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .search_symbols(
                &params.query,
                params.kind.as_deref(),
                params.file.as_deref(),
                params.project.as_deref(),
            )
            .map_err(|e| McpError::internal_error(format!("search_symbols failed: {e}"), None))?;

        let json = serde_json::to_string_pretty(&results)
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
    #[tool(description = "Get all symbols in a file, ordered by line number")]
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

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Get direct children of a symbol (e.g. methods of a class).
    #[tool(description = "Get direct children of a symbol (e.g. methods of a class)")]
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

        let json = serde_json::to_string_pretty(&results)
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

    /// List all indexed projects.
    #[tool(description = "List all indexed projects")]
    async fn list_projects(&self) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .list_projects()
            .map_err(|e| McpError::internal_error(format!("list_projects failed: {e}"), None))?;

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }
}

#[tool_handler]
impl ServerHandler for CodeIndexServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Code index query tools. Use list_projects to see indexed projects. \
                 Use search_symbols, search_files, and search_texts for full-text search \
                 (optionally filtered by project). Use get_file_symbols, get_symbol_children, \
                 and get_imports for structural lookups."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

/// Start the MCP server over stdio with the given search database.
pub async fn start_server(db: Arc<Mutex<SearchDb>>) -> Result<()> {
    let server = CodeIndexServer::new(db);
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
