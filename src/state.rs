//! Adapter state persistence

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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

    /// Homarr API key for authentication
    /// Format: "{id}.{token}" (e.g., "abc123.randomtoken...")
    /// This is rotated from the bootstrap key on first boot.
    #[serde(default)]
    pub api_key: Option<String>,

    /// Apps removed from specific boards (don't re-add to that board)
    /// Key: board_id, Value: set of app URLs removed from that board
    #[serde(default)]
    pub removed_apps_by_board: HashMap<String, HashSet<String>>,

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

    /// Check if an app was removed from a specific board
    pub fn is_removed_from_board(&self, board_id: &str, app_url: &str) -> bool {
        self.removed_apps_by_board
            .get(board_id)
            .map(|apps| apps.contains(app_url))
            .unwrap_or(false)
    }

    /// Mark an app as removed from a specific board
    #[allow(dead_code)]
    pub fn mark_removed_from_board(&mut self, board_id: &str, app_url: &str) {
        self.removed_apps_by_board
            .entry(board_id.to_string())
            .or_default()
            .insert(app_url.to_string());
    }

    /// Clear the removed flag for an app on a specific board
    /// Called when user manually re-adds an app to a board
    #[allow(dead_code)]
    pub fn clear_removed_from_board(&mut self, board_id: &str, app_url: &str) {
        if let Some(apps) = self.removed_apps_by_board.get_mut(board_id) {
            apps.remove(app_url);
        }
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
        assert!(state.removed_apps_by_board.is_empty());
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
        state.mark_removed_from_board("board-1", "http://app1.local");
        state.mark_removed_from_board("board-1", "http://app2.local");
        state.mark_removed_from_board("board-2", "http://app1.local");
        state.update_sync_time();

        // Save
        state.save(&state_path).unwrap();

        // Load back
        let loaded = State::load(&state_path).unwrap();
        assert!(loaded.first_boot_completed);
        assert!(loaded.is_removed_from_board("board-1", "http://app1.local"));
        assert!(loaded.is_removed_from_board("board-1", "http://app2.local"));
        assert!(loaded.is_removed_from_board("board-2", "http://app1.local"));
        assert!(!loaded.is_removed_from_board("board-2", "http://app2.local"));
        assert!(!loaded.is_removed_from_board("board-3", "http://app1.local"));
        assert!(loaded.last_sync.is_some());
    }

    #[test]
    fn test_per_board_removal_tracking() {
        let mut state = State::default();
        let board_a = "board-a";
        let board_b = "board-b";
        let app_url = "http://test-app.local";

        // Initially not removed from any board
        assert!(!state.is_removed_from_board(board_a, app_url));
        assert!(!state.is_removed_from_board(board_b, app_url));

        // Mark removed from board A
        state.mark_removed_from_board(board_a, app_url);

        // Should be removed from A but not B
        assert!(state.is_removed_from_board(board_a, app_url));
        assert!(!state.is_removed_from_board(board_b, app_url));

        // Mark removed from board B too
        state.mark_removed_from_board(board_b, app_url);
        assert!(state.is_removed_from_board(board_b, app_url));

        // Clear from board A (user re-added)
        state.clear_removed_from_board(board_a, app_url);
        assert!(!state.is_removed_from_board(board_a, app_url));
        assert!(state.is_removed_from_board(board_b, app_url));
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
    fn test_removed_apps_tracked_by_url_per_board() {
        let mut state = State::default();
        let board_id = "test-board";
        let url = "http://localhost:3000";

        assert!(!state.is_removed_from_board(board_id, url));
        state.mark_removed_from_board(board_id, url);
        assert!(state.is_removed_from_board(board_id, url));

        // Different container ID with same URL should still be considered removed
        // (we track by URL, not container_id)
    }

    #[test]
    fn test_clear_removed_nonexistent_board() {
        let mut state = State::default();
        // Should not panic when clearing from a board that doesn't exist
        state.clear_removed_from_board("nonexistent-board", "http://app.local");
        assert!(!state.is_removed_from_board("nonexistent-board", "http://app.local"));
    }
}
