//! Homarr Container Adapter
//!
//! This service provides:
//! - First-boot setup: Completes Homarr onboarding with HaLOS branding
//! - Auto-discovery: Watches Docker containers and adds them to Homarr dashboard in real-time

mod branding;
mod config;
mod docker;
mod error;
mod homarr;
mod state;

use clap::{Parser, Subcommand};
use tokio::sync::mpsc;
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

use crate::config::Config;
use crate::docker::ContainerEvent;
use crate::error::Result;

#[derive(Parser)]
#[command(name = "homarr-container-adapter")]
#[command(about = "Adapter for Homarr dashboard: first-boot setup and auto-discovery")]
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
    /// Watch Docker events and sync containers in real-time (main daemon mode)
    Watch,

    /// Run a single sync cycle (scan all containers once)
    Sync,

    /// Run first-boot setup only
    Setup,

    /// Check adapter status
    Status,
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
        Commands::Watch => {
            info!("Starting watch mode");
            run_watch(&config).await?;
        }
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
    }

    Ok(())
}

/// Main watch loop - monitors Docker events and syncs in real-time
async fn run_watch(config: &Config) -> Result<()> {
    // Ensure first-boot setup is done
    let mut state = state::State::load(&config.state_file).unwrap_or_default();

    if !state.first_boot_completed {
        info!("First boot detected, running setup");
        run_setup(config).await?;
        state = state::State::load(&config.state_file).unwrap_or_default();
    }

    // Load branding for board name
    let branding = branding::BrandingConfig::load(&config.branding_file)?;

    // Create Homarr client and login
    let client = homarr::HomarrClient::new(&config.homarr_url)?;
    client.ensure_logged_in(&branding).await?;

    // Do initial sync of existing containers
    info!("Initial sync of existing containers");
    let discovered = docker::discover_apps(config).await?;
    // Pre-fetch existing apps for efficient deduplication
    let existing_apps = client.get_all_apps().await.unwrap_or_else(|e| {
        warn!("Failed to fetch existing apps: {}", e);
        vec![]
    });
    for app in &discovered {
        // Use URL as key for deduplication (container_id changes on restart)
        if !state.discovered_apps.contains_key(&app.url) {
            match client
                .add_discovered_app(app, &branding.board.name, Some(&existing_apps))
                .await
            {
                Ok(_) => {
                    state.discovered_apps.insert(
                        app.url.clone(),
                        state::DiscoveredApp {
                            name: app.name.clone(),
                            container_id: app.container_id.clone(),
                            added_at: chrono::Utc::now(),
                        },
                    );
                    state.save(&config.state_file)?;
                }
                Err(e) => {
                    warn!("Failed to add app '{}': {}", app.name, e);
                }
            }
        }
    }

    // Start watching Docker events
    let (tx, mut rx) = mpsc::channel::<ContainerEvent>(32);

    // Spawn event watcher task
    let watch_config = config.clone();
    tokio::spawn(async move {
        if let Err(e) = docker::watch_events(&watch_config, tx).await {
            tracing::error!("Docker event watcher failed: {}", e);
        }
    });

    // Process events
    info!("Watching for Docker container events...");
    while let Some(event) = rx.recv().await {
        match event {
            ContainerEvent::Started(app) => {
                // Check if already tracked (by URL, not container_id)
                if state.discovered_apps.contains_key(&app.url) {
                    info!("App '{}' already tracked, skipping", app.name);
                    continue;
                }

                // Check if user removed it (by URL)
                if state.is_removed(&app.url) {
                    info!("App '{}' was removed by user, skipping", app.name);
                    continue;
                }

                // Re-login in case session expired
                if let Err(e) = client.ensure_logged_in(&branding).await {
                    warn!("Failed to login: {}", e);
                    continue;
                }

                // Add to Homarr (no cached list for single event)
                match client
                    .add_discovered_app(&app, &branding.board.name, None)
                    .await
                {
                    Ok(_) => {
                        state.discovered_apps.insert(
                            app.url.clone(),
                            state::DiscoveredApp {
                                name: app.name.clone(),
                                container_id: app.container_id.clone(),
                                added_at: chrono::Utc::now(),
                            },
                        );
                        state.update_sync_time();
                        if let Err(e) = state.save(&config.state_file) {
                            warn!("Failed to save state: {}", e);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to add app '{}' to Homarr: {}", app.name, e);
                    }
                }
            }
            ContainerEvent::Stopped(container_id) => {
                // Log container stop but don't remove from Homarr
                // (apps may restart, user can remove manually if needed)
                // Note: We now track by URL, so we search by container_id in values
                let stopped_app = state
                    .discovered_apps
                    .values()
                    .find(|a| a.container_id == container_id);
                if let Some(app) = stopped_app {
                    info!("Container stopped: {} (keeping in Homarr)", app.name);
                }
            }
        }
    }

    warn!("Event channel closed, exiting watch mode");
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

    // Scan Docker containers
    info!("Scanning Docker containers");
    let discovered = docker::discover_apps(config).await?;

    // Create client and login
    let client = homarr::HomarrClient::new(&config.homarr_url)?;
    client.ensure_logged_in(&branding).await?;

    // Pre-fetch existing apps for efficient deduplication
    let existing_apps = client.get_all_apps().await.unwrap_or_else(|e| {
        warn!("Failed to fetch existing apps: {}", e);
        vec![]
    });

    // Add new apps (use URL as key for deduplication)
    for app in &discovered {
        if !state.discovered_apps.contains_key(&app.url) && !state.is_removed(&app.url) {
            match client
                .add_discovered_app(app, &branding.board.name, Some(&existing_apps))
                .await
            {
                Ok(_) => {
                    state.discovered_apps.insert(
                        app.url.clone(),
                        state::DiscoveredApp {
                            name: app.name.clone(),
                            container_id: app.container_id.clone(),
                            added_at: chrono::Utc::now(),
                        },
                    );
                }
                Err(e) => {
                    warn!("Failed to add app '{}': {}", app.name, e);
                }
            }
        }
    }

    state.update_sync_time();
    state.save(&config.state_file)?;

    info!("Sync complete");
    Ok(())
}

async fn run_setup(config: &Config) -> Result<()> {
    // Load branding config
    let branding = branding::BrandingConfig::load(&config.branding_file)?;

    // Create Homarr client
    let client = homarr::HomarrClient::new(&config.homarr_url)?;

    // Check onboarding status
    let step = client.get_onboarding_step().await?;
    info!("Current onboarding step: {:?}", step);

    if step.current != "finish" {
        info!("Completing onboarding");
        client.complete_onboarding(&branding).await?;
    }

    // Login and create default board
    info!("Setting up default board");
    client.setup_default_board(&branding).await?;

    // Mark first boot complete
    let mut state = state::State::load(&config.state_file).unwrap_or_default();
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
        println!("Discovered apps: {}", state.discovered_apps.len());
        for (url, app) in &state.discovered_apps {
            println!(
                "  - {} ({}) [{}]",
                app.name,
                url,
                &app.container_id[..12.min(app.container_id.len())]
            );
        }
    } else {
        println!("Status: First-boot setup pending");
    }

    Ok(())
}
