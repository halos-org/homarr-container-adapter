//! App registry for loading app definitions from static files
//!
//! Apps are defined in TOML files in `/etc/halos/webapps.d/`.
//! This module handles loading, parsing, and watching registry files.

use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{AdapterError, Result};

/// Default registry directory
#[allow(dead_code)]
pub const DEFAULT_REGISTRY_DIR: &str = "/etc/halos/webapps.d";

/// App definition from a registry file
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct AppDefinition {
    /// Display name for the app
    pub name: String,

    /// URL to access the app
    pub url: String,

    /// Optional description
    pub description: Option<String>,

    /// Icon URL (can be /icons/*, http(s)://, or /usr/share/pixmaps/*)
    pub icon_url: Option<String>,

    /// Category for grouping (e.g., "Marine", "System")
    pub category: Option<String>,

    /// App type classification
    #[serde(rename = "type", default)]
    pub app_type: AppType,

    /// Optional override for ping URL (health checks)
    pub ping_url: Option<String>,

    /// Board layout configuration (includes priority)
    #[serde(default)]
    pub layout: LayoutConfig,
}

/// App type - determines how health checks work
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AppType {
    /// Docker container name (enables container health tracking)
    pub container_name: Option<String>,

    /// External link flag (no health checks)
    #[serde(default)]
    pub external: bool,
}

/// Board layout configuration
#[derive(Debug, Clone, Deserialize)]
pub struct LayoutConfig {
    /// Priority for placement order (lower = placed first, default: 50)
    /// Ranges: 00-19 system, 20-39 primary, 40-59 default, 60-79 utility, 80-99 external
    #[serde(default = "default_priority")]
    pub priority: u8,

    /// Width in grid columns (default: 1)
    #[serde(default = "default_size")]
    pub width: u8,

    /// Height in grid rows (default: 1)
    #[serde(default = "default_size")]
    pub height: u8,

    /// Explicit column position (0-11 for 12-column grid)
    /// If omitted, auto-positioned based on priority
    pub x_offset: Option<u8>,

    /// Explicit row position
    /// If omitted, auto-positioned based on priority
    pub y_offset: Option<u8>,
}

fn default_priority() -> u8 {
    50
}

fn default_size() -> u8 {
    1
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            priority: 50,
            width: 1,
            height: 1,
            x_offset: None,
            y_offset: None,
        }
    }
}

/// Loaded registry entry with source file path
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RegistryEntry {
    /// Source file path
    pub file_path: PathBuf,

    /// App definition from the file
    pub app: AppDefinition,
}

impl AppDefinition {
    /// Check if this is a Docker container app
    #[allow(dead_code)]
    pub fn is_container(&self) -> bool {
        self.app_type.container_name.is_some()
    }

    /// Check if this is an external link (no health checks)
    pub fn is_external(&self) -> bool {
        self.app_type.external
    }

    /// Get the container name if this is a container app
    pub fn container_name(&self) -> Option<&str> {
        self.app_type.container_name.as_deref()
    }

    /// Get the layout configuration
    pub fn effective_layout(&self) -> &LayoutConfig {
        &self.layout
    }

    /// Get priority for sorting (convenience method)
    #[allow(dead_code)]
    pub fn priority(&self) -> u8 {
        self.layout.priority
    }
}

/// Load all app definitions from the registry directory
pub fn load_all_apps<P: AsRef<Path>>(registry_dir: P) -> Result<Vec<RegistryEntry>> {
    let registry_dir = registry_dir.as_ref();

    if !registry_dir.exists() {
        tracing::warn!(
            "Registry directory does not exist: {:?}, no apps will be loaded",
            registry_dir
        );
        return Ok(Vec::new());
    }

    if !registry_dir.is_dir() {
        return Err(AdapterError::Config(format!(
            "Registry path is not a directory: {:?}",
            registry_dir
        )));
    }

    let mut entries = Vec::new();

    let dir_entries = fs::read_dir(registry_dir)?;

    for entry in dir_entries {
        let entry = entry?;
        let path = entry.path();

        // Only process .toml files
        if path.extension().map(|e| e == "toml").unwrap_or(false) {
            match load_app_file(&path) {
                Ok(app) => {
                    tracing::debug!("Loaded app '{}' from {:?}", app.name, path);
                    entries.push(RegistryEntry {
                        file_path: path,
                        app,
                    });
                }
                Err(e) => {
                    tracing::warn!("Failed to load app from {:?}: {}", path, e);
                    // Continue loading other files
                }
            }
        }
    }

    // Sort by priority (lower = first)
    entries.sort_by_key(|e| e.app.layout.priority);

    tracing::info!(
        "Loaded {} apps from registry directory {:?}",
        entries.len(),
        registry_dir
    );

    Ok(entries)
}

