//! Adapter state persistence

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::error::{AdapterError, Result};

/// Persistent state for the adapter
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// Schema version for migrations
    #[serde(default = "default_version")]
    pub version: String,

    /// Whether first-boot setup has been completed
    #[serde(default)]
    pub first_boot_completed: bool,

    /// Apps that the user has removed from Homarr (don't re-add)
    #[serde(default)]
    pub removed_apps: HashSet<String>,

    /// Last sync timestamp
    #[serde(default)]
    pub last_sync: Option<DateTime<Utc>>,

    /// Discovered apps and when they were added
    #[serde(default)]
    pub discovered_apps: std::collections::HashMap<String, DiscoveredApp>,
}

fn default_version() -> String {
    "1.0".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiscoveredApp {
    pub name: String,
    pub url: String,
    pub added_at: DateTime<Utc>,
}

impl State {
    /// Load state from file, returning default if file doesn't exist
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(path)?;
        let state: State = serde_json::from_str(&contents).map_err(|e| {
            tracing::warn!("Failed to parse state file, using defaults: {}", e);
            AdapterError::State(format!("Failed to parse state: {}", e))
        })?;

        Ok(state)
    }

    /// Save state to file
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let contents = serde_json::to_string_pretty(self)?;
        fs::write(path, contents)?;

        Ok(())
    }

    /// Mark an app as removed by user
    pub fn mark_removed(&mut self, app_id: &str) {
        self.removed_apps.insert(app_id.to_string());
    }

    /// Check if an app was removed by user
    pub fn is_removed(&self, app_id: &str) -> bool {
        self.removed_apps.contains(app_id)
    }

    /// Update last sync time
    pub fn update_sync_time(&mut self) {
        self.last_sync = Some(Utc::now());
    }
}
