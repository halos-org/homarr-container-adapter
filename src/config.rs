//! Adapter configuration

use serde::Deserialize;
use std::fs;
use std::path::Path;

use crate::error::Result;

/// Main adapter configuration
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Config {
    /// Homarr API URL
    #[serde(default = "default_homarr_url")]
    pub homarr_url: String,

    /// Path to branding config file
    #[serde(default = "default_branding_file")]
    pub branding_file: String,

    /// Path to state file
    #[serde(default = "default_state_file")]
    pub state_file: String,

    /// Docker socket path
    #[serde(default = "default_docker_socket")]
    pub docker_socket: String,

    /// Enable debug logging
    #[serde(default)]
    pub debug: bool,
}

fn default_homarr_url() -> String {
    "http://localhost:7575".to_string()
}

fn default_branding_file() -> String {
    "/etc/halos-homarr-branding/branding.toml".to_string()
}

fn default_state_file() -> String {
    "/var/lib/homarr-container-adapter/state.json".to_string()
}

fn default_docker_socket() -> String {
    "/var/run/docker.sock".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            homarr_url: default_homarr_url(),
            branding_file: default_branding_file(),
            state_file: default_state_file(),
            docker_socket: default_docker_socket(),
            debug: false,
        }
    }
}

impl Config {
    /// Load configuration from file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        if !path.exists() {
            tracing::warn!("Config file not found at {:?}, using defaults", path);
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&contents)?;

        Ok(config)
    }
}
