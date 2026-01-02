use crate::db::Database;
use crate::events::Event;
use crate::nntp::NntpClient;
use crate::settings::{IngestionMode, Settings};
use anyhow::{anyhow, Result};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

pub struct Ingestor {
    settings: Settings,
    db: Arc<Database>,
    sender: Sender<Event>,
}

impl Ingestor {
    pub fn new(settings: Settings, db: Arc<Database>, sender: Sender<Event>) -> Self {
        Self {
            settings,
            db,
            sender,
        }
    }

    pub async fn run(&self) -> Result<()> {
        match self.settings.ingestion.mode {
            IngestionMode::Nntp => self.run_nntp().await,
            IngestionMode::LocalArchive => self.run_local_archive().await,
        }
    }

    async fn run_nntp(&self) -> Result<()> {
        info!(
            "Starting NNTP Ingestor for groups: {:?}",
            self.settings.nntp.groups
        );

        loop {
            if let Err(e) = self.process_nntp_cycle().await {
                error!("NNTP Ingestion cycle failed: {}", e);
            }
            sleep(Duration::from_secs(60)).await;
        }
    }

    async fn process_nntp_cycle(&self) -> Result<()> {
        let mut client =
            NntpClient::connect(&self.settings.nntp.server, self.settings.nntp.port).await?;

        for group_name in &self.settings.nntp.groups {
            self.db.ensure_mailing_list(group_name, group_name).await?;

            let info = client.group(group_name).await?;
            let last_known = self.db.get_last_article_num(group_name).await?;

            info!(
                "Group {}: estimated count={}, low={}, high={}, last_known={}",
                group_name, info.number, info.low, info.high, last_known
            );

            let mut current = last_known;
            if current == 0 && info.high > 0 {
                current = info.high.saturating_sub(5);
                self.db.update_last_article_num(group_name, current).await?;
                info!("Initialized high-water mark to {}", current);
            }

            if current < info.high {
                let next_id = current + 1;
                info!("Fetching article {}", next_id);
                match client.article(&next_id.to_string()).await {
                    Ok(lines) => {
                        self.sender
                            .send(Event::ArticleFetched {
                                group: group_name.clone(),
                                article_id: next_id.to_string(),
                                content: lines,
                                raw: None,
                            })
                            .await?;
                        self.db.update_last_article_num(group_name, next_id).await?;
                        info!("Updated high-water mark to {}", next_id);
                    }
                    Err(e) => {
                        error!("Failed to fetch article {}: {}", next_id, e);
                    }
                }
            }
        }

        client.quit().await?;
        Ok(())
    }

    async fn run_local_archive(&self) -> Result<()> {
        let archive_settings =
            self.settings.ingestion.archive.as_ref().ok_or_else(|| {
                anyhow!("LocalArchive mode selected but no archive path provided")
            })?;

        info!(
            "Starting Local Archive Ingestor from {:?}",
            archive_settings.path
        );

        // 1. Get list of all object SHAs
        info!("Listing git objects...");
        let output = Command::new("git")
            .arg("-c")
            .arg("safe.bareRepository=all")
            .current_dir(&archive_settings.path)
            .arg("rev-list")
            .arg("--all")
            .arg("--objects")
            .output()
            .await?;

        if !output.status.success() {
            return Err(anyhow!(
                "Failed to list git objects: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let shas: Vec<String> = stdout
            .lines()
            .filter_map(|line| line.split_whitespace().next().map(|s| s.to_string()))
            .collect();

        info!("Found {} objects. Starting batch processing...", shas.len());

        // 2. Start git cat-file --batch
        let mut child = Command::new("git")
            .arg("-c")
            .arg("safe.bareRepository=all")
            .current_dir(&archive_settings.path)
            .arg("cat-file")
            .arg("--batch")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Failed to open stdin for git cat-file"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to open stdout for git cat-file"))?;
        let mut reader = BufReader::new(stdout);

        let mut count = 0;
        let mut processed_blobs = 0;

        // We process in a loop. To avoid deadlock, we could spawn a writer task, 
        // or just write one SHA, read result, write next... (synchronous sequential)
        // Sequential is safest and simplest here.

        for hash in shas {
            // Write SHA to stdin
            stdin.write_all(format!("{}\n", hash).as_bytes()).await?;
            stdin.flush().await?;

            // Read header: <sha> <type> <size>
            let mut header = String::new();
            if reader.read_line(&mut header).await? == 0 {
                break; // EOF
            }

            let parts: Vec<&str> = header.split_whitespace().collect();
            if parts.len() < 3 {
                warn!("Invalid batch header for {}: {}", hash, header);
                continue;
            }

            let obj_type = parts[1];
            let size: usize = parts[2].parse().unwrap_or(0);

            // Read content + newline
            let mut content = vec![0u8; size];
            reader.read_exact(&mut content).await?;
            
            // Consume the trailing newline that --batch outputs
            let mut newline = [0u8; 1];
            reader.read_exact(&mut newline).await?;

            if obj_type == "blob" {
                // Convert raw to lines for the 'content' field (legacy)
                let content_str = String::from_utf8_lossy(&content);
                let lines: Vec<String> = content_str.lines().map(|s| s.to_string()).collect();

                self.sender
                    .send(Event::ArticleFetched {
                        group: "local-archive".to_string(),
                        article_id: hash.clone(),
                        content: lines,
                        raw: Some(content),
                    })
                    .await?;

                processed_blobs += 1;
                if processed_blobs % 1000 == 0 {
                    info!("Processed {} blobs", processed_blobs);
                }
            }

            count += 1;
            // if count % 5000 == 0 {
            //     info!("Scanned {}/{} objects...", count, shas.len());
            // }
        }

        info!(
            "Local archive ingestion completed. Scanned {} objects, processed {} blobs.",
            count, processed_blobs
        );
        Ok(())
    }
}