use clap::{Parser, Subcommand};
use sashiko::db::Database;
use sashiko::events::{Event, ParsedArticle};
use sashiko::ingestor::Ingestor;
use sashiko::settings::Settings;
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

    /// Exit after processing all messages (useful for benchmarking)
    #[arg(long)]
    exit_after_ingest: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Inspect,
}

const PARSER_VERSION: i32 = 2;

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

    if let Some(Commands::Inspect) = cli.command {
        return sashiko::inspector::run_inspection(db).await.map_err(|e| e.into());
    }

    // Create internal task queues
    // raw_tx -> Parser -> parsed_tx -> DB Worker
    let (raw_tx, mut raw_rx) = mpsc::channel::<Event>(1000);
    let (parsed_tx, mut parsed_rx) = mpsc::channel::<ParsedArticle>(1000);

    // Parser Dispatcher
    tokio::spawn(async move {
        info!("Parser Dispatcher started");
        while let Some(event) = raw_rx.recv().await {
            let tx = parsed_tx.clone();
            tokio::spawn(async move {
                // Extract raw bytes
                let (group, article_id, raw_bytes) = match event {
                    Event::ArticleFetched {
                        group,
                        article_id,
                        content,
                        raw,
                    } => {
                        let bytes = match raw {
                            Some(b) => b,
                            None => content.join("\n").into_bytes(),
                        };
                        (group, article_id, bytes)
                    }
                };

                // Offload CPU parsing to blocking thread pool
                let parse_result =
                    tokio::task::spawn_blocking(move || sashiko::patch::parse_email(&raw_bytes))
                        .await;

                match parse_result {
                    Ok(Ok((metadata, patch_opt))) => {
                        if let Err(e) = tx
                            .send(ParsedArticle {
                                group,
                                article_id,
                                metadata,
                                patch: patch_opt,
                            })
                            .await
                        {
                            error!("Failed to send parsed article: {}", e);
                        }
                    }
                    Ok(Err(e)) => {
                        info!("Parse error for {}: {}", article_id, e);
                    }
                    Err(e) => {
                        error!("Join error in parser: {}", e);
                    }
                }
            });
        }
    });

    // DB Worker (Transactional Batching)
    let worker_db = db.clone();
    let db_worker_handle = tokio::spawn(async move {
        info!("DB Worker started");

        let mut buffer = Vec::with_capacity(100);
        let mut total_processed = 0;
        let mut total_ingested = 0;
        let mut total_errors = 0;

        loop {
            let count = parsed_rx.recv_many(&mut buffer, 100).await;
            if count == 0 {
                break;
            }

            // info!("Processing batch of {} parsed articles", count); // Too verbose
            if let Err(e) = worker_db.begin_transaction().await {
                error!("Failed to begin transaction: {}", e);
                total_errors += count; // Assume all failed if txn fails? Or just log error.
                continue;
            }

            for article in buffer.drain(..) {
                match process_parsed_article(&worker_db, article).await {
                    ProcessStatus::Ingested => total_ingested += 1,
                    ProcessStatus::Error => total_errors += 1,
                }
                total_processed += 1;

                if total_processed % 500 == 0 {
                    info!(
                        "Ingestion Progress: {} processed ({} ingested, {} errors)",
                        total_processed, total_ingested, total_errors
                    );
                }
            }

            if let Err(e) = worker_db.commit_transaction().await {
                error!("Failed to commit transaction: {}", e);
                // If commit fails, technically we lost the batch work in DB, but counters are already updated.
                // For simple stats, this is acceptable, but ideally we'd track "pending" stats.
                // Keeping it simple.
            }
        }

        // Final stats
        info!(
            "Ingestion Complete: {} processed ({} ingested, {} errors)",
            total_processed, total_ingested, total_errors
        );
    });

    // Start Ingestor (feeds raw_tx)
    let ingestor = Ingestor::new(
        settings.clone(),
        db.clone(),
        raw_tx,
        cli.download,
        cli.no_nntp,
    );
    tokio::spawn(async move {
        if let Err(e) = ingestor.run().await {
            error!("Ingestor fatal error: {}", e);
        }
    });

    // Start Web API
    let api_settings = settings.server.clone();
    let api_db = db.clone();
    tokio::spawn(async move {
        if let Err(e) = sashiko::api::run_server(api_settings, api_db).await {
            error!("Web API fatal error: {}", e);
        }
    });

    if cli.exit_after_ingest {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down...");
            }
            _ = db_worker_handle => {
                info!("Ingestion complete. Exiting.");
            }
        }
    } else {
        // Keep the main thread running
        tokio::signal::ctrl_c().await?;
        info!("Shutting down...");
    }

    Ok(())
}

