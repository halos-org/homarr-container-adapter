//! Homarr Container Adapter
//!
//! This service provides:
//! - First-boot setup: Completes Homarr onboarding with HaLOS branding
//! - App registry: Syncs apps from /etc/halos/webapps.d/ to Homarr dashboard
//! - Watch mode: Daemon that monitors Docker events and syncs on changes

mod branding;
mod config;
mod error;
mod homarr;
mod registry;
mod signalk;
mod state;

use std::collections::HashMap;
use std::time::Duration;

use bollard::container::ListContainersOptions;
use bollard::system::EventsOptions;
use bollard::Docker;
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use tokio::time::{interval, sleep};
use tracing::{debug, error, info, warn, Level};
use tracing_subscriber::FmtSubscriber;

use crate::config::Config;
use crate::error::{AdapterError, Result};

#[derive(Parser)]
#[command(name = "homarr-container-adapter")]
#[command(about = "Adapter for Homarr dashboard: first-boot setup and app registry sync")]
#[command(version)]
struct Cli {
    /// Config file path
    #[arg(
        short,
        long,
        default_value = "/etc/homarr-container-adapter/config.toml"
    )]
    config: String,

    /// Enable debug logging
    #[arg(short, long)]
    debug: bool,

    /// Reset state before running command
    ///
    /// Clears all persistent state including API key, sync history, and
    /// removal tracking. Useful for testing or recovering from corrupted state.
    #[arg(long)]
    reset_state: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a sync cycle (load registry and sync to Homarr)
    Sync,

    /// Run first-boot setup only
    Setup,

    /// Check adapter status
    Status,

    /// Watch for Docker events and sync continuously (daemon mode)
    Watch,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up logging
    let level = if cli.debug { Level::DEBUG } else { Level::INFO };
    let subscriber = FmtSubscriber::builder()
        .with_max_level(level)
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    // Load config
    let config = Config::load(&cli.config)?;

    // Handle --reset-state flag
    if cli.reset_state {
        reset_state(&config)?;
    }

    match cli.command {
        Commands::Sync => {
            info!("Running sync cycle");
            run_sync(&config).await?;
        }
        Commands::Setup => {
            info!("Running first-boot setup");
            run_setup(&config).await?;
        }
        Commands::Status => {
            check_status(&config).await?;
        }
        Commands::Watch => {
            info!("Starting watch mode (daemon)");
            run_watch(&config).await?;
        }
    }

    Ok(())
}

