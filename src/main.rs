use std::io::IsTerminal;
use std::path::Path;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "codeix",
    about = "Portable, composable code index\n\nhttps://codeix.dev"
)]
struct Cli {
    /// Root directory (defaults to current dir)
    #[arg(short = 'r', long = "root", global = true, default_value = ".")]
    root: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Build the .codeindex/ for discovered projects
    Build,
    /// Start the MCP server (default when stdin is piped)
    Serve {
        /// Disable file watching
        #[arg(long)]
        no_watch: bool,
    },
    /// Interactive query REPL (default when in a terminal)
    ///
    /// Run 'codeix query help' to see available commands.
    Query {
        /// Disable file watching
        #[arg(long)]
        no_watch: bool,
        /// Command to execute (if omitted, starts REPL)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let root = Path::new(&cli.root);

    let command = cli.command.unwrap_or_else(|| {
        if std::io::stdin().is_terminal() {
            // Interactive terminal: default to query REPL
            Commands::Query {
                no_watch: false,
                command: vec![],
            }
        } else {
            // Piped stdin (e.g. MCP client): default to serve
            Commands::Serve { no_watch: false }
        }
    });

    match command {
        Commands::Build => {
            codeix::cli::build::run(root)?;
        }
        Commands::Serve { no_watch } => {
            codeix::cli::serve::run(root, !no_watch)?;
        }
        Commands::Query { no_watch, command } => {
            codeix::cli::query::run(root, !no_watch, command)?;
        }
    }

    Ok(())
}
