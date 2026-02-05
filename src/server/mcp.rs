use std::sync::{Arc, Mutex};

use anyhow::Result;
use rmcp::{
    Error as McpError, ServerHandler, ServiceExt,
    model::{
        CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    schemars, tool,
    transport::stdio,
};

use super::db::SearchDb;

/// MCP server exposing code-index query tools.
///
/// `SearchDb` wraps a `rusqlite::Connection` which is not `Sync`, so we protect
/// it with a `Mutex` to satisfy rmcp's `Send + Sync` requirements.
#[derive(Clone)]
pub struct CodeIndexServer {
    db: Arc<Mutex<SearchDb>>,
}

#[tool(tool_box)]
impl CodeIndexServer {
    /// Search symbols by name using FTS5 full-text search (BM25-ranked).
    #[tool(description = "Search symbols by name with optional kind/file filters")]
    fn search_symbols(
        &self,
        #[tool(param)]
        #[schemars(description = "Search query for symbol names")]
        query: String,
        #[tool(param)]
        #[schemars(description = "Filter by symbol kind (e.g. function, struct, class, method)")]
        kind: Option<String>,
        #[tool(param)]
        #[schemars(description = "Filter by file path")]
        file: Option<String>,
    ) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .search_symbols(&query, kind.as_deref(), file.as_deref())
            .map_err(|e| McpError::internal_error(format!("search_symbols failed: {e}"), None))?;

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Search files by path using FTS5 full-text search (BM25-ranked).
    #[tool(description = "Search files by path with optional language filter")]
    fn search_files(
        &self,
        #[tool(param)]
        #[schemars(description = "Search query for file paths")]
        query: String,
        #[tool(param)]
        #[schemars(description = "Filter by language (e.g. python, rust, javascript)")]
        lang: Option<String>,
    ) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .search_files(&query, lang.as_deref())
            .map_err(|e| McpError::internal_error(format!("search_files failed: {e}"), None))?;

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Search text entries (docstrings, comments) using FTS5 full-text search (BM25-ranked).
    #[tool(
        description = "Search text entries (docstrings, comments) with optional kind/file filters"
    )]
    fn search_texts(
        &self,
        #[tool(param)]
        #[schemars(description = "Search query for text content")]
        query: String,
        #[tool(param)]
        #[schemars(description = "Filter by text kind (e.g. docstring, comment)")]
        kind: Option<String>,
        #[tool(param)]
        #[schemars(description = "Filter by file path")]
        file: Option<String>,
    ) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .search_text(&query, kind.as_deref(), file.as_deref())
            .map_err(|e| McpError::internal_error(format!("search_texts failed: {e}"), None))?;

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Get all symbols in a file, ordered by line number.
    #[tool(description = "Get all symbols in a file, ordered by line number")]
    fn get_file_symbols(
        &self,
        #[tool(param)]
        #[schemars(description = "File path to get symbols for")]
        file: String,
    ) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .get_file_symbols(&file)
            .map_err(|e| McpError::internal_error(format!("get_file_symbols failed: {e}"), None))?;

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Get direct children of a symbol (e.g. methods of a class).
    #[tool(description = "Get direct children of a symbol (e.g. methods of a class)")]
    fn get_symbol_children(
        &self,
        #[tool(param)]
        #[schemars(description = "File path containing the parent symbol")]
        file: String,
        #[tool(param)]
        #[schemars(description = "Name of the parent symbol")]
        parent: String,
    ) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db.get_symbol_children(&file, &parent).map_err(|e| {
            McpError::internal_error(format!("get_symbol_children failed: {e}"), None)
        })?;

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Get all import statements for a file.
    #[tool(description = "Get all import statements for a file")]
    fn get_imports(
        &self,
        #[tool(param)]
        #[schemars(description = "File path to get imports for")]
        file: String,
    ) -> Result<CallToolResult, McpError> {
        let db = self
            .db
            .lock()
            .map_err(|e| McpError::internal_error(format!("db lock poisoned: {e}"), None))?;
        let results = db
            .get_imports(&file)
            .map_err(|e| McpError::internal_error(format!("get_imports failed: {e}"), None))?;

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("serialization failed: {e}"), None))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }
}

#[tool(tool_box)]
impl ServerHandler for CodeIndexServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "codeix".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            instructions: Some(
                "Code index query tools. Use search_symbols, search_files, and search_texts \
                 for full-text search. Use get_file_symbols, get_symbol_children, and \
                 get_imports for structural lookups."
                    .into(),
            ),
        }
    }
}

/// Start the MCP server over stdio with the given search database.
pub async fn start_server(db: Arc<Mutex<SearchDb>>) -> Result<()> {
    let server = CodeIndexServer { db };
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
