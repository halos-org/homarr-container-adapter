//! Signal K webapp discovery
//!
//! Discovers webapps installed in Signal K server by querying its REST API,
//! and converts them to `AppDefinition` objects for syncing to Homarr.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use crate::registry::{AppDefinition, AppType, LayoutConfig};

/// Webapps to exclude from Homarr (same filter as SK's own webapps page)
const EXCLUDED_WEBAPPS: &[&str] = &["@signalk/server-admin-ui"];

/// Default icon for webapps that don't provide one
const DEFAULT_ICON: &str = "/icons/docker.svg";

/// Traefik path prefix for Signal K server
const SIGNALK_PATH_PREFIX: &str = "/signalk-server";

/// Return the canonical SK-webapp path identity for a URL, or `None` if the URL
/// is not a Signal K webapp URL.
///
/// The identity spans absolute and path-only URL forms so callers can compare
/// "is this the same SK webapp?" across the URL-format transition introduced
/// when adapter cards moved from absolute URLs to path-only hrefs.
///
/// Identity is the sub-path after `/signalk-server/`, with a leading slash and
/// no trailing slash. Example:
/// - `"https://host/signalk-server/@signalk/freeboard-sk/"` → `Some("/@signalk/freeboard-sk")`
/// - `"/signalk-server/@signalk/freeboard-sk"`             → `Some("/@signalk/freeboard-sk")`
/// - `"https://host/signalk-server/"` (SK Server tile)     → `None`
/// - `"https://host/grafana/"` or `"/cockpit/"`            → `None`
///
/// SK package names are case-sensitive, so identity comparison is
/// case-sensitive.
pub fn signalk_webapp_identity(url: &str) -> Option<String> {
    // Find the part after the SK path prefix. `split` is used (rather than
    // `url::Url::parse`) so this works for both absolute and path-only inputs.
    let rest = url.split(SIGNALK_PATH_PREFIX).nth(1)?;

    // Strip any query/fragment so identity is path-only.
    let path = rest.split(['?', '#']).next().unwrap_or(rest);

    // Trim slashes; if nothing remains, this is the SK Server tile, not a webapp.
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }

    Some(format!("/{}", trimmed))
}

// --- Signal K API response types ---

/// Entry from GET /skServer/webapps
#[derive(Debug, Deserialize)]
struct SkWebapp {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    signalk: Option<SkMetadata>,
}

#[derive(Debug, Deserialize)]
struct SkMetadata {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "appIcon")]
    app_icon: Option<String>,
}

/// Entry from GET /signalk/v1/apps/list
#[derive(Debug, Deserialize)]
struct SkAppListEntry {
    name: String,
    location: String,
}

/// Strip leading `./` from a relative path
fn strip_dot_slash(path: &str) -> &str {
    path.strip_prefix("./").unwrap_or(path)
}

/// Build the icon URL accessible from the user's browser via Traefik.
///
/// Path-only so the browser resolves it against whichever hostname the user
/// is currently using (mDNS, VPN FQDN, DHCP-DNS), matching the multi-hostname
/// access model used by `build_webapp_url`.
///
/// Example: `/signalk-server/@signalk/freeboard-sk/assets/icons/icon-72x72.png`
fn build_icon_url(package_name: &str, app_icon: Option<&str>) -> String {
    match app_icon {
        Some(icon) if !icon.is_empty() => {
            let icon = strip_dot_slash(icon).trim_start_matches('/');
            format!(
                "{}/{}/{}",
                SIGNALK_PATH_PREFIX,
                package_name.trim_matches('/'),
                icon
            )
        }
        _ => DEFAULT_ICON.to_string(),
    }
}

/// Build the user-facing path-only URL through Traefik path-prefix redirect.
///
/// Returning a path-only URL lets the browser resolve it against whichever
/// hostname the user is currently using (mDNS, VPN FQDN, DHCP-DNS), which is
/// the multi-hostname access goal.
///
/// Example: `/signalk-server/@signalk/freeboard-sk/`
fn build_webapp_url(location: &str) -> String {
    let location = location.trim_start_matches('/');
    format!("{}/{}", SIGNALK_PATH_PREFIX, location)
}

