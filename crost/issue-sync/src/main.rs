use std::net::SocketAddr;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crost_issue_sync::{
    Config, StateStore, SyncEngine, backfill_all, backfill_project, webhook,
};

#[derive(Debug, Parser)]
#[command(name = "crost-issue-sync", about = "Crost GitHub Issues ↔ Macro task sync")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run webhook ingress HTTP server.
    Serve {
        #[arg(long, env = "CROST_ISSUE_SYNC_CONFIG")]
        config: Option<String>,
    },
    /// Import existing open GitHub issues into Macro tasks.
    Backfill {
        #[arg(long, env = "CROST_ISSUE_SYNC_CONFIG")]
        config: Option<String>,
        /// Limit backfill to one project id (default: all configured projects).
        #[arg(long)]
        project_id: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("crost_issue_sync=info".parse()?))
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Serve { config } => serve(load_config(config)?).await?,
        Command::Backfill {
            config,
            project_id,
        } => backfill(load_config(config)?, project_id.as_deref()).await?,
    }
    Ok(())
}

fn load_config(path: Option<String>) -> anyhow::Result<Config> {
    if let Some(path) = path {
        Ok(Config::from_json_file(path)?)
    } else {
        Ok(Config::from_env()?)
    }
}

async fn serve(cfg: Config) -> anyhow::Result<()> {
    let store = StateStore::open(&cfg.state_db_path)?;
    let engine = Arc::new(SyncEngine::new(cfg.clone(), store)?);
    let state = webhook::WebhookState {
        engine,
        github_secret: cfg.github_webhook_secret.clone(),
        macro_secret: cfg.macro_webhook_secret.clone(),
    };
    let app = webhook::router(state);
    let addr: SocketAddr = cfg.listen_addr.parse()?;
    tracing::info!(%addr, "issue sync listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn backfill(cfg: Config, project_id: Option<&str>) -> anyhow::Result<()> {
    let store = StateStore::open(&cfg.state_db_path)?;
    let engine = SyncEngine::new(cfg, store)?;
    let count = if let Some(project_id) = project_id {
        backfill_project(&engine, project_id).await?
    } else {
        backfill_all(&engine).await?
    };
    tracing::info!(imported = count, "backfill complete");
    println!("imported {count} issue(s)");
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown requested");
}
