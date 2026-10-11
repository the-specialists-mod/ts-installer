//! The launcher's saved state: the finished install and the launch options.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::install::Installation;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub installation: Option<Installation>,
    pub launch: LaunchOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LaunchOptions {
    pub windowed: bool,
    /// Opens the developer console (`-console`).
    pub console: bool,
    /// Extra command-line arguments, split on whitespace.
    pub extra_args: String,
    /// Wine executable for Windows builds on Linux.
    pub wine: String,
    /// `WINEPREFIX`; empty uses Wine's default.
    pub wine_prefix: String,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self { windowed: false, console: true, extra_args: String::new(), wine: "wine".into(), wine_prefix: String::new() }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("ts-launcher").join("config.json")
    }

    /// Loads the saved config, or the default when there is none or it is unreadable.
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).map_err(|e| log::warn!("ignoring bad config: {e}")).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self)?).with_context(|| format!("write {}", path.display()))
    }
}