enum ProcessStatus {
    Ingested,
    Error,
}

async fn process_parsed_article(worker_db: &Database, article: ParsedArticle) -> ProcessStatus {
    let ParsedArticle {
        group,
        article_id,
        metadata,
        patch,
    } = article;
    let patch_opt = patch;

    // 1. Thread Resolution
    let thread_id = if let Some(ref reply_to) = metadata.in_reply_to {
        match worker_db
            .ensure_thread_for_message(reply_to, metadata.date)
            .await
        {
            Ok(tid) => tid,
            Err(e) => {
                error!("Failed to ensure thread for parent {}: {}", reply_to, e);
                return ProcessStatus::Error;
            }
        }
    } else {
        match worker_db
            .ensure_thread_for_message(&metadata.message_id, metadata.date)
            .await
        {
            Ok(tid) => tid,
            Err(e) => {
                error!(
                    "Failed to ensure thread for self {}: {}",
                    metadata.message_id, e
                );
                return ProcessStatus::Error;
            }
        }
    };

    let is_git_hash = article_id.len() == 40 && article_id.chars().all(|c| c.is_ascii_hexdigit());
    let (body_to_store, git_hash_opt) = if is_git_hash {
        ("", Some(article_id.as_str()))
    } else {
        (metadata.body.as_str(), None)
    };

    // 2. Create Message
    if let Err(e) = worker_db
        .create_message(
            &metadata.message_id,
            thread_id,
            metadata.in_reply_to.as_deref(),
            &metadata.author,
            &metadata.subject,
            metadata.date,
            body_to_store,
            &metadata.to,
            &metadata.cc,
            git_hash_opt,
            Some(&group),
        )
        .await
    {
        error!("Failed to create message: {}", e);
        return ProcessStatus::Error;
    }

    // Subsystem Identification and Linking
    let subsystems = identify_subsystems(&metadata.to, &metadata.cc);
    let mut subsystem_ids = Vec::new();
    for (name, email) in subsystems {
        match worker_db.ensure_subsystem(&name, &email).await {
            Ok(sid) => subsystem_ids.push(sid),
            Err(e) => error!("Failed to ensure subsystem {}: {}", name, e),
        }
    }

    if let Ok(Some(msg_id_db)) = worker_db
        .get_message_id_by_msg_id(&metadata.message_id)
        .await
    {
        for &sid in &subsystem_ids {
            if let Err(e) = worker_db.add_subsystem_to_message(msg_id_db, sid).await {
                error!("Failed to link message to subsystem: {}", e);
            }
            if let Err(e) = worker_db.add_subsystem_to_thread(thread_id, sid).await {
                error!("Failed to link thread to subsystem: {}", e);
            }
        }
    }

    // Removed baseline detection from ingestion as it's now part of review process

    // Removed per-article info log
    /*
    let subject = if metadata.subject.len() > 80 {
        format!("{}...", &metadata.subject[..77])
    } else {
        metadata.subject.clone()
    };
    info!(
        "Article: group={}, id={}, author={}, subject=\"{}\"",
        group, article_id, metadata.author, subject
    );
    */

    let cover_letter_id = if metadata.index == 0 {
        Some(metadata.message_id.as_str())
    } else {
        None
    };

    if metadata.is_patch_or_cover {
        match worker_db
            .create_patchset(
                thread_id,
                cover_letter_id,
                &metadata.subject,
                &metadata.author,
                metadata.date,
                metadata.total,
                PARSER_VERSION,
                &metadata.to,
                &metadata.cc,
                metadata.version,
                metadata.index,
            )
            .await
        {
            Ok(Some(patchset_id)) => {
                for &sid in &subsystem_ids {
                    if let Err(e) = worker_db.add_subsystem_to_patchset(patchset_id, sid).await {
                        error!("Failed to link patchset to subsystem: {}", e);
                    }
                }

                if let Some(patch) = patch_opt {
                    match worker_db
                        .create_patch(
                            patchset_id,
                            &patch.message_id,
                            patch.part_index,
                            &patch.diff,
                        )
                        .await
                    {
                        Ok(patch_id) => {
                            for &sid in &subsystem_ids {
                                if let Err(e) =
                                    worker_db.add_subsystem_to_patch(patch_id, sid).await
                                {
                                    error!("Failed to link patch to subsystem: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to save patch: {}", e);
                            return ProcessStatus::Error;
                        }
                    }
                }
                ProcessStatus::Ingested
            }
            Ok(None) => {
                // Skipped patchset creation (reply mismatch or duplicate)
                // BUT message was ingested successfully.
                ProcessStatus::Ingested
            }
            Err(e) => {
                error!("Failed to save patchset: {}", e);
                ProcessStatus::Error
            }
        }
    } else {
        // Skipped patchset creation/update for non-patch message
        // BUT message was ingested successfully.
        ProcessStatus::Ingested
    }
}

// Helper function to map To/Cc to Subsystems
fn identify_subsystems(to: &str, cc: &str) -> Vec<(String, String)> {
    let mut subsystems = Vec::new();
    let mut all_recipients = String::new();
    all_recipients.push_str(to);
    all_recipients.push_str(", ");
    all_recipients.push_str(cc);

    for email in all_recipients.split(',') {
        let email = email.trim();
        if email.is_empty() {
            continue;
        }

        let lower_email = email.to_lowercase();

        // 1. Static Map (Mimic MAINTAINERS)
        if lower_email.contains("linux-kernel@vger.kernel.org") {
            subsystems.push((
                "LKML".to_string(),
                "linux-kernel@vger.kernel.org".to_string(),
            ));
        } else if lower_email.contains("netdev@vger.kernel.org") {
            subsystems.push(("netdev".to_string(), "netdev@vger.kernel.org".to_string()));
        } else if lower_email.contains("bpf@vger.kernel.org") {
            subsystems.push(("bpf".to_string(), "bpf@vger.kernel.org".to_string()));
        } else if lower_email.contains("linux-usb@vger.kernel.org") {
            subsystems.push(("usb".to_string(), "linux-usb@vger.kernel.org".to_string()));
        } else if lower_email.contains("linux-fsdevel@vger.kernel.org") {
            subsystems.push((
                "fsdevel".to_string(),
                "linux-fsdevel@vger.kernel.org".to_string(),
            ));
        } else if lower_email.contains("linux-mm@kvack.org") {
            subsystems.push(("mm".to_string(), "linux-mm@kvack.org".to_string()));
        } else if lower_email.ends_with("@vger.kernel.org")
            || lower_email.ends_with("@lists.linux.dev")
            || lower_email.ends_with("@lists.infradead.org")
        {
            // Fallback: derive name from email user part
            if let Some(name) = lower_email.split('@').next() {
                subsystems.push((name.to_string(), lower_email));
            }
        }
    }

    subsystems.sort();
    subsystems.dedup();
    subsystems
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

    #[test]
    fn test_identify_subsystems() {
        // Test known subsystem
        let to = "linux-kernel@vger.kernel.org";
        let cc = "netdev@vger.kernel.org";
        let subsystems = identify_subsystems(to, cc);
        assert!(subsystems.contains(&(
            "LKML".to_string(),
            "linux-kernel@vger.kernel.org".to_string()
        )));
        assert!(subsystems.contains(&("netdev".to_string(), "netdev@vger.kernel.org".to_string())));

        // Test fallback
        let to = "unknown-list@vger.kernel.org";
        let cc = "";
        let subsystems = identify_subsystems(to, cc);
        assert!(subsystems.contains(&(
            "unknown-list".to_string(),
            "unknown-list@vger.kernel.org".to_string()
        )));

        // Test mixed
        let to = "linux-usb@vger.kernel.org, random-user@example.com";
        let cc = "bpf@vger.kernel.org";
        let subsystems = identify_subsystems(to, cc);
        assert!(subsystems.contains(&("usb".to_string(), "linux-usb@vger.kernel.org".to_string())));
        assert!(subsystems.contains(&("bpf".to_string(), "bpf@vger.kernel.org".to_string())));
        // random-user should be ignored as it doesn't match list patterns
        assert_eq!(subsystems.len(), 2);
    }
}
