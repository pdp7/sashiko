mod ai;
mod api;
mod baseline;
mod db;
mod events;
mod git_ops;
mod ingestor;
mod nntp;
mod patch;
mod settings;

use clap::Parser;
use db::Database;
use events::Event;
use ingestor::Ingestor;
use settings::Settings;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Name of the person to greet
    #[arg(short, long)]
    name: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments
    let _cli = Cli::parse();

    // Initialize tracing with EnvFilter, defaulting to "info" if RUST_LOG is not set
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    fmt()
        .with_env_filter(env_filter)
        .init();

    info!("Starting Sashiko...");

    // Load settings
    let settings = match Settings::new() {
        Ok(s) => {
            info!("Settings loaded successfully");
            s
        }
        Err(e) => {
            error!("Failed to load settings: {}", e);
            return Err(e.into());
        }
    };

    info!("Settings: {:?}", settings);

    // Initialize Database
    let db = Arc::new(Database::new(&settings.database).await?);
    db.migrate().await?;

    // Create internal task queue
    let (tx, mut rx) = mpsc::channel::<Event>(100);

    // Spawn Worker (Placeholder)
    tokio::spawn(async move {
        info!("Worker started");
        while let Some(event) = rx.recv().await {
            info!("Worker received event: {:?}", event);
        }
    });

    // Start Ingestor
    let ingestor = Ingestor::new(settings.clone(), db.clone(), tx);
    tokio::spawn(async move {
        if let Err(e) = ingestor.run().await {
            error!("Ingestor fatal error: {}", e);
        }
    });

    // Start Web API
    let api_settings = settings.server.clone();
    let api_db = db.clone();
    tokio::spawn(async move {
        if let Err(e) = api::run_server(api_settings, api_db).await {
            error!("Web API fatal error: {}", e);
        }
    });

    // Keep the main thread running
    tokio::signal::ctrl_c().await?;
    info!("Shutting down...");

    Ok(())
}