mod cache;
mod event;
mod record;
mod server;
mod store;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use rmcp::ServiceExt;

#[derive(Parser)]
#[command(name = "memory-mcp", about = "Append-only memory for local agents")]
struct Cli {
    /// Data directory (record/ and cache/ live here)
    #[arg(long, env = "MEMORY_DIR", default_value = "./data")]
    dir: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the MCP server on stdio
    Serve,
    /// Add a statement as the owner (trusted immediately)
    Add {
        #[arg(long, default_value = "global")]
        scope: String,
        text: String,
    },
    /// Lift an agent claim into the trusted context
    Lift {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Retract a statement (closes it, nothing is deleted)
    Retract {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Search a scope
    Show {
        #[arg(long, default_value = "global")]
        scope: String,
        #[arg(default_value = "")]
        query: String,
    },
    /// Provenance of one statement
    Explain { id: String },
    /// Rebuild the cache from the record
    Rebuild,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut store = store::Store::open(&cli.dir)?;
    match cli.cmd {
        Cmd::Serve => {
            let service = server::MemoryServer::new(store)
                .serve(rmcp::transport::stdio())
                .await?;
            service.waiting().await?;
        }
        Cmd::Add { scope, text } => {
            let id = store.claim(store::human(), &scope, &text, None)?;
            println!("{}", id);
        }
        Cmd::Lift { id, reason } => {
            store.lift(&id, reason.as_deref())?;
            println!("lifted {}", id);
        }
        Cmd::Retract { id, reason } => {
            store.retract(&id, store::human(), reason.as_deref())?;
            println!("retracted {}", id);
        }
        Cmd::Show { scope, query } => {
            for c in store.recall(&scope, &query, 50)? {
                let mark = if c.trust == "trusted" { "+" } else { "?" };
                println!("{} {} {} [{}] {}", mark, c.id, &c.tx[..10], c.scope, c.text);
            }
        }
        Cmd::Explain { id } => {
            println!("{}", serde_json::to_string_pretty(&store.explain(&id)?)?);
        }
        Cmd::Rebuild => {
            let n = store.rebuild()?;
            println!("replayed {} events", n);
        }
    }
    Ok(())
}
