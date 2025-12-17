//! Homarr Container Adapter
//!
//! This service provides:
//! - First-boot setup: Completes Homarr onboarding with HaLOS branding
//! - App registry: Syncs apps from /etc/halos/webapps.d/ to Homarr dashboard

mod branding;
mod config;
mod error;
mod homarr;
mod registry;
mod state;

use clap::{Parser, Subcommand};
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

use crate::config::Config;
use crate::error::Result;

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

    // Create client and login
    let client = homarr::HomarrClient::new(&config.homarr_url)?;
    client.ensure_logged_in(&branding).await?;

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
        if state.is_removed(&entry.app.url) {
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
