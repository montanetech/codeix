//! Interactive REPL for querying the code index.
//!
//! Provides the same API as the MCP server but through an interactive command line.

use std::path::Path;
use std::sync::{Arc, mpsc};

use anyhow::{Context, Result};
use clap::Parser;
use clap_repl::{ClapEditor, ReadCommandOutput};
use rmcp::handler::server::wrapper::Parameters;

use crate::cli::build::build_index_to_db;
use crate::mount::MountedEvent;
use crate::mount::handler::{flush_mount_to_disk, run_event_loop};
use crate::server::mcp::{
    CodeIndexServer, GetCalleesParams, GetCallersParams, GetFileSymbolsParams, GetImportsParams,
    GetSymbolChildrenParams, SearchFilesParams, SearchReferencesParams, SearchSymbolsParams,
    SearchTextsParams, extract_result_text,
};

/// REPL commands matching the MCP tools.
/// NOTE: When adding/removing/renaming tools, also update src/server/mcp.rs (tool implementations)
#[derive(Debug, Parser)]
#[command(name = "")]
pub enum QueryCommand {
    /// Search or list symbols
    SearchSymbols(#[command(flatten)] SearchSymbolsParams),
    /// Search files by path
    SearchFiles(#[command(flatten)] SearchFilesParams),
    /// Search text entries (docstrings, comments)
    SearchTexts(#[command(flatten)] SearchTextsParams),
    /// Get all symbols in a file
    GetFileSymbols(#[command(flatten)] GetFileSymbolsParams),
    /// Get children of a symbol
    GetSymbolChildren(#[command(flatten)] GetSymbolChildrenParams),
    /// Get imports for a file
    GetImports(#[command(flatten)] GetImportsParams),
    /// List all indexed projects
    ListProjects,
    /// Find callers of a symbol
    GetCallers(#[command(flatten)] GetCallersParams),
    /// Find what a symbol calls
    GetCallees(#[command(flatten)] GetCalleesParams),
    /// Search references
    SearchReferences(#[command(flatten)] SearchReferencesParams),
    /// Flush index to disk
    FlushIndex,
    /// Exit the REPL
    #[command(alias = "quit")]
    Exit,
}

/// Run the interactive query REPL or execute a single command.
///
/// If `command` is empty, starts the interactive REPL.
/// Otherwise, executes the command and exits.
pub fn run(root: &Path, watch: bool, command: Vec<String>) -> Result<()> {
    // If watch mode: create channel BEFORE building
    // This way directories are watched during the single walk (no second walk needed)
    let (tx, rx): (
        Option<mpsc::Sender<MountedEvent>>,
        Option<mpsc::Receiver<MountedEvent>>,
    ) = if watch {
        let (tx, rx) = mpsc::channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    // Build index with FTS enabled (loads from .codeindex/ if exists, otherwise parses files)
    let (mount_table, db) =
        build_index_to_db(root, true, true, tx.clone()).context("failed to build/load index")?;

    // Flush any dirty mounts to disk
    {
        let mt = mount_table
            .lock()
            .map_err(|e| anyhow::anyhow!("mount table lock poisoned: {e}"))?;
        for (mount_root, mount) in mt.iter() {
            if mount.dirty {
                flush_mount_to_disk(mount_root, &mt, &db)
                    .with_context(|| format!("failed to flush {}", mount_root.display()))?;
            }
        }
    }

    // Spawn event loop if watching
    if let (Some(tx), Some(rx)) = (tx, rx) {
        let mount_table_clone = Arc::clone(&mount_table);
        let db_clone = Arc::clone(&db);

        std::thread::spawn(move || {
            if let Err(e) = run_event_loop(rx, tx, mount_table_clone, db_clone) {
                tracing::error!("event loop error: {}", e);
            }
        });
    }

    // Create the tokio runtime for async tool calls
    let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;

    // Create the MCP server (reusing its tool implementations)
    let server = CodeIndexServer::new(Arc::clone(&db), Arc::clone(&mount_table));

    // Helper to execute a command
    let execute_command = |cmd: QueryCommand| {
        rt.block_on(async {
            let result = match cmd {
                QueryCommand::SearchSymbols(params) => {
                    server.search_symbols(Parameters(params)).await
                }
                QueryCommand::SearchFiles(params) => server.search_files(Parameters(params)).await,
                QueryCommand::SearchTexts(params) => server.search_texts(Parameters(params)).await,
                QueryCommand::GetFileSymbols(params) => {
                    server.get_file_symbols(Parameters(params)).await
                }
                QueryCommand::GetSymbolChildren(params) => {
                    server.get_symbol_children(Parameters(params)).await
                }
                QueryCommand::GetImports(params) => server.get_imports(Parameters(params)).await,
                QueryCommand::ListProjects => server.list_projects().await,
                QueryCommand::GetCallers(params) => server.get_callers(Parameters(params)).await,
                QueryCommand::GetCallees(params) => server.get_callees(Parameters(params)).await,
                QueryCommand::SearchReferences(params) => {
                    server.search_references(Parameters(params)).await
                }
                QueryCommand::FlushIndex => server.flush_index().await,
                QueryCommand::Exit => unreachable!(),
            };

            match result {
                Ok(r) => println!("{}", extract_result_text(&r)),
                Err(e) => eprintln!("Error: {}", e.message),
            }
        });
    };

    // Single command mode: parse and execute, then exit
    if !command.is_empty() {
        // Prepend empty string for clap (it expects argv[0] to be program name)
        let mut args = vec!["".to_string()];
        args.extend(command);

        match QueryCommand::try_parse_from(&args) {
            Ok(cmd) => {
                if matches!(cmd, QueryCommand::Exit) {
                    return Ok(());
                }
                execute_command(cmd);
            }
            Err(e) => {
                e.print().ok();
            }
        }
        return Ok(());
    }

    // Interactive REPL mode
    println!("codeix query REPL — type 'help' for commands, 'exit' to quit");
    let mut rl = ClapEditor::<QueryCommand>::builder().build();
    loop {
        match rl.read_command() {
            ReadCommandOutput::Command(cmd) => {
                // Handle exit command
                if matches!(cmd, QueryCommand::Exit) {
                    break;
                }
                execute_command(cmd);
            }
            ReadCommandOutput::EmptyLine | ReadCommandOutput::CtrlC => continue,
            ReadCommandOutput::CtrlD => break,
            ReadCommandOutput::ClapError(e) => {
                e.print().ok();
            }
            ReadCommandOutput::ShlexError => {
                eprintln!("Error: Invalid input (check quotes)");
            }
            ReadCommandOutput::ReedlineError(e) => {
                eprintln!("Error: {}", e);
                break;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_command_parse() {
        // Test list-projects (no args)
        let cmd = QueryCommand::try_parse_from(["", "list-projects"]).unwrap();
        assert!(matches!(cmd, QueryCommand::ListProjects));

        // Test flush-index (no args)
        let cmd = QueryCommand::try_parse_from(["", "flush-index"]).unwrap();
        assert!(matches!(cmd, QueryCommand::FlushIndex));

        // Test search-symbols with query (positional arg)
        let cmd = QueryCommand::try_parse_from(["", "search-symbols", "foo"]).unwrap();
        if let QueryCommand::SearchSymbols(params) = cmd {
            assert_eq!(params.query, Some("foo".to_string()));
        } else {
            panic!("Expected SearchSymbols");
        }

        // Test search-symbols with options
        let cmd =
            QueryCommand::try_parse_from(["", "search-symbols", "--kind", "function"]).unwrap();
        if let QueryCommand::SearchSymbols(params) = cmd {
            assert_eq!(params.kind, Some("function".to_string()));
        } else {
            panic!("Expected SearchSymbols");
        }

        // Test get-file-symbols with file (positional required arg)
        let cmd = QueryCommand::try_parse_from(["", "get-file-symbols", "src/main.rs"]).unwrap();
        if let QueryCommand::GetFileSymbols(params) = cmd {
            assert_eq!(params.file, "src/main.rs");
        } else {
            panic!("Expected GetFileSymbols");
        }

        // Test get-callers with name (positional required arg)
        let cmd = QueryCommand::try_parse_from(["", "get-callers", "my_function"]).unwrap();
        if let QueryCommand::GetCallers(params) = cmd {
            assert_eq!(params.name, "my_function");
        } else {
            panic!("Expected GetCallers");
        }

        // Test exit command
        let cmd = QueryCommand::try_parse_from(["", "exit"]).unwrap();
        assert!(matches!(cmd, QueryCommand::Exit));

        // Test quit alias
        let cmd = QueryCommand::try_parse_from(["", "quit"]).unwrap();
        assert!(matches!(cmd, QueryCommand::Exit));
    }
}
