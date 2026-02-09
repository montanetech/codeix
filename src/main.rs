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
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Build the .codeindex/ for discovered projects
    Build {
        /// Root directory to scan (defaults to current dir)
        #[arg(default_value = ".")]
        path: String,
    },
    /// Start the MCP server (default when stdin is piped)
    Serve {
        /// Root directory (defaults to current dir)
        #[arg(default_value = ".")]
        path: String,
        /// Disable file watching
        #[arg(long)]
        no_watch: bool,
    },
    /// Interactive query REPL (default when in a terminal)
    ///
    /// Run 'codeix query help' to see available commands.
    Query {
        /// Root directory (defaults to current dir)
        #[arg(short = 'C', long, default_value = ".")]
        path: String,
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

    let command = cli.command.unwrap_or_else(|| {
        if std::io::stdin().is_terminal() {
            // Interactive terminal: default to query REPL
            Commands::Query {
                path: ".".into(),
                no_watch: false,
                command: vec![],
            }
        } else {
            // Piped stdin (e.g. MCP client): default to serve
            Commands::Serve {
                path: ".".into(),
                no_watch: false,
            }
        }
    });

    match command {
        Commands::Build { path } => {
            codeix::cli::build::run(Path::new(&path))?;
        }
        Commands::Serve { path, no_watch } => {
            codeix::cli::serve::run(Path::new(&path), !no_watch)?;
        }
        Commands::Query {
            path,
            no_watch,
            command,
        } => {
            codeix::cli::query::run(Path::new(&path), !no_watch, command)?;
        }
    }

    Ok(())
}
