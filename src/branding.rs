//! Branding configuration from halos-homarr-branding package

use serde::Deserialize;
use std::fs;
use std::path::Path;

use crate::error::{AdapterError, Result};

/// Branding configuration loaded from /etc/halos-homarr-branding/branding.toml
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct BrandingConfig {
    pub identity: Identity,
    pub theme: Theme,
    pub credentials: Credentials,
    pub board: Board,
    pub settings: Settings,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Identity {
    pub product_name: String,
    /// Browser tab title
    #[serde(default)]
    pub page_title: Option<String>,
    /// Meta title for SEO
    #[serde(default)]
    pub meta_title: Option<String>,
    /// Logo URL served via /branding/ prefix
    #[serde(default)]
    pub logo_image_url: Option<String>,
    /// Favicon URL served via /branding/ prefix
    #[serde(default)]
    pub favicon_image_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Theme {
    pub default_color_scheme: String,
    pub primary_color: String,
    pub secondary_color: String,
    #[serde(default = "default_item_radius")]
    pub item_radius: String,
    #[serde(default = "default_opacity")]
    pub opacity: u8,
    /// Background image URL served via /branding/ prefix
    #[serde(default)]
    pub background_image_url: Option<String>,
    /// Custom CSS to inject into the dashboard
    #[serde(default)]
    pub custom_css: Option<String>,
}

fn default_item_radius() -> String {
    "lg".to_string()
}

fn default_opacity() -> u8 {
    100
}

#[derive(Debug, Deserialize)]
pub struct Credentials {
    pub admin_username: String,
    pub admin_password: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Board {
    pub name: String,
    pub display_name: String,
    #[serde(default = "default_layouts")]
    pub layouts: Vec<LayoutEntry>,
    pub is_public: bool,
}

/// A single responsive layout: a column count that applies from `breakpoint`
/// pixels of viewport width upward, until the next-larger breakpoint takes over.
#[derive(Debug, Deserialize, Clone)]
pub struct LayoutEntry {
    pub name: String,
    pub breakpoint: i32,
    pub column_count: i32,
}

fn default_layouts() -> Vec<LayoutEntry> {
    vec![
        LayoutEntry {
            name: "Mobile".to_string(),
            breakpoint: 0,
            column_count: 4,
        },
        LayoutEntry {
            name: "Tablet".to_string(),
            breakpoint: 768,
            column_count: 6,
        },
        LayoutEntry {
            name: "Desktop".to_string(),
            breakpoint: 1200,
            column_count: 12,
        },
    ]
}

impl Board {
    /// The base layout (smallest breakpoint, normally 0). Homarr falls back to
    /// the smallest-breakpoint layout for very narrow viewports, and its column
    /// count seeds the board at creation time.
    pub fn base_layout(&self) -> &LayoutEntry {
        self.layouts
            .iter()
            .min_by_key(|l| l.breakpoint)
            .expect("board must define at least one layout")
    }
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub analytics: AnalyticsSettings,
    pub crawling: CrawlingSettings,
}

#[derive(Debug, Deserialize)]
pub struct AnalyticsSettings {
    pub enable_general: bool,
    pub enable_widget_data: bool,
    pub enable_integration_data: bool,
    pub enable_user_data: bool,
}

#[derive(Debug, Deserialize)]
pub struct CrawlingSettings {
    pub no_index: bool,
    pub no_follow: bool,
    pub no_translate: bool,
    pub no_sitelinks_search_box: bool,
}

impl BrandingConfig {
    /// Load branding configuration from file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        if !path.exists() {
            return Err(AdapterError::Config(format!(
                "Branding config not found at {:?}",
                path
            )));
        }

        let contents = fs::read_to_string(path)?;
        let config: BrandingConfig = toml::from_str(&contents)?;

        if config.board.layouts.is_empty() {
            return Err(AdapterError::Config(
                "board must define at least one layout".to_string(),
            ));
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_parses_responsive_layout_list() {
        let toml = r#"
            name = "Halos"
            display_name = "Halos"
            is_public = true

            [[layouts]]
            name = "Mobile"
            breakpoint = 0
            column_count = 4

            [[layouts]]
            name = "Desktop"
            breakpoint = 1200
            column_count = 12
        "#;
        let board: Board = toml::from_str(toml).unwrap();
        assert_eq!(board.layouts.len(), 2);
        // base_layout picks the smallest breakpoint regardless of order
        assert_eq!(board.base_layout().breakpoint, 0);
        assert_eq!(board.base_layout().column_count, 4);
    }

    #[test]
    fn board_without_layouts_falls_back_to_defaults() {
        let toml = r#"
            name = "Halos"
            display_name = "Halos"
            is_public = true
        "#;
        let board: Board = toml::from_str(toml).unwrap();
        let breakpoints: Vec<i32> = board.layouts.iter().map(|l| l.breakpoint).collect();
        assert_eq!(breakpoints, vec![0, 768, 1200]);
    }
}