/// Load a single app definition from a file
fn load_app_file<P: AsRef<Path>>(path: P) -> Result<AppDefinition> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path)?;
    let app: AppDefinition = toml::from_str(&contents)?;

    // Validate required fields
    if app.name.is_empty() {
        return Err(AdapterError::Config(format!(
            "App name is empty in {:?}",
            path
        )));
    }

    if app.url.is_empty() {
        return Err(AdapterError::Config(format!(
            "App URL is empty in {:?}",
            path
        )));
    }

    Ok(app)
}

/// Get apps as a HashMap keyed by URL (for deduplication)
#[allow(dead_code)]
pub fn apps_by_url(entries: &[RegistryEntry]) -> HashMap<String, &RegistryEntry> {
    entries.iter().map(|e| (e.app.url.clone(), e)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn create_test_app_file(dir: &Path, name: &str, content: &str) {
        let path = dir.join(format!("{}.toml", name));
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn test_load_minimal_app() {
        let dir = TempDir::new().unwrap();
        create_test_app_file(
            dir.path(),
            "test-app",
            r#"
name = "Test App"
url = "http://localhost:8080"
"#,
        );

        let entries = load_all_apps(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].app.name, "Test App");
        assert_eq!(entries[0].app.url, "http://localhost:8080");
        assert_eq!(entries[0].app.priority(), 50); // default
        assert!(!entries[0].app.is_container());
        assert!(!entries[0].app.is_external());
    }

    #[test]
    fn test_load_full_app() {
        let dir = TempDir::new().unwrap();
        create_test_app_file(
            dir.path(),
            "signalk",
            r#"
name = "Signal K"
url = "http://halos.local:3000"
description = "Marine data server"
icon_url = "/icons/signalk.png"
category = "Marine"

[type]
container_name = "signalk-server"

[layout]
priority = 25
width = 2
height = 2
x_offset = 0
y_offset = 0
"#,
        );

        let entries = load_all_apps(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);

        let app = &entries[0].app;
        assert_eq!(app.name, "Signal K");
        assert_eq!(app.priority(), 25);
        assert!(app.is_container());
        assert_eq!(app.container_name(), Some("signalk-server"));

        let layout = app.effective_layout();
        assert_eq!(layout.priority, 25);
        assert_eq!(layout.width, 2);
        assert_eq!(layout.height, 2);
        assert_eq!(layout.x_offset, Some(0));
        assert_eq!(layout.y_offset, Some(0));
    }

    #[test]
    fn test_load_external_app() {
        let dir = TempDir::new().unwrap();
        create_test_app_file(
            dir.path(),
            "docs",
            r#"
name = "Documentation"
url = "https://docs.example.com"

[type]
external = true

[layout]
priority = 85
"#,
        );

        let entries = load_all_apps(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].app.is_external());
        assert!(!entries[0].app.is_container());
        assert_eq!(entries[0].app.priority(), 85);
    }

    #[test]
    fn test_priority_sorting() {
        let dir = TempDir::new().unwrap();

        create_test_app_file(
            dir.path(),
            "app-c",
            r#"
name = "App C"
url = "http://localhost:3"

[layout]
priority = 50
"#,
        );

        create_test_app_file(
            dir.path(),
            "app-a",
            r#"
name = "App A"
url = "http://localhost:1"

[layout]
priority = 10
"#,
        );

        create_test_app_file(
            dir.path(),
            "app-b",
            r#"
name = "App B"
url = "http://localhost:2"

[layout]
priority = 25
"#,
        );

        let entries = load_all_apps(dir.path()).unwrap();
        assert_eq!(entries.len(), 3);

        // Should be sorted by priority
        assert_eq!(entries[0].app.name, "App A");
        assert_eq!(entries[1].app.name, "App B");
        assert_eq!(entries[2].app.name, "App C");
    }

    #[test]
    fn test_empty_directory() {
        let dir = TempDir::new().unwrap();
        let entries = load_all_apps(dir.path()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_nonexistent_directory() {
        let entries = load_all_apps("/nonexistent/path").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_invalid_file_skipped() {
        let dir = TempDir::new().unwrap();

        // Valid file
        create_test_app_file(
            dir.path(),
            "valid",
            r#"
name = "Valid App"
url = "http://localhost:1"
"#,
        );

        // Invalid file (missing required field)
        create_test_app_file(
            dir.path(),
            "invalid",
            r#"
name = "Invalid App"
# missing url
"#,
        );

        let entries = load_all_apps(dir.path()).unwrap();
        // Only valid file should be loaded
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].app.name, "Valid App");
    }

    #[test]
    fn test_non_toml_files_ignored() {
        let dir = TempDir::new().unwrap();

        create_test_app_file(
            dir.path(),
            "valid",
            r#"
name = "Valid App"
url = "http://localhost:1"
"#,
        );

        // Create a non-TOML file
        let txt_path = dir.path().join("readme.txt");
        fs::write(&txt_path, "This is not a TOML file").unwrap();

        let entries = load_all_apps(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
    }
}