async fn run_sync(config: &Config) -> Result<()> {
    // Check if first-boot setup is needed
    let mut state = state::State::load(&config.state_file)?;

    if !state.first_boot_completed {
        info!("First boot detected, running setup");
        run_setup(config).await?;
        // Reload state after setup (it saved first_boot_completed = true)
        state = state::State::load(&config.state_file)?;
    }

    // One-shot housekeeping: collapse discovered_apps duplicates that
    // accumulated when the same logical app was synced under multiple URL
    // forms (e.g., absolute URL on the first run, path-only after upgrade).
    // The end-of-run state.save persists the pruned map together with
    // sync_time, so no separate save here.
    let pruned = state.prune_duplicate_discovered_apps();
    if pruned > 0 {
        info!(
            "State prune: collapsed {} duplicate discovered_apps entries",
            pruned
        );
    }

    // Create client and set up authentication
    let mut client = homarr::HomarrClient::new(&config.homarr_url)?;
    ensure_authenticated(&mut client, config, &mut state).await?;

    // Discover writable boards
    let writable_boards = client.get_writable_boards().await.unwrap_or_else(|e| {
        warn!("Failed to fetch writable boards: {}", e);
        vec![]
    });

    if writable_boards.is_empty() {
        warn!("No writable boards found, skipping sync");
        return Ok(());
    }

    info!(
        "Found {} writable board(s): {}",
        writable_boards.len(),
        writable_boards
            .iter()
            .map(|b| b.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Pre-fetch existing apps for efficient deduplication
    let existing_apps = client.get_all_apps().await.unwrap_or_else(|e| {
        warn!("Failed to fetch existing apps: {}", e);
        vec![]
    });

    // Load registry apps
    info!("Loading apps from registry: {}", config.registry_dir);
    let registry_apps = registry::load_all_apps(&config.registry_dir).unwrap_or_else(|e| {
        warn!("Failed to load registry apps: {}", e);
        vec![]
    });

    // Discover Signal K webapps
    // Some(apps) = SK reachable (may be empty), None = SK unreachable
    let signalk_result = match config.signalk_url.as_deref() {
        Some(url) if !url.is_empty() => {
            info!("Discovering Signal K webapps from {}", url);
            signalk::discover_webapps(url).await
        }
        _ => {
            debug!("Signal K webapp discovery disabled (no signalk_url configured)");
            None
        }
    };
    let signalk_apps = signalk_result.as_deref().unwrap_or(&[]);

    // Collect all visible apps: registry (filtered) + Signal K (always visible)
    let visible_registry: Vec<_> = registry_apps
        .iter()
        .filter(|e| e.app.is_visible())
        .map(|e| &e.app)
        .collect();
    let hidden_count = registry_apps.len() - visible_registry.len();
    if hidden_count > 0 {
        debug!(
            "Filtered out {} hidden app(s) from {} total registry apps",
            hidden_count,
            registry_apps.len()
        );
    }

    let all_visible_apps: Vec<&registry::AppDefinition> = visible_registry
        .into_iter()
        .chain(signalk_apps.iter())
        .collect();

    // Sync each visible app to each writable board
    let mut synced_count = 0;
    for app in &all_visible_apps {
        // Track app in discovered_apps (once per app, not per board)
        let container_id = app.container_name().unwrap_or("").to_string();
        state.discovered_apps.insert(
            app.url.clone(),
            state::DiscoveredApp {
                name: app.name.clone(),
                container_id,
                added_at: chrono::Utc::now(),
            },
        );

        // Sync to each writable board
        for board in &writable_boards {
            // Check if app was removed from this specific board
            if state.is_removed_from_board(&board.id, &app.url) {
                debug!(
                    "App '{}' was removed from board '{}', skipping",
                    app.name, board.name
                );
                continue;
            }

            match client
                .add_registry_app(app, &board.name, Some(&existing_apps))
                .await
            {
                Ok(_) => {
                    synced_count += 1;
                }
                Err(e) => {
                    warn!(
                        "Failed to add app '{}' to board '{}': {}",
                        app.name, board.name, e
                    );
                }
            }
        }
    }

    // Clean up stale Signal K webapps (only when SK was reachable).
    //
    // Runs *after* the per-app sync loop so that any app row whose URL
    // form changed (e.g., absolute -> path-only) is updated in place by
    // sync first, preserving its appId. By the time cleanup runs, the
    // only state.json entries that look stale are genuinely-uninstalled
    // webapps.
    if signalk_result.is_some() {
        cleanup_stale_signalk_webapps(&client, &mut state, signalk_apps, &writable_boards).await;
    }

    state.update_sync_time();
    state.save(&config.state_file)?;

    info!(
        "Sync complete: {} visible app(s) ({} registry, {} Signal K), {} app-board combinations synced",
        all_visible_apps.len(),
        all_visible_apps.len() - signalk_apps.len(),
        signalk_apps.len(),
        synced_count
    );
    Ok(())
}

/// Compute the URLs in `discovered_apps` that look like SK webapps but
/// are no longer in `current_sk_identities`.
///
/// Pure function — extracted so the staleness rule is independently
/// testable from the orchestration in `cleanup_stale_signalk_webapps`.
fn compute_stale_sk_urls(
    discovered_apps: &std::collections::HashMap<String, state::DiscoveredApp>,
    current_sk_identities: &std::collections::HashSet<String>,
) -> Vec<String> {
    discovered_apps
        .iter()
        .filter(|(url, _)| {
            let Some(identity) = signalk::signalk_webapp_identity(url) else {
                return false;
            };
            !current_sk_identities.contains(&identity)
        })
        .map(|(url, _)| url.clone())
        .collect()
}

/// Remove stale Signal K webapps from Homarr and from `state.discovered_apps`.
///
/// Cascade-deletes the corresponding Homarr app row (and any board items
/// pointing at it) for each stale URL. State entries are pruned only on
/// successful cascade; on failure the entry is retained so the next sync
/// can retry.
///
/// Recovery property: even if a prior sync's cascade left orphan items on
/// some board, the next sync's call here will re-find the app row (the
/// cascade is sweep-first, so the row is still there on partial failure)
/// and re-attempt the sweep idempotently. If the row is genuinely missing
/// — e.g., a human deleted it manually — the loop still calls the cascade
/// with whatever app_id resolution it can find via the snapshot, and only
/// prunes state when the call returns Ok.
async fn cleanup_stale_signalk_webapps(
    client: &homarr::HomarrClient,
    state: &mut state::State,
    signalk_apps: &[registry::AppDefinition],
    writable_boards: &[homarr::BoardWithPermission],
) {
    let current_sk_identities: std::collections::HashSet<String> = signalk_apps
        .iter()
        .filter_map(|a| signalk::signalk_webapp_identity(&a.url))
        .collect();

    let stale_urls = compute_stale_sk_urls(&state.discovered_apps, &current_sk_identities);
    if stale_urls.is_empty() {
        return;
    }

    // Re-fetch apps so we see post-sync state (URL forms updated). Bail
    // out of cleanup if the fetch fails — pruning state without a fresh
    // app list could orphan items, leak global app rows, or both.
    let mut apps_after_sync = match client.get_all_apps().await {
        Ok(apps) => apps,
        Err(e) => {
            warn!(
                "Skipping stale-SK cleanup: failed to refetch apps ({}). \
                 State entries retained for retry.",
                e
            );
            return;
        }
    };

    let board_names: Vec<&str> = writable_boards.iter().map(|b| b.name.as_str()).collect();

    for url in &stale_urls {
        let app_name = state
            .discovered_apps
            .get(url)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "unknown".to_string());

        // Match against the live snapshot using the same two-tier rule
        // the sync loop uses, so future changes to the matcher stay
        // consistent across both call sites.
        let existing_id =
            homarr::HomarrClient::find_app_by_url(&apps_after_sync, url).map(|a| a.id.clone());

        let cascade_result = match &existing_id {
            Some(id) => client.delete_app_and_orphan_items(id, &board_names).await,
            None => {
                // App row is already gone in Homarr (manual cleanup,
                // prior partial cascade, etc.). Treat as success — the
                // sweep-first cascade ordering means a partial-failure
                // retry sees `existing_id.is_some()` and re-sweeps; an
                // app-already-gone state has nothing left to sweep
                // because the prior successful cascade already swept
                // before deleting.
                Ok(())
            }
        };

        match cascade_result {
            Ok(_) => {
                state.discovered_apps.remove(url);
                if let Some(id) = existing_id {
                    // Drop the matched app from the local snapshot so a
                    // subsequent stale URL in this loop that resolves to
                    // the same row does not re-target a now-deleted id.
                    apps_after_sync.retain(|a| a.id != id);
                }
                info!(
                    "Removed stale Signal K webapp '{}' from Homarr and discovered apps",
                    app_name
                );
            }
            Err(e) => {
                // Keep the state entry so the next sync retries the
                // cascade. Sweep-first ordering means the global app row
                // is still present on partial failure, so the retry will
                // re-find it via find_app_by_url and re-attempt.
                warn!(
                    "Failed to remove stale webapp '{}': {} \
                     (state entry kept for retry)",
                    app_name, e
                );
            }
        }
    }
}

/// Ensure the Homarr client is authenticated with a valid API key.
///
/// If a permanent API key is stored in state, use it.
/// Otherwise, rotate from the bootstrap API key to a new permanent key.
async fn ensure_authenticated(
    client: &mut homarr::HomarrClient,
    config: &Config,
    state: &mut state::State,
) -> Result<()> {
    use std::fs;

    // Check if we already have a permanent API key
    if let Some(ref api_key) = state.api_key {
        info!("Using stored API key for authentication");
        client.set_api_key(api_key.clone());
        return Ok(());
    }

    // No permanent key - need to rotate from bootstrap key
    info!("No permanent API key found, rotating from bootstrap key");

    // Read bootstrap key from file
    let bootstrap_key = fs::read_to_string(&config.bootstrap_api_key_file)
        .map_err(|e| {
            AdapterError::Config(format!(
                "Failed to read bootstrap API key from {}: {}",
                config.bootstrap_api_key_file, e
            ))
        })?
        .trim()
        .to_string();

    if bootstrap_key.is_empty() {
        return Err(AdapterError::Config(
            "Bootstrap API key file is empty".to_string(),
        ));
    }

    // Rotate to permanent key
    let permanent_key = client.rotate_api_key(&bootstrap_key).await?;

    // Save the permanent key to state
    state.api_key = Some(permanent_key.clone());
    state.save(&config.state_file)?;

    info!("API key rotation complete, permanent key saved to state");
    Ok(())
}

async fn run_setup(config: &Config) -> Result<()> {
    // Load branding config
    let branding = branding::BrandingConfig::load(&config.branding_file)?;

    // Create Homarr client
    let mut client = homarr::HomarrClient::new(&config.homarr_url)?;

    // Load state
    let mut state = state::State::load(&config.state_file).unwrap_or_default();

    // Ensure we have a valid API key (rotate from bootstrap if needed)
    ensure_authenticated(&mut client, config, &mut state).await?;

    // Check onboarding status (should already be complete from seed database)
    let step = client.get_onboarding_step().await?;
    info!("Current onboarding step: {:?}", step);

    if step.current != "finish" {
        info!("Completing onboarding");
        client.complete_onboarding(&branding).await?;
    }

    // Set up default board
    info!("Setting up default board");
    client.setup_default_board(&branding).await?;

    // Mark first boot complete
    state.first_boot_completed = true;
    state.save(&config.state_file)?;

    info!("First-boot setup complete");
    Ok(())
}

async fn check_status(config: &Config) -> Result<()> {
    let state = state::State::load(&config.state_file).unwrap_or_default();

    if state.first_boot_completed {
        println!("Status: First-boot setup completed");
        println!("Last sync: {:?}", state.last_sync);
        println!("Registered apps: {}", state.discovered_apps.len());
        for (url, app) in &state.discovered_apps {
            let container_info = if app.container_id.is_empty() {
                "external".to_string()
            } else {
                format!(
                    "container: {}",
                    &app.container_id[..12.min(app.container_id.len())]
                )
            };
            println!("  - {} ({}) [{}]", app.name, url, container_info);
        }
    } else {
        println!("Status: First-boot setup pending");
    }

    Ok(())
}

/// Reset adapter state to initial values
///
/// Removes the state file, clearing:
/// - API key (will be re-rotated from bootstrap key)
/// - First-boot completion flag (will re-run setup)
/// - Authelia sync flag
/// - Discovered apps tracking
/// - Removed apps tracking
/// - Last sync timestamp
fn reset_state(config: &Config) -> Result<()> {
    use std::path::Path;

    let state_path = Path::new(&config.state_file);

    if state_path.exists() {
        std::fs::remove_file(state_path)?;
        info!("State file removed: {}", config.state_file);
    } else {
        info!(
            "State file does not exist, nothing to reset: {}",
            config.state_file
        );
    }

    Ok(())
}

/// Watch mode: monitor Docker events and sync on changes
async fn run_watch(config: &Config) -> Result<()> {
    // Wait for startup delay to let Homarr start
    if config.startup_delay > 0 {
        info!(
            "Waiting {} seconds for Homarr to start...",
            config.startup_delay
        );
        sleep(Duration::from_secs(config.startup_delay)).await;
    }

    // Connect to Docker
    let docker = Docker::connect_with_socket(
        &config.docker_socket,
        120, // timeout in seconds
        bollard::API_DEFAULT_VERSION,
    )?;

    // Verify Docker connection
    match docker.ping().await {
        Ok(_) => info!("Connected to Docker daemon"),
        Err(e) => {
            error!("Failed to connect to Docker: {}", e);
            return Err(e.into());
        }
    }

    // Run initial sync with retry
    loop {
        match run_sync(config).await {
            Ok(_) => {
                info!("Initial sync completed successfully");
                break;
            }
            Err(e) => {
                warn!("Initial sync failed: {}. Retrying in 10 seconds...", e);
                sleep(Duration::from_secs(10)).await;
            }
        }
    }

    // Start watching Docker events and periodic sync
    info!(
        "Watching for Docker events, periodic sync every {} seconds",
        config.sync_interval
    );
    watch_loop(config, &docker).await
}

/// Main watch loop that handles Docker events and periodic syncs
async fn watch_loop(config: &Config, docker: &Docker) -> Result<()> {
    let mut sync_timer = interval(Duration::from_secs(config.sync_interval));
    // Skip the first immediate tick
    sync_timer.tick().await;

    // Set up Docker event stream with filter for container events
    let mut filters = HashMap::new();
    filters.insert("type", vec!["container"]);
    filters.insert("event", vec!["start", "stop", "die", "destroy"]);

    loop {
        // Create a fresh event stream for this iteration
        let options = EventsOptions {
            since: None,
            until: None,
            filters: filters.clone(),
        };
        let mut events = docker.events(Some(options));

        tokio::select! {
            // Handle Docker events
            Some(event_result) = events.next() => {
                match event_result {
                    Ok(event) => {
                        let action = event.action.as_deref().unwrap_or("unknown");
                        let actor = event.actor.as_ref();
                        let container_name = actor
                            .and_then(|a| a.attributes.as_ref())
                            .and_then(|attrs| attrs.get("name"))
                            .map(|s| s.as_str())
                            .unwrap_or("unknown");

                        info!("Docker event: {} container '{}'", action, container_name);

                        // Brief delay to let container fully start/stop
                        sleep(Duration::from_secs(2)).await;

                        // Trigger sync
                        if let Err(e) = run_sync(config).await {
                            warn!("Sync failed after Docker event: {}", e);
                        }
                    }
                    Err(e) => {
                        warn!("Docker event stream error: {}. Reconnecting...", e);
                        sleep(Duration::from_secs(5)).await;
                    }
                }
            }

            // Periodic sync timer
            _ = sync_timer.tick() => {
                debug!("Periodic sync triggered");
                if let Err(e) = run_sync(config).await {
                    warn!("Periodic sync failed: {}", e);
                }
            }
        }
    }
}

/// Get the current list of running containers (for debugging)
#[allow(dead_code)]
async fn list_containers(docker: &Docker) -> Result<Vec<String>> {
    let options = ListContainersOptions::<String> {
        all: false,
        ..Default::default()
    };

    let containers = docker.list_containers(Some(options)).await?;
    let names: Vec<String> = containers
        .iter()
        .filter_map(|c| c.names.as_ref())
        .flat_map(|names| names.iter())
        .map(|name| name.trim_start_matches('/').to_string())
        .collect();

    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn discovered(name: &str) -> state::DiscoveredApp {
        state::DiscoveredApp {
            name: name.to_string(),
            container_id: String::new(),
            added_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_compute_stale_sk_urls_excludes_non_sk() {
        let mut apps = HashMap::new();
        apps.insert("/cockpit/".to_string(), discovered("Cockpit"));
        apps.insert(
            "/signalk-server/@signalk/freeboard-sk/".to_string(),
            discovered("Freeboard-SK"),
        );
        let mut current = HashSet::new();
        current.insert("/@signalk/freeboard-sk".to_string());

        let stale = compute_stale_sk_urls(&apps, &current);
        // Cockpit is not an SK URL; the SK webapp's identity matches
        // current, so neither is stale.
        assert!(stale.is_empty());
    }

    #[test]
    fn test_compute_stale_sk_urls_matches_across_url_form() {
        // State holds absolute URL; current SK identities derived from
        // path-only URLs. The identity-based filter must consider this
        // entry "current", not stale.
        let mut apps = HashMap::new();
        apps.insert(
            "https://host.local/signalk-server/@signalk/freeboard-sk/".to_string(),
            discovered("Freeboard-SK"),
        );
        let mut current = HashSet::new();
        current.insert("/@signalk/freeboard-sk".to_string());

        assert!(compute_stale_sk_urls(&apps, &current).is_empty());
    }

    #[test]
    fn test_compute_stale_sk_urls_flags_uninstalled_webapp() {
        let mut apps = HashMap::new();
        apps.insert(
            "/signalk-server/@signalk/freeboard-sk/".to_string(),
            discovered("Freeboard-SK"),
        );
        apps.insert(
            "/signalk-server/@mxtommy/kip/".to_string(),
            discovered("KIP"),
        );
        // Only freeboard-sk is currently installed.
        let mut current = HashSet::new();
        current.insert("/@signalk/freeboard-sk".to_string());

        let stale = compute_stale_sk_urls(&apps, &current);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0], "/signalk-server/@mxtommy/kip/");
    }

    #[test]
    fn test_compute_stale_sk_urls_ignores_sk_server_tile() {
        // The bare /signalk-server/ path is the SK Server tile, not a
        // webapp; never stale per signalk_webapp_identity returning None.
        let mut apps = HashMap::new();
        apps.insert(
            "https://host.local/signalk-server/".to_string(),
            discovered("Signal K Server"),
        );
        let current = HashSet::new();
        assert!(compute_stale_sk_urls(&apps, &current).is_empty());
    }
}
