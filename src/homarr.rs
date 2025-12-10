//! Homarr API client

use reqwest::{Client, cookie::Jar};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::branding::BrandingConfig;
use crate::config::Config;
use crate::docker::DiscoveredApp;
use crate::error::{AdapterError, Result};

/// Homarr API client
pub struct HomarrClient {
    client: Client,
    base_url: String,
}

#[derive(Debug, Deserialize)]
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
        let response: TrpcResponse<OnboardingStep> = self.client.get(&url).send().await?.json().await?;
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
        self.set_color_scheme(&branding.theme.default_color_scheme).await?;

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

        let section_id = board.sections.first().map(|s| s.id.clone()).unwrap_or_default();
        let layout_id = board.layouts.first().map(|l| l.id.clone()).unwrap_or_default();

        let cockpit = &branding.board.cockpit;

        let url = format!("{}/api/trpc/board.saveBoard", self.base_url);
        let payload = json!({
            "json": {
                "id": board_id,
                "sections": board.sections,
                "items": [{
                    "id": format!("cockpit-{}", app_id),
                    "kind": "app",
                    "appId": app_id,
                    "options": {},
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
}

/// Sync discovered apps with Homarr
pub async fn sync_apps(_config: &Config, _apps: &[DiscoveredApp]) -> Result<()> {
    // TODO: Implement full sync logic
    // - Get current apps from Homarr
    // - Compare with discovered apps
    // - Add new apps, skip removed apps
    tracing::info!("App sync not yet implemented");
    Ok(())
}
