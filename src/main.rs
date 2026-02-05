use std::io::IsTerminal;
use std::path::Path;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "codeix", about = "Portable, composable code index")]
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
    /// Start the MCP server (default when no subcommand given)
    Serve {
        /// Root directory (defaults to current dir)
        #[arg(default_value = ".")]
        path: String,
        /// Disable file watching
        #[arg(long)]
        no_watch: bool,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    let command = cli.command.unwrap_or_else(|| {
        if std::io::stdin().is_terminal() {
            // Interactive: no subcommand given, print help and exit
            Cli::parse_from(["codeix", "--help"]);
            unreachable!()
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
    }

    Ok(())
}
