//! Homarr Container Adapter
//!
//! This service provides:
//! - First-boot setup: Completes Homarr onboarding with HaLOS branding
//! - App registry: Syncs apps from /etc/halos/webapps.d/ to Homarr dashboard
//! - Watch mode: Daemon that monitors Docker events and syncs on changes

mod authelia;
mod branding;
mod config;
mod error;
mod homarr;
mod registry;
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

/// Default board ID for removal tracking during single-board operation.
/// TODO: Remove this when multi-board sync is implemented (#33).
const DEFAULT_BOARD_ID: &str = "default";

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

    // Load branding
    let branding = branding::BrandingConfig::load(&config.branding_file)?;

    // Create client and set up authentication
    let mut client = homarr::HomarrClient::new(&config.homarr_url)?;
    ensure_authenticated(&mut client, config, &mut state).await?;

    // Pre-fetch existing apps for efficient deduplication
    let existing_apps = client.get_all_apps().await.unwrap_or_else(|e| {
        warn!("Failed to fetch existing apps: {}", e);
        vec![]
    });

    // Load and sync registry apps
    info!("Loading apps from registry: {}", config.registry_dir);
    let registry_apps = registry::load_all_apps(&config.registry_dir).unwrap_or_else(|e| {
        warn!("Failed to load registry apps: {}", e);
        vec![]
    });

    let mut synced_count = 0;
    for entry in &registry_apps {
        // TODO: Replace with per-board checking when multi-board sync is implemented (#33)
        if state.is_removed_from_board(DEFAULT_BOARD_ID, &entry.app.url) {
            info!(
                "Registry app '{}' was removed by user, skipping",
                entry.app.name
            );
            continue;
        }

        match client
            .add_registry_app(&entry.app, &branding.board.name, Some(&existing_apps))
            .await
        {
            Ok(_) => {
                // Track in state (use empty container_id for non-container apps)
                let container_id = entry.app.container_name().unwrap_or("").to_string();
                state.discovered_apps.insert(
                    entry.app.url.clone(),
                    state::DiscoveredApp {
                        name: entry.app.name.clone(),
                        container_id,
                        added_at: chrono::Utc::now(),
                    },
                );
                synced_count += 1;
            }
            Err(e) => {
                warn!("Failed to add registry app '{}': {}", entry.app.name, e);
            }
        }
    }

    state.update_sync_time();
    state.save(&config.state_file)?;

    info!("Sync complete: {} apps synced from registry", synced_count);
    Ok(())
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

    // Sync credentials to Authelia if not already done
    if !state.authelia_sync_completed {
        sync_authelia_credentials(config, &branding, &mut state)?;
    }

    // Mark first boot complete
    state.first_boot_completed = true;
    state.save(&config.state_file)?;

    info!("First-boot setup complete");
    Ok(())
}

/// Sync credentials from branding to Authelia user database
fn sync_authelia_credentials(
    config: &Config,
    branding: &branding::BrandingConfig,
    state: &mut state::State,
) -> Result<()> {
    use std::path::Path;

    let db_path = Path::new(&config.authelia_users_db);

    // Only sync if the parent directory exists (Authelia is installed)
    if let Some(parent) = db_path.parent() {
        if parent.exists() {
            info!("Authelia detected, syncing credentials");

            match authelia::sync_credentials(
                db_path,
                &branding.credentials.admin_username,
                &branding.credentials.admin_password,
                None, // Use default email
            ) {
                Ok(()) => {
                    state.authelia_sync_completed = true;
                    info!("Authelia credential sync completed");
                }
                Err(e) => {
                    warn!("Failed to sync Authelia credentials: {}", e);
                    // Don't fail setup if Authelia sync fails
                }
            }
        } else {
            info!(
                "Authelia not installed (directory {} does not exist), skipping credential sync",
                parent.display()
            );
        }
    }

    Ok(())
}

async fn check_status(config: &Config) -> Result<()> {
    let state = state::State::load(&config.state_file).unwrap_or_default();

    if state.first_boot_completed {
        println!("Status: First-boot setup completed");
        println!(
            "Authelia sync: {}",
            if state.authelia_sync_completed {
                "completed"
            } else {
                "not completed"
            }
        );
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
