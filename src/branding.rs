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
    pub logo_path: String,
    #[serde(default)]
    pub favicon_path: Option<String>,
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
    pub column_count: u8,
    pub is_public: bool,
    pub cockpit: CockpitTile,
}

#[derive(Debug, Deserialize)]
pub struct CockpitTile {
    pub enabled: bool,
    pub name: String,
    pub description: String,
    pub href: String,
    pub icon_url: String,
    pub width: u8,
    pub height: u8,
    pub x_offset: u8,
    pub y_offset: u8,
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

        Ok(config)
    }
}
