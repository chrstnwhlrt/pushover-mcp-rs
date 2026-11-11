//! The config file: Pushover credentials and defaults.
//!
//! The server reads the file on every tool call, so a token added after the
//! first start is used without restarting the MCP client.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Configuration for the Pushover MCP server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Pushover application API token (required)
    #[serde(default)]
    pub token: String,
    /// User or group key the notifications go to (required)
    #[serde(default)]
    pub default_user_key: Option<String>,
    /// Device (or comma-separated devices) used when a call names none
    #[serde(default)]
    pub default_device: Option<String>,
    /// Key for end-to-end encryption of the notifications, 64 hexadecimal
    /// characters (optional)
    #[serde(default)]
    pub encryption_key: Option<String>,
}

/// `<config dir>/pushover-mcp-rs/config.json`, the config dir being
/// `$XDG_CONFIG_HOME` or `~/.config` on Linux, `~/Library/Application Support`
/// on macOS and `%APPDATA%` on Windows.
pub fn default_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("Could not determine the config directory")?;
    Ok(config_dir.join(env!("CARGO_PKG_NAME")).join("config.json"))
}

/// Writes an empty config file to fill in, unless the file exists.
///
/// Returns whether a file was written. The file holds the API token, so on
/// Unix only its owner may read it.
pub fn create_template_if_missing(path: &Path) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Could not create {}", parent.display()))?;
    }

    let mut content = serde_json::to_string_pretty(&Config::default())
        .context("Could not serialize the config template")?;
    content.push('\n');

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = match options.open(path) {
        Ok(file) => file,
        // Created between the check above and now, by another server start
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => {
            return Err(e).with_context(|| format!("Could not create {}", path.display()));
        }
    };
    file.write_all(content.as_bytes())
        .with_context(|| format!("Could not write {}", path.display()))?;
    Ok(true)
}

/// Reads and normalizes the config file.
pub fn load(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Could not read the config file {}", path.display()))?;
    let mut config: Config = serde_json::from_str(&content)
        .with_context(|| format!("The config file {} is not valid", path.display()))?;
    config.normalize();
    Ok(config)
}

impl Config {
    /// Trims all values; an optional value that is empty then counts as unset.
    /// Surrounding whitespace in a token or key makes every API call fail.
    fn normalize(&mut self) {
        let token = self.token.trim();
        if token.len() != self.token.len() {
            self.token = token.to_owned();
        }
        self.default_user_key = trim_to_option(self.default_user_key.take());
        self.default_device = trim_to_option(self.default_device.take());
        self.encryption_key = trim_to_option(self.encryption_key.take());
    }
}

/// Trims a string; an empty result becomes `None`.
pub fn trim_to_option(value: Option<String>) -> Option<String> {
    let value = value?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else if trimmed.len() == value.len() {
        Some(value)
    } else {
        Some(trimmed.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_to_option_drops_blank_values() {
        assert_eq!(trim_to_option(None), None);
        assert_eq!(trim_to_option(Some(String::new())), None);
        assert_eq!(trim_to_option(Some(" \t\n".into())), None);
        assert_eq!(
            trim_to_option(Some(" iphone ".into())),
            Some("iphone".into())
        );
        assert_eq!(trim_to_option(Some("iphone".into())), Some("iphone".into()));
    }

    #[test]
    fn load_trims_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"token": " abc \n", "default_user_key": "  ", "default_device": " pixel ",
                "encryption_key": " 0a1b \n"}"#,
        )
        .unwrap();
        let config = load(&path).unwrap();
        assert_eq!(
            config,
            Config {
                token: "abc".into(),
                default_user_key: None,
                default_device: Some("pixel".into()),
                encryption_key: Some("0a1b".into()),
            }
        );
    }

    #[test]
    fn load_names_the_file_on_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let error = format!("{:#}", load(&path).unwrap_err());
        assert!(error.contains(&path.display().to_string()), "{error}");

        std::fs::write(&path, "{ not json").unwrap();
        let error = format!("{:#}", load(&path).unwrap_err());
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(error.contains("not valid"), "{error}");
    }

    #[test]
    fn template_is_written_once_and_loads_as_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");
        assert!(create_template_if_missing(&path).unwrap());
        assert!(!create_template_if_missing(&path).unwrap());

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.ends_with("}\n"), "{content}");
        assert_eq!(load(&path).unwrap(), Config::default());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
