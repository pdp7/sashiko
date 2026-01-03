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
    /// Number of last messages to ingest
    #[arg(long)]
    download: Option<usize>,

    /// Disable NNTP ingestor
    #[arg(long)]
    no_nntp: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments
    let cli = Cli::parse();

    // Initialize tracing with EnvFilter, defaulting to "info" if RUST_LOG is not set
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    fmt().with_env_filter(env_filter).init();

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

    // Initialize Database
    let db = Arc::new(Database::new(&settings.database).await?);
    db.migrate().await?;

    // Create internal task queue
    let (tx, mut rx) = mpsc::channel::<Event>(100);

    // Spawn Worker (Placeholder)
    let worker_db = db.clone();
    tokio::spawn(async move {
        info!("Worker started");

        while let Some(event) = rx.recv().await {
            match event {
                Event::ArticleFetched {
                    group,
                    article_id,
                    content,
                    raw,
                } => {
                    let raw_bytes = match raw {
                        Some(b) => b,
                        None => content.join("\n").into_bytes(),
                    };

                    match crate::patch::parse_email(&raw_bytes) {
                        Ok((metadata, patch_opt)) => {
                            let subject = if metadata.subject.len() > 80 {
                                format!("{}...", &metadata.subject[..77])
                            } else {
                                metadata.subject.clone()
                            };

                            info!(
                                "Article: group={}, id={}, author={}, subject=\"{}\"",
                                group, article_id, metadata.author, subject
                            );

                            match worker_db
                                .create_patchset(
                                    &metadata.message_id,
                                    &metadata.subject,
                                    &metadata.author,
                                    metadata.date,
                                    metadata.total,
                                )
                                .await
                            {
                                Ok(patchset_id) => {
                                    if let Some(patch) = patch_opt {
                                        if let Err(e) = worker_db
                                            .create_patch(
                                                patchset_id,
                                                &patch.message_id,
                                                patch.part_index,
                                                &patch.body,
                                                &patch.diff,
                                            )
                                            .await
                                        {
                                            error!("Failed to save patch: {}", e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to save patchset: {}", e);
                                }
                            }
                        }

                        Err(e) => {
                            info!(
                                "Article (parse failed): group={}, id={}, error={}",
                                group, article_id, e
                            );
                        }
                    }
                }
            }
        }
    });

    // Start Ingestor
    let ingestor = Ingestor::new(settings.clone(), db.clone(), tx, cli.download, cli.no_nntp);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parsing() {
        let args = vec!["sashiko", "--download", "100", "--no-nntp"];
        let cli = Cli::parse_from(args);
        assert_eq!(cli.download, Some(100));
        assert!(cli.no_nntp);

        let args = vec!["sashiko"];
        let cli = Cli::parse_from(args);
        assert_eq!(cli.download, None);
        assert!(!cli.no_nntp);
    }
}
