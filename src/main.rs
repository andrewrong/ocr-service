use std::net::SocketAddr;

use anyhow::Context;
use clap::{Parser, Subcommand};
use ocr_service::{config::Config, http};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "ocr-service",
    version,
    about = "Local model-powered OCR service"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the HTTP API server.
    Serve {
        /// TCP port to listen on (overrides OCR_PORT).
        #[arg(long)]
        port: Option<u16>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "ocr_service=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let mut config = Config::from_env()?;

    match cli.command {
        Command::Serve { port } => {
            if let Some(port) = port {
                config.port = port;
            }
            serve(config).await
        }
    }
}

async fn serve(config: Config) -> anyhow::Result<()> {
    let address = SocketAddr::from(([0, 0, 0, 0], config.port));
    let app = http::router(config)?;
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind HTTP server to {address}"))?;

    tracing::info!(%address, "OCR HTTP service listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to install Ctrl+C handler");
    }
}
