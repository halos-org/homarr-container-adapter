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

    /// Whether Authelia credential sync has been completed
    #[serde(default)]
    pub authelia_sync_completed: bool,

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

/// Discovered app metadata stored in state.
/// Note: The HashMap key is the app URL (stable identifier).
/// Container ID is stored for reference but not used as key since it changes on container restart.
#[derive(Debug, Serialize, Deserialize)]
pub struct DiscoveredApp {
    pub name: String,
    pub container_id: String,
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
    #[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_state() {
        let state = State::default();
        assert!(!state.first_boot_completed);
        assert!(state.removed_apps.is_empty());
        assert!(state.last_sync.is_none());
        assert!(state.discovered_apps.is_empty());
        // Default derive uses String::default() (empty), default_version is for serde
        assert!(state.version.is_empty());
    }

    #[test]
    fn test_load_nonexistent_returns_default() {
        let result = State::load("/nonexistent/path/state.json");
        assert!(result.is_ok());
        let state = result.unwrap();
        assert!(!state.first_boot_completed);
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state.json");

        let mut state = State {
            first_boot_completed: true,
            ..Default::default()
        };
        state.mark_removed("app1");
        state.mark_removed("app2");
        state.update_sync_time();

        // Save
        state.save(&state_path).unwrap();

        // Load back
        let loaded = State::load(&state_path).unwrap();
        assert!(loaded.first_boot_completed);
        assert!(loaded.is_removed("app1"));
        assert!(loaded.is_removed("app2"));
        assert!(!loaded.is_removed("app3"));
        assert!(loaded.last_sync.is_some());
    }

    #[test]
    fn test_mark_removed_and_is_removed() {
        let mut state = State::default();

        assert!(!state.is_removed("test-app"));
        state.mark_removed("test-app");
        assert!(state.is_removed("test-app"));
    }

    #[test]
    fn test_update_sync_time() {
        let mut state = State::default();
        assert!(state.last_sync.is_none());

        let before = Utc::now();
        state.update_sync_time();
        let after = Utc::now();

        let sync_time = state.last_sync.unwrap();
        assert!(sync_time >= before && sync_time <= after);
    }

    #[test]
    fn test_save_creates_parent_directories() {
        let temp_dir = TempDir::new().unwrap();
        let nested_path = temp_dir
            .path()
            .join("nested")
            .join("dir")
            .join("state.json");

        let state = State::default();
        let result = state.save(&nested_path);
        assert!(result.is_ok());
        assert!(nested_path.exists());
    }

    // Tests for URL-based deduplication (issue #15)

    #[test]
    fn test_discovered_apps_keyed_by_url() {
        let mut state = State::default();
        let url = "http://localhost:3000".to_string();

        state.discovered_apps.insert(
            url.clone(),
            DiscoveredApp {
                name: "Signal K".to_string(),
                container_id: "abc123".to_string(),
                added_at: Utc::now(),
            },
        );

        // Should find by URL
        assert!(state.discovered_apps.contains_key(&url));
        // Should NOT find by container_id (that's not the key anymore)
        assert!(!state.discovered_apps.contains_key("abc123"));
    }

    #[test]
    fn test_same_url_different_container_id_no_duplicate() {
        let mut state = State::default();
        let url = "http://localhost:3000".to_string();

        // First container with this URL
        state.discovered_apps.insert(
            url.clone(),
            DiscoveredApp {
                name: "Signal K".to_string(),
                container_id: "abc123".to_string(),
                added_at: Utc::now(),
            },
        );

        // Same URL, different container (after restart)
        // HashMap.insert with same key replaces the value - no duplicate possible
        // In the actual app code, we use get_mut() to update container_id in place
        state.discovered_apps.insert(
            url.clone(),
            DiscoveredApp {
                name: "Signal K".to_string(),
                container_id: "def456".to_string(),
                added_at: Utc::now(),
            },
        );

        // Should have exactly one entry (URL-keyed HashMap prevents duplicates)
        assert_eq!(state.discovered_apps.len(), 1);
        // Should have the new container_id
        assert_eq!(
            state.discovered_apps.get(&url).unwrap().container_id,
            "def456"
        );
    }

    #[test]
    fn test_removed_apps_tracked_by_url() {
        let mut state = State::default();
        let url = "http://localhost:3000";

        assert!(!state.is_removed(url));
        state.mark_removed(url);
        assert!(state.is_removed(url));

        // Different container ID with same URL should still be considered removed
        // (we track by URL, not container_id)
    }
}
