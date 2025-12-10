//! Docker container discovery and event monitoring

use bollard::container::{InspectContainerOptions, ListContainersOptions};
use bollard::system::EventsOptions;
use bollard::Docker;
use futures_util::StreamExt;
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::error::{AdapterError, Result};

/// Discovered app from Docker labels
#[derive(Debug, Clone)]
#[allow(dead_code)]
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
    let docker =
        Docker::connect_with_socket(&config.docker_socket, 120, bollard::API_DEFAULT_VERSION)
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
fn parse_homarr_labels(
    container_id: &str,
    labels: &HashMap<String, String>,
) -> Option<DiscoveredApp> {
    // Required labels
    let name = labels.get("homarr.name")?;
    let url = labels.get("homarr.url")?;

    // Get container name from labels or use a default
    let container_name = labels
        .get("com.docker.compose.service")
        .cloned()
        .unwrap_or_else(|| {
            if container_id.len() >= 12 {
                container_id[..12].to_string()
            } else {
                container_id.to_string()
            }
        });

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

/// Docker event types we care about
#[derive(Debug, Clone)]
pub enum ContainerEvent {
    Started(DiscoveredApp),
    Stopped(String), // container_id
}

/// Get app info from a specific container by ID
pub async fn get_container_app(
    config: &Config,
    container_id: &str,
) -> Result<Option<DiscoveredApp>> {
    let docker =
        Docker::connect_with_socket(&config.docker_socket, 120, bollard::API_DEFAULT_VERSION)
            .map_err(|e| AdapterError::Docker(format!("Failed to connect to Docker: {}", e)))?;

    let container = docker
        .inspect_container(container_id, None::<InspectContainerOptions>)
        .await
        .map_err(|e| AdapterError::Docker(format!("Failed to inspect container: {}", e)))?;

    let labels = container.config.and_then(|c| c.labels).unwrap_or_default();

    // Check if this container has homarr.enable=true
    if labels.get("homarr.enable") == Some(&"true".to_string()) {
        Ok(parse_homarr_labels(container_id, &labels))
    } else {
        Ok(None)
    }
}

/// Watch Docker events and send container start/stop events
pub async fn watch_events(config: &Config, tx: mpsc::Sender<ContainerEvent>) -> Result<()> {
    let docker =
        Docker::connect_with_socket(&config.docker_socket, 120, bollard::API_DEFAULT_VERSION)
            .map_err(|e| AdapterError::Docker(format!("Failed to connect to Docker: {}", e)))?;

    // Filter for container events only
    let mut filters = HashMap::new();
    filters.insert("type".to_string(), vec!["container".to_string()]);
    filters.insert(
        "event".to_string(),
        vec!["start".to_string(), "stop".to_string(), "die".to_string()],
    );

    let options = EventsOptions {
        filters,
        ..Default::default()
    };

    tracing::info!("Starting Docker event monitoring");
    let mut events = docker.events(Some(options));

    while let Some(event_result) = events.next().await {
        match event_result {
            Ok(event) => {
                let action = event.action.as_deref().unwrap_or("");
                let container_id = event
                    .actor
                    .as_ref()
                    .and_then(|a| a.id.as_ref())
                    .map(|s| s.as_str())
                    .unwrap_or("");

                if container_id.is_empty() {
                    continue;
                }

                tracing::debug!(
                    "Docker event: {} for container {}",
                    action,
                    &container_id[..12.min(container_id.len())]
                );

                match action {
                    "start" => {
                        // Container started - check if it has homarr labels
                        match get_container_app(config, container_id).await {
                            Ok(Some(app)) => {
                                tracing::info!(
                                    "Container started with homarr labels: {}",
                                    app.name
                                );
                                if tx.send(ContainerEvent::Started(app)).await.is_err() {
                                    tracing::error!("Failed to send event - channel closed");
                                    break;
                                }
                            }
                            Ok(None) => {
                                // Container doesn't have homarr labels, ignore
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to inspect container {}: {}",
                                    container_id,
                                    e
                                );
                            }
                        }
                    }
                    "stop" | "die" => {
                        // Container stopped
                        if tx
                            .send(ContainerEvent::Stopped(container_id.to_string()))
                            .await
                            .is_err()
                        {
                            tracing::error!("Failed to send event - channel closed");
                            break;
                        }
                    }
                    _ => {}
                }
            }
            Err(e) => {
                tracing::error!("Docker event stream error: {}", e);
                // Continue watching - don't exit on transient errors
            }
        }
    }

    tracing::warn!("Docker event stream ended");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_labels(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn test_parse_homarr_labels_all_fields() {
        let labels = make_labels(&[
            ("homarr.name", "My App"),
            ("homarr.url", "http://localhost:8080"),
            ("homarr.description", "Test application"),
            ("homarr.icon", "https://example.com/icon.png"),
            ("homarr.category", "Development"),
            ("com.docker.compose.service", "myapp"),
        ]);

        let app = parse_homarr_labels("abc123def456", &labels).unwrap();
        assert_eq!(app.name, "My App");
        assert_eq!(app.url, "http://localhost:8080");
        assert_eq!(app.description, Some("Test application".to_string()));
        assert_eq!(
            app.icon_url,
            Some("https://example.com/icon.png".to_string())
        );
        assert_eq!(app.category, Some("Development".to_string()));
        assert_eq!(app.container_name, "myapp");
        assert_eq!(app.container_id, "abc123def456");
    }

    #[test]
    fn test_parse_homarr_labels_required_only() {
        let labels = make_labels(&[
            ("homarr.name", "Minimal App"),
            ("homarr.url", "http://localhost:3000"),
        ]);

        let app = parse_homarr_labels("abcdef123456789", &labels).unwrap();
        assert_eq!(app.name, "Minimal App");
        assert_eq!(app.url, "http://localhost:3000");
        assert_eq!(app.description, None);
        assert_eq!(app.icon_url, None);
        assert_eq!(app.category, None);
        // Container name should be truncated container ID
        assert_eq!(app.container_name, "abcdef123456");
    }

    #[test]
    fn test_parse_homarr_labels_missing_name() {
        let labels = make_labels(&[("homarr.url", "http://localhost:8080")]);

        let result = parse_homarr_labels("container123", &labels);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_homarr_labels_missing_url() {
        let labels = make_labels(&[("homarr.name", "App Without URL")]);

        let result = parse_homarr_labels("container123", &labels);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_homarr_labels_short_container_id() {
        let labels = make_labels(&[("homarr.name", "Test"), ("homarr.url", "http://test")]);

        // Container ID shorter than 12 chars should be used as-is
        let app = parse_homarr_labels("short", &labels).unwrap();
        assert_eq!(app.container_name, "short");
    }

    #[test]
    fn test_parse_homarr_labels_compose_service_overrides_id() {
        let labels = make_labels(&[
            ("homarr.name", "Test"),
            ("homarr.url", "http://test"),
            ("com.docker.compose.service", "custom-service"),
        ]);

        let app = parse_homarr_labels("abcdef123456789", &labels).unwrap();
        // Compose service name should be used instead of container ID
        assert_eq!(app.container_name, "custom-service");
    }
}
