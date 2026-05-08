mod config;
mod model;
mod server;

use anyhow::Result;
use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = "MCP server for sending push notifications via Pushover API"
)]
struct Cli {}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI args (handles --version, --help)
    let _cli = Cli::parse();

    // Initialize logging to stderr (stdout is reserved for MCP protocol)
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("pushover_mcp_rs=info")),
        )
        .with(fmt::layer().with_writer(std::io::stderr).with_ansi(false))
        .init();

    // Load or create config
    let config = match config::load_config() {
        Ok(cfg) => {
            if !config::is_token_configured(&cfg) {
                let config_path = config::get_config_path()?;
                tracing::warn!(
                    "No Pushover token configured. Edit the config file at: {}",
                    config_path.display()
                );
            }
            cfg
        }
        Err(e) => {
            tracing::error!("Failed to load config: {}. Using defaults.", e);
            config::Config::default()
        }
    };

    tracing::info!(
        "Starting Pushover MCP server v{}",
        env!("CARGO_PKG_VERSION")
    );

    // Create MCP server
    let server =
        server::PushoverServer::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"), config)
            .await;

    // Connect via stdio transport
    let service = server.serve(stdio()).await?;

    // Wait for service completion
    service.waiting().await?;

    Ok(())
}
