use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub telegram_bot_token: String,
    pub server_ip: String,
    pub domain: String,
    pub rathole_api: String,
    pub db_path: String,
    pub port_min: u16,
    pub port_max: u16,
    pub log_level: String,
    pub idle_timeout_secs: u64,
    /// Address for the webhook HTTP server (e.g. "0.0.0.0:8090")
    pub webhook_listen_addr: Option<String>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read config file: {}", path.display()))?;
        let config: Config = toml::from_str(&text)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
        Ok(config)
    }
}
