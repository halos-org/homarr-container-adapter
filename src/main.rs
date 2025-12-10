//! Homarr Container Adapter
//!
//! This service provides:
//! - First-boot setup: Completes Homarr onboarding with HaLOS branding
//! - Auto-discovery: Scans Docker containers and adds them to Homarr dashboard

mod branding;
mod config;
mod docker;
mod error;
mod homarr;
mod state;

use clap::{Parser, Subcommand};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

use crate::config::Config;
use crate::error::Result;

#[derive(Parser)]
#[command(name = "homarr-container-adapter")]
#[command(about = "Adapter for Homarr dashboard: first-boot setup and auto-discovery")]
#[command(version)]
struct Cli {
    /// Config file path
    #[arg(short, long, default_value = "/etc/homarr-container-adapter/config.toml")]
    config: String,

    /// Enable debug logging
    #[arg(short, long)]
    debug: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a single sync cycle (for systemd timer)
    Sync,

    /// Run first-boot setup only
    Setup,

    /// Check if first-boot setup is needed
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
    let state = state::State::load(&config.state_file)?;

    if !state.first_boot_completed {
        info!("First boot detected, running setup");
        run_setup(config).await?;
    }

    // Scan Docker containers and update Homarr
    info!("Scanning Docker containers");
    let discovered = docker::discover_apps(config).await?;

    info!("Updating Homarr dashboard");
    homarr::sync_apps(config, &discovered).await?;

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
    } else {
        println!("Status: First-boot setup pending");
    }

    Ok(())
}