/// Build the ping URL for health checks from within Docker.
///
/// Example: `http://host.docker.internal:3000/@signalk/freeboard-sk/`
fn build_ping_url(location: &str) -> String {
    format!("http://host.docker.internal:3000{}", location)
}

/// Discover Signal K webapps and convert them to AppDefinitions.
///
/// Fetches both `/skServer/webapps` (for metadata) and `/signalk/v1/apps/list`
/// (for the location/mount path), joins on package name, and converts to
/// AppDefinition objects that the existing sync machinery can handle.
///
/// Returns `Some(apps)` on success (even if empty), `None` if SK is unreachable.
/// The distinction matters: `Some(vec![])` means "SK is up but has no webapps"
/// (safe to clean up stale entries), while `None` means "SK is down" (don't
/// remove anything).
pub async fn discover_webapps(signalk_url: &str) -> Option<Vec<AppDefinition>> {
    match discover_webapps_inner(signalk_url).await {
        Ok(apps) => {
            tracing::info!("Discovered {} Signal K webapp(s)", apps.len());
            Some(apps)
        }
        Err(e) => {
            tracing::warn!("Signal K webapp discovery failed: {}", e);
            None
        }
    }
}

async fn discover_webapps_inner(
    signalk_url: &str,
) -> std::result::Result<Vec<AppDefinition>, Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let base = signalk_url.trim_end_matches('/');

    // Fetch both endpoints concurrently
    let (webapps_resp, apps_list_resp) = tokio::join!(
        client.get(format!("{}/skServer/webapps", base)).send(),
        client.get(format!("{}/signalk/v1/apps/list", base)).send(),
    );

    let webapps: Vec<SkWebapp> = webapps_resp?.json().await?;
    let apps_list: Vec<SkAppListEntry> = apps_list_resp?.json().await?;

    // Build location lookup by package name
    let locations: HashMap<String, String> = apps_list
        .into_iter()
        .map(|entry| (entry.name, entry.location))
        .collect();

    let mut apps = Vec::new();

    for webapp in webapps {
        // Skip excluded webapps
        if EXCLUDED_WEBAPPS.contains(&webapp.name.as_str()) {
            continue;
        }

        // Need a location to build the URL
        let location = match locations.get(&webapp.name) {
            Some(loc) => loc.clone(),
            None => {
                tracing::debug!(
                    "Skipping Signal K webapp '{}': no location in apps/list",
                    webapp.name
                );
                continue;
            }
        };

        let sk_meta = webapp.signalk.as_ref();
        let display_name = sk_meta
            .and_then(|m| m.display_name.as_deref())
            .unwrap_or(&webapp.name);
        let app_icon = sk_meta.and_then(|m| m.app_icon.as_deref());

        apps.push(AppDefinition {
            name: display_name.to_string(),
            url: build_webapp_url(&location),
            description: webapp.description,
            icon_url: Some(build_icon_url(&webapp.name, app_icon)),
            category: Some("Marine".to_string()),
            visible: true,
            app_type: AppType {
                container_name: None,
                external: false,
            },
            ping_url: Some(build_ping_url(&location)),
            layout: LayoutConfig {
                priority: 45,
                width: 2,
                height: 1,
                x_offset: None,
                y_offset: None,
            },
        });
    }

    Ok(apps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_dot_slash() {
        assert_eq!(strip_dot_slash("./assets/icon.png"), "assets/icon.png");
        assert_eq!(strip_dot_slash("assets/icon.png"), "assets/icon.png");
        assert_eq!(strip_dot_slash("./icon.png"), "icon.png");
        assert_eq!(strip_dot_slash(""), "");
    }

    #[test]
    fn test_build_icon_url() {
        assert_eq!(
            build_icon_url(
                "@signalk/freeboard-sk",
                Some("./assets/icons/icon-72x72.png"),
            ),
            "/signalk-server/@signalk/freeboard-sk/assets/icons/icon-72x72.png"
        );

        assert_eq!(
            build_icon_url("@mxtommy/kip", Some("assets/icon-72x72.png")),
            "/signalk-server/@mxtommy/kip/assets/icon-72x72.png"
        );

        assert_eq!(build_icon_url("some-app", None), DEFAULT_ICON);
        assert_eq!(build_icon_url("some-app", Some("")), DEFAULT_ICON);

        // Leading-slash trim mirrors `build_webapp_url`: a SK manifest that
        // happens to start its appIcon with `/` must not produce `//` in the
        // emitted URL, which would survive past Traefik path matching as a
        // distinct path.
        assert_eq!(
            build_icon_url("some-app", Some("/assets/icon.png")),
            "/signalk-server/some-app/assets/icon.png"
        );
    }

    #[test]
    fn test_build_webapp_url_path_only() {
        assert_eq!(
            build_webapp_url("/@signalk/freeboard-sk/"),
            "/signalk-server/@signalk/freeboard-sk/"
        );
    }

    #[test]
    fn test_build_webapp_url_no_leading_slash() {
        assert_eq!(
            build_webapp_url("@signalk/freeboard-sk/"),
            "/signalk-server/@signalk/freeboard-sk/"
        );
    }

    #[test]
    fn test_build_ping_url() {
        assert_eq!(
            build_ping_url("/@signalk/freeboard-sk/"),
            "http://host.docker.internal:3000/@signalk/freeboard-sk/"
        );
    }

    #[tokio::test]
    async fn test_discover_webapps_unreachable_returns_none() {
        let result = discover_webapps("http://127.0.0.1:1").await;
        assert!(result.is_none());
    }

    #[test]
    fn test_signalk_webapp_identity_absolute_and_path_only_match() {
        let abs =
            signalk_webapp_identity("https://myhost.local/signalk-server/@signalk/freeboard-sk/");
        let path_only = signalk_webapp_identity("/signalk-server/@signalk/freeboard-sk/");
        assert!(abs.is_some());
        assert_eq!(abs, path_only);
        assert_eq!(abs.as_deref(), Some("/@signalk/freeboard-sk"));
    }

    #[test]
    fn test_signalk_webapp_identity_trailing_slash_irrelevant() {
        let with = signalk_webapp_identity("/signalk-server/@signalk/freeboard-sk/");
        let without = signalk_webapp_identity("/signalk-server/@signalk/freeboard-sk");
        assert_eq!(with, without);
    }

    #[test]
    fn test_signalk_webapp_identity_strips_query_and_fragment() {
        assert_eq!(
            signalk_webapp_identity("/signalk-server/@signalk/freeboard-sk/?foo=bar"),
            Some("/@signalk/freeboard-sk".to_string())
        );
        assert_eq!(
            signalk_webapp_identity("https://host/signalk-server/@signalk/freeboard-sk/#section"),
            Some("/@signalk/freeboard-sk".to_string())
        );
    }

    #[test]
    fn test_signalk_webapp_identity_rejects_sk_server_tile() {
        assert_eq!(
            signalk_webapp_identity("https://myhost.local/signalk-server/"),
            None
        );
        assert_eq!(signalk_webapp_identity("/signalk-server/"), None);
        assert_eq!(signalk_webapp_identity("/signalk-server"), None);
    }

    #[test]
    fn test_signalk_webapp_identity_rejects_non_sk_urls() {
        assert_eq!(signalk_webapp_identity("/cockpit/"), None);
        assert_eq!(signalk_webapp_identity("https://host/grafana/"), None);
        assert_eq!(signalk_webapp_identity(""), None);
        assert_eq!(signalk_webapp_identity("not-a-url"), None);
    }

    #[test]
    fn test_signalk_webapp_identity_case_sensitive() {
        // SK package names are case-sensitive; identity must reflect that.
        assert_ne!(
            signalk_webapp_identity("/signalk-server/@signalk/Freeboard-SK/"),
            signalk_webapp_identity("/signalk-server/@signalk/freeboard-sk/")
        );
    }

    #[test]
    fn test_signalk_webapp_identity_non_scoped_package() {
        assert_eq!(
            signalk_webapp_identity("https://myhost.local/signalk-server/some-webapp/"),
            Some("/some-webapp".to_string())
        );
    }

    #[test]
    fn test_conversion_from_api_data() {
        // Simulate what discover_webapps_inner does with test data
        let webapps = vec![
            SkWebapp {
                name: "@signalk/freeboard-sk".to_string(),
                description: Some("Chart plotter".to_string()),
                signalk: Some(SkMetadata {
                    display_name: Some("Freeboard-SK".to_string()),
                    app_icon: Some("./assets/icons/icon-72x72.png".to_string()),
                }),
            },
            SkWebapp {
                name: "@signalk/server-admin-ui".to_string(),
                description: Some("Admin UI".to_string()),
                signalk: Some(SkMetadata {
                    display_name: Some("Admin UI".to_string()),
                    app_icon: Some("./img/logo.svg".to_string()),
                }),
            },
            SkWebapp {
                name: "some-webapp-no-signalk-field".to_string(),
                description: None,
                signalk: None,
            },
        ];

        let locations: HashMap<String, String> = [
            (
                "@signalk/freeboard-sk".to_string(),
                "/@signalk/freeboard-sk/".to_string(),
            ),
            (
                "@signalk/server-admin-ui".to_string(),
                "/@signalk/server-admin-ui/".to_string(),
            ),
            (
                "some-webapp-no-signalk-field".to_string(),
                "/some-webapp-no-signalk-field/".to_string(),
            ),
        ]
        .into_iter()
        .collect();

        let mut apps = Vec::new();
        for webapp in webapps {
            if EXCLUDED_WEBAPPS.contains(&webapp.name.as_str()) {
                continue;
            }
            let location = match locations.get(&webapp.name) {
                Some(loc) => loc.clone(),
                None => continue,
            };
            let sk_meta = webapp.signalk.as_ref();
            let display_name = sk_meta
                .and_then(|m| m.display_name.as_deref())
                .unwrap_or(&webapp.name);
            let app_icon = sk_meta.and_then(|m| m.app_icon.as_deref());

            apps.push(AppDefinition {
                name: display_name.to_string(),
                url: build_webapp_url(&location),
                description: webapp.description,
                icon_url: Some(build_icon_url(&webapp.name, app_icon)),
                category: Some("Marine".to_string()),
                visible: true,
                app_type: AppType {
                    container_name: None,
                    external: false,
                },
                ping_url: Some(build_ping_url(&location)),
                layout: LayoutConfig {
                    priority: 45,
                    width: 2,
                    height: 1,
                    x_offset: None,
                    y_offset: None,
                },
            });
        }

        // admin-ui should be filtered out
        assert_eq!(apps.len(), 2);

        // Freeboard-SK uses displayName
        assert_eq!(apps[0].name, "Freeboard-SK");
        assert_eq!(apps[0].url, "/signalk-server/@signalk/freeboard-sk/");
        assert_eq!(
            apps[0].icon_url.as_deref(),
            Some("/signalk-server/@signalk/freeboard-sk/assets/icons/icon-72x72.png")
        );
        assert_eq!(apps[0].description, Some("Chart plotter".to_string()));

        // Webapp without signalk field falls back to package name and default icon
        assert_eq!(apps[1].name, "some-webapp-no-signalk-field");
        assert_eq!(apps[1].icon_url.as_deref(), Some(DEFAULT_ICON));
        assert_eq!(apps[1].description, None);

        // All should be visible, Marine category, priority 45
        for app in &apps {
            assert!(app.visible);
            assert_eq!(app.category, Some("Marine".to_string()));
            assert_eq!(app.layout.priority, 45);
            assert!(app.ping_url.is_some());
        }
    }
}
