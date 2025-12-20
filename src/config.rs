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

    /// Path to app registry directory
    #[serde(default = "default_registry_dir")]
    pub registry_dir: String,

    /// Path to Authelia users database file
    #[serde(default = "default_authelia_users_db")]
    pub authelia_users_db: String,

    /// Enable debug logging
    #[serde(default)]
    pub debug: bool,

    /// Periodic sync interval in seconds (for watch mode)
    #[serde(default = "default_sync_interval")]
    pub sync_interval: u64,

    /// Startup delay in seconds before first sync (for watch mode)
    #[serde(default = "default_startup_delay")]
    pub startup_delay: u64,
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

fn default_registry_dir() -> String {
    "/etc/halos/webapps.d".to_string()
}

fn default_authelia_users_db() -> String {
    "/var/lib/container-apps/authelia-container/data/users_database.yml".to_string()
}

fn default_sync_interval() -> u64 {
    300 // 5 minutes
}

fn default_startup_delay() -> u64 {
    10 // 10 seconds
}

impl Default for Config {
    fn default() -> Self {
        Self {
            homarr_url: default_homarr_url(),
            branding_file: default_branding_file(),
            state_file: default_state_file(),
            docker_socket: default_docker_socket(),
            registry_dir: default_registry_dir(),
            authelia_users_db: default_authelia_users_db(),
            debug: false,
            sync_interval: default_sync_interval(),
            startup_delay: default_startup_delay(),
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
