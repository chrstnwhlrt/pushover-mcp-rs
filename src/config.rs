use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Configuration for the Pushover MCP server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Pushover application API token (required)
    #[serde(default)]
    pub token: String,
    /// Default user/group key (optional, can be overridden per message)
    #[serde(default)]
    pub default_user_key: Option<String>,
    /// Default device name (optional)
    #[serde(default)]
    pub default_device: Option<String>,
}

/// Get the path to the config file.
pub fn get_config_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .context("Could not determine config directory")?;

    Ok(config_dir.join("pushover-mcp-rs").join("config.json"))
}

/// Load the configuration from the config file.
///
/// Creates a default config file if it doesn't exist.
pub fn load_config() -> Result<Config> {
    let config_path = get_config_path()?;

    if !config_path.exists() {
        // Create parent directory if needed
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)
                .context("Failed to create config directory")?;
        }

        // Create default config file
        let default_config = Config::default();
        let mut config_json = serde_json::to_string_pretty(&default_config)
            .context("Failed to serialize default config")?;
        config_json.push('\n'); // POSIX: text files should end with newline

        std::fs::write(&config_path, config_json)
            .context("Failed to write default config file")?;

        tracing::info!("Created default config file at: {}", config_path.display());

        return Ok(default_config);
    }

    // Read existing config
    let config_content = std::fs::read_to_string(&config_path)
        .context("Failed to read config file")?;

    let mut config: Config = serde_json::from_str(&config_content)
        .context("Failed to parse config file")?;

    // Normalize config: trim whitespace from all string fields
    normalize_config(&mut config);

    Ok(config)
}

/// Trim whitespace from config fields to prevent subtle bugs.
fn normalize_config(config: &mut Config) {
    // Trim token - whitespace would cause API failures
    let trimmed_token = config.token.trim();
    if trimmed_token.len() != config.token.len() {
        config.token = trimmed_token.to_string();
    }

    // Trim optional fields, set to None if empty after trimming
    config.default_user_key = config.default_user_key.take().and_then(|s| {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else if trimmed.len() == s.len() {
            Some(s)
        } else {
            Some(trimmed.to_string())
        }
    });

    config.default_device = config.default_device.take().and_then(|s| {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else if trimmed.len() == s.len() {
            Some(s)
        } else {
            Some(trimmed.to_string())
        }
    });
}

/// Check if the config has a valid token set.
#[inline]
pub fn is_token_configured(config: &Config) -> bool {
    !config.token.is_empty()
}
