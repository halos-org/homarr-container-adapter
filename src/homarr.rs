//! Homarr API client

use reqwest::{cookie::Jar, Client};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::branding::BrandingConfig;
use crate::error::{AdapterError, Result};
use crate::registry::AppDefinition;

/// Homarr API client
pub struct HomarrClient {
    client: Client,
    base_url: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct OnboardingStep {
    pub current: String,
    pub previous: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TrpcResponse<T> {
    result: TrpcResult<T>,
}

#[derive(Debug, Deserialize)]
struct TrpcResult<T> {
    data: TrpcData<T>,
}

#[derive(Debug, Deserialize)]
struct TrpcData<T> {
    json: T,
}

#[derive(Debug, Deserialize)]
struct CsrfResponse {
    #[serde(rename = "csrfToken")]
    csrf_token: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct BoardResponse {
    id: String,
    name: String,
    sections: Vec<Section>,
    layouts: Vec<Layout>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct Section {
    id: String,
    kind: String,
    #[serde(rename = "yOffset")]
    y_offset: i32,
    #[serde(rename = "xOffset")]
    x_offset: i32,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct Layout {
    id: String,
    name: String,
    #[serde(rename = "columnCount")]
    column_count: i32,
    breakpoint: i32,
}

#[derive(Debug, Deserialize)]
struct CreateBoardResponse {
    #[serde(rename = "boardId")]
    board_id: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CreateAppResponse {
    #[serde(rename = "appId")]
    app_id: String,
    id: String,
}

/// Minimal app data from app.selectable endpoint
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct SelectableApp {
    pub id: String,
    pub name: String,
    #[serde(rename = "iconUrl")]
    pub icon_url: String,
    pub href: Option<String>,
}

/// Default icon path (relative URL)
const DEFAULT_ICON: &str = "/icons/docker.svg";

/// Derive a host.docker.internal-based ping URL from the app URL.
/// Replaces the hostname with host.docker.internal so Homarr container can reach the app.
/// Note: Requires `extra_hosts: ["host.docker.internal:host-gateway"]` in Homarr's docker-compose.yml
/// Example: "http://halos.local:3000/path" -> "http://host.docker.internal:3000/path"
fn derive_ping_url(app_url: &str) -> Option<String> {
    match url::Url::parse(app_url) {
        Ok(mut parsed) => {
            if parsed.set_host(Some("host.docker.internal")).is_ok() {
                Some(parsed.to_string())
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

/// Simple hash function for generating unique IDs from URLs
fn string_hash(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Check if a board already has an item for a given app ID.
/// Used to prevent duplicate board items when the same app is synced multiple times.
fn board_has_app(items: &[serde_json::Value], app_id: &str) -> bool {
    items.iter().any(|item| {
        item.get("options")
            .and_then(|o| o.get("appId"))
            .and_then(|a| a.as_str())
            == Some(app_id)
    })
}

/// Transform icon paths to relative URLs for Homarr.
///
/// Icons are served by Homarr's nginx from /icons/ which maps to /usr/share/pixmaps.
/// This function transforms:
/// - `/usr/share/pixmaps/app.png` → `/icons/app.png`
/// - HTTP/HTTPS URLs → unchanged
/// - `/icons/*` paths → unchanged
/// - Everything else → `/icons/docker.svg` (fallback)
fn transform_icon_url(icon_path: &str) -> String {
    const PIXMAPS_PREFIX: &str = "/usr/share/pixmaps/";

    if icon_path.is_empty() {
        return DEFAULT_ICON.to_string();
    }

    // HTTP/HTTPS URLs pass through unchanged
    if icon_path.starts_with("http://") || icon_path.starts_with("https://") {
        return icon_path.to_string();
    }

    // Already relative /icons/ paths - pass through unchanged
    if icon_path.starts_with("/icons/") {
        return icon_path.to_string();
    }

    // Transform /usr/share/pixmaps/ paths to /icons/
    if let Some(filename) = icon_path.strip_prefix(PIXMAPS_PREFIX) {
        if filename.is_empty() {
            return DEFAULT_ICON.to_string();
        }
        return format!("/icons/{}", filename);
    }

    // Unknown format - use fallback
    DEFAULT_ICON.to_string()
}

impl HomarrClient {
    /// Create a new Homarr client
    ///
    /// # Arguments
    /// * `base_url` - The Homarr API base URL (e.g., "http://localhost:80")
    pub fn new(base_url: &str) -> Result<Self> {
        let jar = Arc::new(Jar::default());
        let client = Client::builder()
            .cookie_store(true)
            .cookie_provider(jar)
            // Accept self-signed certificates (required for local SSL configurations)
            .danger_accept_invalid_certs(true)
            .build()?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    /// Get current onboarding step
    pub async fn get_onboarding_step(&self) -> Result<OnboardingStep> {
        let url = format!("{}/api/trpc/onboard.currentStep", self.base_url);
        let response: TrpcResponse<OnboardingStep> =
            self.client.get(&url).send().await?.json().await?;
        Ok(response.result.data.json)
    }

    /// Complete the onboarding flow
    pub async fn complete_onboarding(&self, branding: &BrandingConfig) -> Result<()> {
        // Step through onboarding until we reach the user step
        loop {
            let step = self.get_onboarding_step().await?;
            tracing::info!("Onboarding step: {}", step.current);

            match step.current.as_str() {
                "finish" => break,
                "start" => {
                    self.advance_onboarding_step().await?;
                }
                "user" => {
                    self.create_initial_user(branding).await?;
                }
                "settings" => {
                    self.configure_settings(branding).await?;
                }
                _ => {
                    // Skip other steps
                    self.advance_onboarding_step().await?;
                }
            }
        }

        Ok(())
    }

    /// Advance to next onboarding step
    async fn advance_onboarding_step(&self) -> Result<()> {
        let url = format!("{}/api/trpc/onboard.nextStep", self.base_url);
        self.client
            .post(&url)
            .json(&json!({"json": {}}))
            .send()
            .await?;
        Ok(())
    }

    /// Create initial admin user
    async fn create_initial_user(&self, branding: &BrandingConfig) -> Result<()> {
        let url = format!("{}/api/trpc/user.initUser", self.base_url);
        let payload = json!({
            "json": {
                "username": branding.credentials.admin_username,
                "password": branding.credentials.admin_password,
                "confirmPassword": branding.credentials.admin_password
            }
        });

        let response = self.client.post(&url).json(&payload).send().await?;

        if !response.status().is_success() {
            let text = response.text().await?;
            return Err(AdapterError::HomarrApi(format!(
                "Failed to create user: {}",
                text
            )));
        }

        Ok(())
    }

    /// Configure server settings
    async fn configure_settings(&self, branding: &BrandingConfig) -> Result<()> {
        let url = format!("{}/api/trpc/serverSettings.initSettings", self.base_url);
        let payload = json!({
            "json": {
                "analytics": {
                    "enableGeneral": branding.settings.analytics.enable_general,
                    "enableWidgetData": branding.settings.analytics.enable_widget_data,
                    "enableIntegrationData": branding.settings.analytics.enable_integration_data,
                    "enableUserData": branding.settings.analytics.enable_user_data
                },
                "crawlingAndIndexing": {
                    "noIndex": branding.settings.crawling.no_index,
                    "noFollow": branding.settings.crawling.no_follow,
                    "noTranslate": branding.settings.crawling.no_translate,
                    "noSiteLinksSearchBox": branding.settings.crawling.no_sitelinks_search_box
                }
            }
        });

        self.client.post(&url).json(&payload).send().await?;
        Ok(())
    }

    /// Login to Homarr and get session
    async fn login(&self, branding: &BrandingConfig) -> Result<()> {
        // Get CSRF token
        let csrf_url = format!("{}/api/auth/csrf", self.base_url);
        let csrf_response: CsrfResponse = self.client.get(&csrf_url).send().await?.json().await?;

        // Login
        let login_url = format!("{}/api/auth/callback/credentials", self.base_url);
        let params = [
            ("csrfToken", csrf_response.csrf_token.as_str()),
            ("name", &branding.credentials.admin_username),
            ("password", &branding.credentials.admin_password),
        ];

        let response = self.client.post(&login_url).form(&params).send().await?;

        if !response.status().is_success() && response.status().as_u16() != 302 {
            return Err(AdapterError::HomarrApi("Login failed".to_string()));
        }

        Ok(())
    }

    /// Set up default board
    pub async fn setup_default_board(&self, branding: &BrandingConfig) -> Result<()> {
        // Login first
        self.login(branding).await?;

        // Check if board already exists
        let board = self.get_board_by_name(&branding.board.name).await;

        let board_id = if let Ok(board) = board {
            tracing::info!("Board '{}' already exists", branding.board.name);
            board.id
        } else {
            // Create the board
            tracing::info!("Creating board '{}'", branding.board.name);
            self.create_board(branding).await?
        };

        // Apply board branding settings (page title, logo, colors, etc.)
        self.save_board_branding_settings(&board_id, branding)
            .await?;

        // Set as home board
        self.set_home_board(&board_id).await?;

        // Set color scheme
        self.set_color_scheme(&branding.theme.default_color_scheme)
            .await?;

        Ok(())
    }

    /// Save board branding settings (page title, meta title, logo, favicon, colors)
    async fn save_board_branding_settings(
        &self,
        board_id: &str,
        branding: &BrandingConfig,
    ) -> Result<()> {
        let url = format!("{}/api/trpc/board.savePartialBoardSettings", self.base_url);

        // Build the settings payload with only non-null values
        let mut settings = serde_json::Map::new();
        settings.insert("id".to_string(), json!(board_id));

        // Add page title if configured
        if let Some(ref page_title) = branding.identity.page_title {
            settings.insert("pageTitle".to_string(), json!(page_title));
        }

        // Add meta title if configured
        if let Some(ref meta_title) = branding.identity.meta_title {
            settings.insert("metaTitle".to_string(), json!(meta_title));
        }

        // Add logo URL if configured
        if let Some(ref logo_url) = branding.identity.logo_image_url {
            settings.insert("logoImageUrl".to_string(), json!(logo_url));
        }

        // Add favicon URL if configured
        if let Some(ref favicon_url) = branding.identity.favicon_image_url {
            settings.insert("faviconImageUrl".to_string(), json!(favicon_url));
        }

        // Add theme settings
        settings.insert(
            "primaryColor".to_string(),
            json!(branding.theme.primary_color),
        );
        settings.insert(
            "secondaryColor".to_string(),
            json!(branding.theme.secondary_color),
        );
        settings.insert("opacity".to_string(), json!(branding.theme.opacity));
        settings.insert("itemRadius".to_string(), json!(branding.theme.item_radius));

        // Add background image URL if configured
        if let Some(ref bg_url) = branding.theme.background_image_url {
            settings.insert("backgroundImageUrl".to_string(), json!(bg_url));
        }

        // Add custom CSS if configured
        if let Some(ref custom_css) = branding.theme.custom_css {
            settings.insert("customCss".to_string(), json!(custom_css));
        }

        let payload = json!({ "json": settings });

        tracing::info!("Applying board branding settings");
        let response = self.client.post(&url).json(&payload).send().await?;

        if !response.status().is_success() {
            let text = response.text().await?;
            tracing::warn!("Failed to save board branding settings: {}", text);
            // Don't fail the whole setup if branding settings fail
        }

        Ok(())
    }

    /// Get board by name
    async fn get_board_by_name(&self, name: &str) -> Result<BoardResponse> {
        let url = format!(
            "{}/api/trpc/board.getBoardByName?input={}",
            self.base_url,
            urlencoding::encode(&format!("{{\"json\":{{\"name\":\"{}\"}}}}", name))
        );

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(AdapterError::HomarrApi("Board not found".to_string()));
        }

        let trpc_response: TrpcResponse<BoardResponse> = response.json().await?;
        Ok(trpc_response.result.data.json)
    }

    /// Create a new board
    async fn create_board(&self, branding: &BrandingConfig) -> Result<String> {
        let url = format!("{}/api/trpc/board.createBoard", self.base_url);
        let payload = json!({
            "json": {
                "name": branding.board.name,
                "columnCount": branding.board.column_count,
                "isPublic": branding.board.is_public
            }
        });

        let response = self.client.post(&url).json(&payload).send().await?;
        let trpc_response: TrpcResponse<CreateBoardResponse> = response.json().await?;

        Ok(trpc_response.result.data.json.board_id)
    }

    /// Set home board
    async fn set_home_board(&self, board_id: &str) -> Result<()> {
        let url = format!("{}/api/trpc/board.setHomeBoard", self.base_url);
        let payload = json!({"json": {"id": board_id}});
        self.client.post(&url).json(&payload).send().await?;
        Ok(())
    }

    /// Set color scheme
    async fn set_color_scheme(&self, scheme: &str) -> Result<()> {
        let url = format!("{}/api/trpc/user.changeColorScheme", self.base_url);
        let payload = json!({"json": {"colorScheme": scheme}});
        self.client.post(&url).json(&payload).send().await?;
        Ok(())
    }

    /// Ensure we're logged in
    pub async fn ensure_logged_in(&self, branding: &BrandingConfig) -> Result<()> {
        self.login(branding).await
    }

    /// Get all apps (minimal data for matching)
    ///
    /// Returns all apps from Homarr for deduplication checks.
    /// Callers can cache this result to avoid repeated API calls.
    pub async fn get_all_apps(&self) -> Result<Vec<SelectableApp>> {
        let url = format!("{}/api/trpc/app.selectable", self.base_url);
        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(AdapterError::HomarrApi(format!(
                "Failed to fetch apps ({}): {}",
                status, text
            )));
        }

        let trpc_response: TrpcResponse<Vec<SelectableApp>> = response.json().await?;
        Ok(trpc_response.result.data.json)
    }

    /// Find an existing app by URL in a pre-fetched list
    fn find_app_in_list<'a>(apps: &'a [SelectableApp], url: &str) -> Option<&'a SelectableApp> {
        apps.iter().find(|app| app.href.as_deref() == Some(url))
    }

    /// Add a registry app to Homarr (or update if already exists)
    ///
    /// Registry apps can have explicit layout positioning and may not be Docker containers.
    pub async fn add_registry_app(
        &self,
        app: &AppDefinition,
        board_name: &str,
        existing_apps: Option<&[SelectableApp]>,
    ) -> Result<String> {
        // Check if an app with the same URL already exists
        let existing = match existing_apps {
            Some(apps) => Self::find_app_in_list(apps, &app.url).cloned(),
            None => match self.get_all_apps().await {
                Ok(apps) => Self::find_app_in_list(&apps, &app.url).cloned(),
                Err(e) => {
                    tracing::warn!(
                        "Failed to fetch existing apps for deduplication: {}. \
                             Proceeding with create.",
                        e
                    );
                    None
                }
            },
        };

        if let Some(existing_app) = existing {
            // App already exists - update it and ensure it's on the board
            self.update_registry_app(&existing_app.id, app).await?;
            self.add_registry_app_to_board(&existing_app.id, app, board_name)
                .await?;
            return Ok(existing_app.id);
        }

        // Create new app in Homarr
        let url = format!("{}/api/trpc/app.create", self.base_url);
        let icon_url = transform_icon_url(app.icon_url.as_deref().unwrap_or(DEFAULT_ICON));

        // Use explicit ping_url if provided, otherwise derive from URL
        // For external apps, don't set a ping URL (no health checks)
        let ping_url = if app.is_external() {
            None
        } else {
            app.ping_url.clone().or_else(|| derive_ping_url(&app.url))
        };

        let payload = json!({
            "json": {
                "name": app.name,
                "description": app.description.clone().unwrap_or_default(),
                "iconUrl": icon_url,
                "href": app.url,
                "pingUrl": ping_url
            }
        });

        let response = self.client.post(&url).json(&payload).send().await?;

        if !response.status().is_success() {
            let text = response.text().await?;
            return Err(AdapterError::HomarrApi(format!(
                "Failed to create registry app '{}': {}",
                app.name, text
            )));
        }

        let app_response: TrpcResponse<CreateAppResponse> = response.json().await?;
        let app_id = app_response.result.data.json.app_id;

        // Add to board with layout preferences
        self.add_registry_app_to_board(&app_id, app, board_name)
            .await?;

        tracing::info!(
            "Added registry app '{}' to Homarr (app_id: {})",
            app.name,
            app_id
        );
        Ok(app_id)
    }

    /// Update an existing app with registry app data
    async fn update_registry_app(&self, app_id: &str, app: &AppDefinition) -> Result<()> {
        let url = format!("{}/api/trpc/app.update", self.base_url);
        let icon_url = transform_icon_url(app.icon_url.as_deref().unwrap_or(DEFAULT_ICON));

        let ping_url = if app.is_external() {
            None
        } else {
            app.ping_url.clone().or_else(|| derive_ping_url(&app.url))
        };

        let payload = json!({
            "json": {
                "id": app_id,
                "name": app.name,
                "description": app.description.clone().unwrap_or_default(),
                "iconUrl": icon_url,
                "href": app.url,
                "pingUrl": ping_url
            }
        });

        let response = self.client.post(&url).json(&payload).send().await?;

        if !response.status().is_success() {
            let text = response.text().await?;
            return Err(AdapterError::HomarrApi(format!(
                "Failed to update registry app '{}': {}",
                app.name, text
            )));
        }

        tracing::info!(
            "Updated existing registry app '{}' (app_id: {})",
            app.name,
            app_id
        );
        Ok(())
    }

    /// Add a registry app to a board with layout preferences
    async fn add_registry_app_to_board(
        &self,
        app_id: &str,
        app: &AppDefinition,
        board_name: &str,
    ) -> Result<()> {
        let board_items = self.get_board_items(board_name).await.unwrap_or_default();

        // Check if this app is already on the board
        if board_has_app(&board_items, app_id) {
            tracing::info!(
                "Registry app '{}' already on board '{}', skipping",
                app.name,
                board_name
            );
            return Ok(());
        }

        let board = self.get_board_by_name(board_name).await?;

        let section_id = board
            .sections
            .first()
            .map(|s| s.id.clone())
            .unwrap_or_default();
        let layout_id = board
            .layouts
            .first()
            .map(|l| l.id.clone())
            .unwrap_or_default();

        // Get layout preferences from registry
        let layout = app.effective_layout();
        let width = layout.width as i32;
        let height = layout.height as i32;

        // Use explicit position if provided, otherwise auto-position
        let (x_offset, y_offset) = match (layout.x_offset, layout.y_offset) {
            (Some(x), Some(y)) => (x as i32, y as i32),
            _ => self.find_next_position(&board_items, 12), // 12 columns for new layout
        };

        // Generate a unique ID for this board item
        // Use container name if available, otherwise use a hash of the URL
        let item_id = if let Some(container) = app.container_name() {
            format!("registry-{}", container)
        } else {
            format!("registry-{:x}", string_hash(&app.url))
        };

        let url = format!("{}/api/trpc/board.saveBoard", self.base_url);

        let mut items: Vec<serde_json::Value> = board_items;
        items.push(json!({
            "id": item_id,
            "kind": "app",
            "options": {
                "appId": app_id
            },
            "layouts": [{
                "layoutId": layout_id,
                "sectionId": section_id,
                "width": width,
                "height": height,
                "xOffset": x_offset,
                "yOffset": y_offset
            }],
            "integrationIds": [],
            "advancedOptions": {
                "customCssClasses": []
            }
        }));

        let payload = json!({
            "json": {
                "id": board.id,
                "sections": board.sections,
                "items": items,
                "integrations": []
            }
        });

        self.client.post(&url).json(&payload).send().await?;

        tracing::debug!(
            "Added registry app '{}' to board at ({}, {}) size {}x{}",
            app.name,
            x_offset,
            y_offset,
            width,
            height
        );

        Ok(())
    }

    /// Get board items
    async fn get_board_items(&self, board_name: &str) -> Result<Vec<serde_json::Value>> {
        let url = format!(
            "{}/api/trpc/board.getBoardByName?input={}",
            self.base_url,
            urlencoding::encode(&format!("{{\"json\":{{\"name\":\"{}\"}}}}", board_name))
        );

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            return Ok(vec![]);
        }

        // Parse the full board response to get items
        let json: serde_json::Value = response.json().await?;
        let items = json
            .get("result")
            .and_then(|r| r.get("data"))
            .and_then(|d| d.get("json"))
            .and_then(|j| j.get("items"))
            .and_then(|i| i.as_array())
            .cloned()
            .unwrap_or_default();

        Ok(items)
    }

    /// Find next available position on the board (simple left-to-right, top-to-bottom)
    fn find_next_position(&self, items: &[serde_json::Value], column_count: i32) -> (i32, i32) {
        let mut max_y = 0;
        let mut positions_in_max_row: Vec<i32> = vec![];

        for item in items {
            if let Some(layouts) = item.get("layouts").and_then(|l| l.as_array()) {
                for layout in layouts {
                    let x = layout.get("xOffset").and_then(|x| x.as_i64()).unwrap_or(0) as i32;
                    let y = layout.get("yOffset").and_then(|y| y.as_i64()).unwrap_or(0) as i32;
                    let h = layout.get("height").and_then(|h| h.as_i64()).unwrap_or(1) as i32;

                    let item_bottom = y + h;
                    if item_bottom > max_y {
                        max_y = item_bottom;
                        positions_in_max_row.clear();
                    }
                    if y + h == max_y {
                        let w = layout.get("width").and_then(|w| w.as_i64()).unwrap_or(1) as i32;
                        for col in x..(x + w) {
                            positions_in_max_row.push(col);
                        }
                    }
                }
            }
        }

        // Find first empty column in the last row, or start new row
        for x in 0..column_count {
            if !positions_in_max_row.contains(&x) {
                return (x, max_y.saturating_sub(1).max(0));
            }
        }

        // All columns full, start new row
        (0, max_y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_test_client() -> HomarrClient {
        HomarrClient::new("http://localhost:7575").unwrap()
    }

    // HomarrClient creation tests
    #[test]
    fn test_client_new_valid_url() {
        let client = HomarrClient::new("http://localhost:7575");
        assert!(client.is_ok());
    }

    #[test]
    fn test_client_new_strips_trailing_slash() {
        let client = HomarrClient::new("http://localhost:7575/").unwrap();
        assert_eq!(client.base_url, "http://localhost:7575");
    }

    #[test]
    fn test_client_new_preserves_path() {
        let client = HomarrClient::new("http://localhost:7575/homarr").unwrap();
        assert_eq!(client.base_url, "http://localhost:7575/homarr");
    }

    // find_next_position tests
    #[test]
    fn test_find_next_position_empty_board() {
        let client = create_test_client();
        let items: Vec<serde_json::Value> = vec![];
        let (x, y) = client.find_next_position(&items, 10);
        assert_eq!((x, y), (0, 0));
    }

    #[test]
    fn test_find_next_position_single_item() {
        let client = create_test_client();
        let items = vec![json!({
            "layouts": [{
                "xOffset": 0,
                "yOffset": 0,
                "width": 1,
                "height": 1
            }]
        })];
        let (x, y) = client.find_next_position(&items, 10);
        // Should place next to the existing item
        assert_eq!((x, y), (1, 0));
    }

    #[test]
    fn test_find_next_position_full_row() {
        let client = create_test_client();
        // Fill all 10 columns in row 0
        let items: Vec<serde_json::Value> = (0..10)
            .map(|i| {
                json!({
                    "layouts": [{
                        "xOffset": i,
                        "yOffset": 0,
                        "width": 1,
                        "height": 1
                    }]
                })
            })
            .collect();
        let (x, y) = client.find_next_position(&items, 10);
        // Should start a new row
        assert_eq!((x, y), (0, 1));
    }

    #[test]
    fn test_find_next_position_with_gap() {
        let client = create_test_client();
        // Items at positions 0 and 2, leaving gap at 1
        let items = vec![
            json!({
                "layouts": [{
                    "xOffset": 0,
                    "yOffset": 0,
                    "width": 1,
                    "height": 1
                }]
            }),
            json!({
                "layouts": [{
                    "xOffset": 2,
                    "yOffset": 0,
                    "width": 1,
                    "height": 1
                }]
            }),
        ];
        let (x, y) = client.find_next_position(&items, 10);
        // Should fill the gap at position 1
        assert_eq!((x, y), (1, 0));
    }

    #[test]
    fn test_find_next_position_wide_item() {
        let client = create_test_client();
        // Wide item taking columns 0-2
        let items = vec![json!({
            "layouts": [{
                "xOffset": 0,
                "yOffset": 0,
                "width": 3,
                "height": 1
            }]
        })];
        let (x, y) = client.find_next_position(&items, 10);
        // Should place at column 3
        assert_eq!((x, y), (3, 0));
    }

    #[test]
    fn test_find_next_position_tall_item() {
        let client = create_test_client();
        // Tall item at position 0
        let items = vec![json!({
            "layouts": [{
                "xOffset": 0,
                "yOffset": 0,
                "width": 1,
                "height": 3
            }]
        })];
        let (x, y) = client.find_next_position(&items, 10);
        // Should place in the same row but different column
        assert_eq!((x, y), (1, 2));
    }

    #[test]
    fn test_find_next_position_multiple_rows() {
        let client = create_test_client();
        // Items in multiple rows
        let items = vec![
            json!({
                "layouts": [{
                    "xOffset": 0,
                    "yOffset": 0,
                    "width": 10,
                    "height": 1
                }]
            }),
            json!({
                "layouts": [{
                    "xOffset": 0,
                    "yOffset": 1,
                    "width": 5,
                    "height": 1
                }]
            }),
        ];
        let (x, y) = client.find_next_position(&items, 10);
        // Should place after the item in row 1
        assert_eq!((x, y), (5, 1));
    }

    #[test]
    fn test_find_next_position_small_column_count() {
        let client = create_test_client();
        // Fill a 3-column board
        let items: Vec<serde_json::Value> = (0..3)
            .map(|i| {
                json!({
                    "layouts": [{
                        "xOffset": i,
                        "yOffset": 0,
                        "width": 1,
                        "height": 1
                    }]
                })
            })
            .collect();
        let (x, y) = client.find_next_position(&items, 3);
        // Should start a new row
        assert_eq!((x, y), (0, 1));
    }

    #[test]
    fn test_find_next_position_items_without_layouts() {
        let client = create_test_client();
        // Items missing layouts field
        let items = vec![json!({"id": "item1"}), json!({"layouts": []})];
        let (x, y) = client.find_next_position(&items, 10);
        // Should handle gracefully and start at origin
        assert_eq!((x, y), (0, 0));
    }

    // transform_icon_url tests

    #[test]
    fn test_transform_icon_url_pixmaps_path() {
        // File path in /usr/share/pixmaps should become relative /icons/filename
        let result = transform_icon_url("/usr/share/pixmaps/app.png");
        assert_eq!(result, "/icons/app.png");
    }

    #[test]
    fn test_transform_icon_url_pixmaps_nested_path() {
        // Nested paths should preserve directory structure after pixmaps/
        let result = transform_icon_url("/usr/share/pixmaps/subdir/icon.svg");
        assert_eq!(result, "/icons/subdir/icon.svg");
    }

    #[test]
    fn test_transform_icon_url_http_passthrough() {
        // HTTP URLs should pass through unchanged
        let result = transform_icon_url("http://example.com/icon.png");
        assert_eq!(result, "http://example.com/icon.png");
    }

    #[test]
    fn test_transform_icon_url_https_passthrough() {
        // HTTPS URLs should pass through unchanged
        let result = transform_icon_url("https://cdn.example.com/icons/docker.svg");
        assert_eq!(result, "https://cdn.example.com/icons/docker.svg");
    }

    #[test]
    fn test_transform_icon_url_empty_string() {
        // Empty string should return default icon path
        let result = transform_icon_url("");
        assert_eq!(result, "/icons/docker.svg");
    }

    #[test]
    fn test_transform_icon_url_unrecognized_path() {
        // Unrecognized file paths should return default icon path
        let result = transform_icon_url("/some/other/path/icon.png");
        assert_eq!(result, "/icons/docker.svg");
    }

    #[test]
    fn test_transform_icon_url_relative_path() {
        // Relative paths should return default icon path
        let result = transform_icon_url("icons/app.png");
        assert_eq!(result, "/icons/docker.svg");
    }

    #[test]
    fn test_transform_icon_url_icons_path_passthrough() {
        // Already relative /icons/ paths should pass through unchanged
        let result = transform_icon_url("/icons/existing.svg");
        assert_eq!(result, "/icons/existing.svg");
    }

    #[test]
    fn test_transform_icon_url_pixmaps_trailing_slash_only() {
        // Edge case: pixmaps path with trailing slash but no filename
        let result = transform_icon_url("/usr/share/pixmaps/");
        assert_eq!(result, "/icons/docker.svg");
    }

    // Tests for board item deduplication (issue #15)

    #[test]
    fn test_board_has_app_finds_existing() {
        let items = vec![
            json!({
                "id": "discovered-abc123",
                "kind": "app",
                "options": {
                    "appId": "app-xyz-123"
                }
            }),
            json!({
                "id": "discovered-def456",
                "kind": "app",
                "options": {
                    "appId": "app-other-456"
                }
            }),
        ];

        assert!(board_has_app(&items, "app-xyz-123"));
        assert!(board_has_app(&items, "app-other-456"));
        assert!(!board_has_app(&items, "app-nonexistent"));
    }

    #[test]
    fn test_board_has_app_handles_empty_board() {
        let items: Vec<serde_json::Value> = vec![];
        assert!(!board_has_app(&items, "any-app-id"));
    }

    #[test]
    fn test_board_has_app_handles_malformed_items() {
        let items = vec![
            json!({"id": "item-without-options"}),
            json!({"id": "item-with-empty-options", "options": {}}),
            json!({"id": "item-with-null-appid", "options": {"appId": null}}),
        ];

        // Should not crash and should return false for all
        assert!(!board_has_app(&items, "any-app-id"));
    }

    // Tests for derive_ping_url (auto-derive host.docker.internal URL for health checks)

    #[test]
    fn test_derive_ping_url_replaces_hostname() {
        let result = derive_ping_url("http://halos.local:3000");
        assert_eq!(
            result,
            Some("http://host.docker.internal:3000/".to_string())
        );
    }

    #[test]
    fn test_derive_ping_url_preserves_path() {
        let result = derive_ping_url("http://halos.local:8086/api/v2");
        assert_eq!(
            result,
            Some("http://host.docker.internal:8086/api/v2".to_string())
        );
    }

    #[test]
    fn test_derive_ping_url_preserves_https() {
        let result = derive_ping_url("https://halos.local:443/app");
        assert_eq!(result, Some("https://host.docker.internal/app".to_string()));
    }

    #[test]
    fn test_derive_ping_url_handles_no_port() {
        let result = derive_ping_url("http://halos.local/dashboard");
        assert_eq!(
            result,
            Some("http://host.docker.internal/dashboard".to_string())
        );
    }

    #[test]
    fn test_derive_ping_url_invalid_url_returns_none() {
        let result = derive_ping_url("not-a-valid-url");
        assert_eq!(result, None);
    }
}
