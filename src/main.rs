//! MCP server for sending push notifications through the Pushover API.
//!
//! It speaks JSON-RPC 2.0 over stdin/stdout; logs go to stderr.

mod attachment;
mod config;
mod model;
mod pushover;
mod schema;
mod server;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(
    name = env!("CARGO_PKG_NAME"),
    version,
    about,
    after_help = "The default config file is <config dir>/pushover-mcp-rs/config.json, the config \
                  dir being $XDG_CONFIG_HOME or ~/.config on Linux, ~/Library/Application Support \
                  on macOS and %APPDATA% on Windows. The first start writes an empty one to fill in."
)]
struct Cli {
    /// Config file to use instead of the default, e.g. for a second Pushover account
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Sender of every notification, e.g. Otomo for a voice assistant; the model is then
    /// not asked for one. Without it the model names itself and its session.
    #[arg(long, value_name = "NAME")]
    sender: Option<String>,

    /// Loudest priority this client may send: silent, low, normal, high or emergency (or -2 to
    /// 2). high rings through quiet hours, emergency repeats until acknowledged, so both need
    /// to be allowed here; a louder request goes out with this priority and a note.
    #[arg(long, value_name = "LEVEL", default_value = "normal", value_parser = priority_level)]
    max_priority: model::Priority,

    /// Base URL of the Pushover API (only to be changed for tests)
    #[arg(long, value_name = "URL", default_value = pushover::DEFAULT_API_URL)]
    api_url: String,
}

fn priority_level(level: &str) -> Result<model::Priority, String> {
    model::Priority::parse(level)
        .ok_or_else(|| "expected silent, low, normal, high or emergency (or -2 to 2)".to_owned())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // stdout carries the MCP protocol, so logs go to stderr.
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("warn,pushover_mcp_rs=info")),
        )
        .with(fmt::layer().with_writer(std::io::stderr).with_ansi(false))
        .init();

    let sender = match cli.sender.as_deref().map(str::trim) {
        Some("") => anyhow::bail!("--sender must not be empty"),
        Some(sender) if sender.chars().count() > model::MAX_SENDER_CHARS => anyhow::bail!(
            "--sender has more than {} characters",
            model::MAX_SENDER_CHARS
        ),
        sender => sender.map(str::to_owned),
    };

    // Absolute, so that messages name the file wherever the client runs us.
    let config_path = match cli.config {
        Some(path) => std::path::absolute(&path)
            .with_context(|| format!("Invalid config path {}", path.display()))?,
        None => config::default_path()?,
    };
    let created = config::create_template_if_missing(&config_path).unwrap_or_else(|e| {
        tracing::warn!("{e:#}");
        false
    });
    if created {
        tracing::warn!(
            "Created the config file {}; add the Pushover token and user key there",
            config_path.display()
        );
    } else {
        // Tools read the file on every call; this only reports problems early.
        match config::load(&config_path) {
            Ok(config) if config.token.is_empty() || config.default_user_key.is_none() => {
                tracing::warn!(
                    "The Pushover token or user key is missing in {}",
                    config_path.display()
                );
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("{e:#}"),
        }
    }

    let user_agent = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
    let api = pushover::PushoverClient::new(&cli.api_url, user_agent)?;

    tracing::info!(
        "Starting {} v{}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );
    let service = server::PushoverServer::new(config_path, api, sender, cli.max_priority)
        .serve(stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
