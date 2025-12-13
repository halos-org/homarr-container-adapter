//! Homarr API client

use reqwest::{cookie::Jar, Client};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::branding::BrandingConfig;
use crate::docker::DiscoveredApp;
use crate::error::{AdapterError, Result};

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

impl HomarrClient {
    /// Create a new Homarr client
    pub fn new(base_url: &str) -> Result<Self> {
        let jar = Arc::new(Jar::default());
        let client = Client::builder()
            .cookie_store(true)
            .cookie_provider(jar)
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

    /// Set up default board with Cockpit tile
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

        // Create Cockpit app if it doesn't exist
        if branding.board.cockpit.enabled {
            self.ensure_cockpit_app(branding, &board_id).await?;
        }

        // Set as home board
        self.set_home_board(&board_id).await?;

        // Set color scheme
        self.set_color_scheme(&branding.theme.default_color_scheme)
            .await?;

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

    /// Ensure Cockpit app exists and is on the board
    async fn ensure_cockpit_app(&self, branding: &BrandingConfig, board_id: &str) -> Result<()> {
        let cockpit = &branding.board.cockpit;

        // Create app
        let url = format!("{}/api/trpc/app.create", self.base_url);
        let payload = json!({
            "json": {
                "name": cockpit.name,
                "description": cockpit.description,
                "iconUrl": cockpit.icon_url,
                "href": cockpit.href,
                "pingUrl": null
            }
        });

        let response = self.client.post(&url).json(&payload).send().await?;

        if response.status().is_success() {
            let app_response: TrpcResponse<CreateAppResponse> = response.json().await?;
            let app_id = app_response.result.data.json.app_id;

            // Add to board
            self.add_app_to_board(board_id, &app_id, branding).await?;
        }

        Ok(())
    }

    /// Add an app to a board
    async fn add_app_to_board(
        &self,
        board_id: &str,
        app_id: &str,
        branding: &BrandingConfig,
    ) -> Result<()> {
        // Get current board state
        let board = self.get_board_by_name(&branding.board.name).await?;

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

        let cockpit = &branding.board.cockpit;

        let url = format!("{}/api/trpc/board.saveBoard", self.base_url);
        let payload = json!({
            "json": {
                "id": board_id,
                "sections": board.sections,
                "items": [{
                    "id": format!("cockpit-{}", app_id),
                    "kind": "app",
                    "options": {
                        "appId": app_id
                    },
                    "layouts": [{
                        "layoutId": layout_id,
                        "sectionId": section_id,
                        "width": cockpit.width,
                        "height": cockpit.height,
                        "xOffset": cockpit.x_offset,
                        "yOffset": cockpit.y_offset
                    }],
                    "integrationIds": [],
                    "advancedOptions": {
                        "customCssClasses": []
                    }
                }],
                "integrations": []
            }
        });

        self.client.post(&url).json(&payload).send().await?;
        Ok(())
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

    /// Add a discovered app to Homarr
    pub async fn add_discovered_app(
        &self,
        app: &DiscoveredApp,
        board_name: &str,
    ) -> Result<String> {
        // Create the app in Homarr
        let url = format!("{}/api/trpc/app.create", self.base_url);
        // Default to Docker icon if no icon specified
        let default_icon = "https://cdn.jsdelivr.net/gh/walkxcode/dashboard-icons/svg/docker.svg";
        let icon_url = app
            .icon_url
            .clone()
            .unwrap_or_else(|| default_icon.to_string());

        let payload = json!({
            "json": {
                "name": app.name,
                "description": app.description.clone().unwrap_or_default(),
                "iconUrl": icon_url,
                "href": app.url,
                "pingUrl": null
            }
        });

        let response = self.client.post(&url).json(&payload).send().await?;

        if !response.status().is_success() {
            let text = response.text().await?;
            return Err(AdapterError::HomarrApi(format!(
                "Failed to create app '{}': {}",
                app.name, text
            )));
        }

        let app_response: TrpcResponse<CreateAppResponse> = response.json().await?;
        let app_id = app_response.result.data.json.app_id;

        // Add to board
        self.add_discovered_app_to_board(&app_id, app, board_name)
            .await?;

        tracing::info!("Added app '{}' to Homarr (app_id: {})", app.name, app_id);
        Ok(app_id)
    }

    /// Add a discovered app to a board with auto-positioning
    async fn add_discovered_app_to_board(
        &self,
        app_id: &str,
        app: &DiscoveredApp,
        board_name: &str,
    ) -> Result<()> {
        // Get current board state
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

        // Get existing items to find next available position
        let board_items = self.get_board_items(board_name).await.unwrap_or_default();
        let (x_offset, y_offset) = self.find_next_position(&board_items, 10); // 10 columns

        let url = format!("{}/api/trpc/board.saveBoard", self.base_url);

        // Build items list with existing items plus the new one
        let mut items: Vec<serde_json::Value> = board_items;
        items.push(json!({
            "id": format!("discovered-{}", app.container_id),
            "kind": "app",
            "options": {
                "appId": app_id
            },
            "layouts": [{
                "layoutId": layout_id,
                "sectionId": section_id,
                "width": 1,
                "height": 1,
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
}
