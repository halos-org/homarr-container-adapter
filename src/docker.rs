//! Docker container discovery

use bollard::Docker;
use bollard::container::ListContainersOptions;
use std::collections::HashMap;

use crate::config::Config;
use crate::error::{AdapterError, Result};

/// Discovered app from Docker labels
#[derive(Debug, Clone)]
pub struct DiscoveredApp {
    pub container_id: String,
    pub container_name: String,
    pub name: String,
    pub description: Option<String>,
    pub url: String,
    pub icon_url: Option<String>,
    pub category: Option<String>,
}

/// Discover apps from Docker containers with homarr.* labels
pub async fn discover_apps(config: &Config) -> Result<Vec<DiscoveredApp>> {
    let docker = Docker::connect_with_socket(&config.docker_socket, 120, bollard::API_DEFAULT_VERSION)
        .map_err(|e| AdapterError::Docker(format!("Failed to connect to Docker: {}", e)))?;

    let options = ListContainersOptions::<String> {
        all: false, // Only running containers
        ..Default::default()
    };

    let containers = docker
        .list_containers(Some(options))
        .await
        .map_err(|e| AdapterError::Docker(format!("Failed to list containers: {}", e)))?;

    let mut apps = Vec::new();

    for container in containers {
        if let Some(labels) = container.labels {
            // Check if this container has homarr.enable=true
            if labels.get("homarr.enable") == Some(&"true".to_string()) {
                if let Some(app) = parse_homarr_labels(&container.id.unwrap_or_default(), &labels) {
                    tracing::debug!("Discovered app: {:?}", app);
                    apps.push(app);
                }
            }
        }
    }

    tracing::info!("Discovered {} apps from Docker containers", apps.len());
    Ok(apps)
}

/// Parse homarr.* labels from a container
fn parse_homarr_labels(container_id: &str, labels: &HashMap<String, String>) -> Option<DiscoveredApp> {
    // Required labels
    let name = labels.get("homarr.name")?;
    let url = labels.get("homarr.url")?;

    // Get container name from labels or use a default
    let container_name = labels
        .get("com.docker.compose.service")
        .cloned()
        .unwrap_or_else(|| container_id[..12].to_string());

    Some(DiscoveredApp {
        container_id: container_id.to_string(),
        container_name,
        name: name.clone(),
        description: labels.get("homarr.description").cloned(),
        url: url.clone(),
        icon_url: labels.get("homarr.icon").cloned(),
        category: labels.get("homarr.category").cloned(),
    })
}
