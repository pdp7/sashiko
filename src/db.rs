use crate::settings::DatabaseSettings;
use anyhow::Result;
use libsql::Builder;
use serde::Serialize;
use tracing::info;

pub struct Database {
    pub conn: libsql::Connection,
}

#[derive(Debug, Serialize)]
pub struct PatchsetRow {
    pub id: i64,
    pub message_id: String,
    pub subject: Option<String>,
    pub author: Option<String>,
    pub date: Option<i64>,
    pub status: Option<String>,
}

impl Database {
    pub async fn new(settings: &DatabaseSettings) -> Result<Self> {
        info!("Connecting to database at {}", settings.url);

        let db = if settings.url.starts_with("libsql://") || settings.url.starts_with("https://") {
            Builder::new_remote(settings.url.clone(), settings.token.clone())
                .build()
                .await?
        } else {
            Builder::new_local(&settings.url).build().await?
        };

        let conn = db.connect()?;

        Ok(Self { conn })
    }

    pub async fn migrate(&self) -> Result<()> {
        let schema = include_str!("schema.sql");
        self.conn.execute_batch(schema).await?;

        // Idempotent migrations
        let _ = self
            .conn
            .execute(
                "ALTER TABLE patchsets ADD COLUMN parser_version INTEGER DEFAULT 0",
                libsql::params![],
            )
            .await;
        let _ = self
            .conn
            .execute(
                "ALTER TABLE patchsets ADD COLUMN to_recipients TEXT",
                libsql::params![],
            )
            .await;
        let _ = self
            .conn
            .execute(
                "ALTER TABLE patchsets ADD COLUMN cc_recipients TEXT",
                libsql::params![],
            )
            .await;
        let _ = self
            .conn
            .execute(
                "ALTER TABLE patchsets ADD COLUMN baseline_id INTEGER",
                libsql::params![],
            )
            .await;
        let _ = self
            .conn
            .execute(
                "ALTER TABLE reviews ADD COLUMN interaction_id TEXT",
                libsql::params![],
            )
            .await;

        info!("Database schema applied");
        Ok(())
    }

    pub async fn ensure_mailing_list(&self, name: &str, group: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO mailing_lists (name, nntp_group, last_article_num) VALUES (?, ?, 0)",
                libsql::params![name, group],
            )
            .await?;
        Ok(())
    }

    pub async fn get_last_article_num(&self, group: &str) -> Result<u64> {
        let mut rows = self
            .conn
            .query(
                "SELECT last_article_num FROM mailing_lists WHERE nntp_group = ?",
                libsql::params![group],
            )
            .await?;

        if let Ok(Some(row)) = rows.next().await {
            let num: i64 = row.get(0)?;
            Ok(num as u64)
        } else {
            Ok(0)
        }
    }

    pub async fn update_last_article_num(&self, group: &str, num: u64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE mailing_lists SET last_article_num = ? WHERE nntp_group = ?",
                libsql::params![num as i64, group],
            )
            .await?;
        Ok(())
    }

    pub async fn get_patchset_version(&self, message_id: &str) -> Result<Option<i32>> {
        let mut rows = self
            .conn
            .query(
                "SELECT parser_version FROM patchsets WHERE message_id = ?",
                libsql::params![message_id],
            )
            .await?;

        if let Ok(Some(row)) = rows.next().await {
            let ver: Option<i32> = row.get(0).ok();
            Ok(ver)
        } else {
            Ok(None)
        }
    }

    pub async fn create_baseline(
        &self,
        repo_url: Option<&str>,
        branch: Option<&str>,
        commit: Option<&str>,
    ) -> Result<i64> {
        // Simple deduplication: if exactly same, return id.
        // But for simplicity, we just insert.
        self.conn
            .execute(
                "INSERT INTO baselines (repo_url, branch, last_known_commit) VALUES (?, ?, ?)",
                libsql::params![repo_url, branch, commit],
            )
            .await?;

        let mut rows = self
            .conn
            .query("SELECT last_insert_rowid()", libsql::params![])
            .await?;
        if let Ok(Some(row)) = rows.next().await {
            Ok(row.get(0)?)
        } else {
            Err(anyhow::anyhow!("Failed to get baseline ID"))
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_patchset(
        &self,
        message_id: &str,
        subject: &str,
        author: &str,
        date: i64,
        total_parts: u32,
        parser_version: i32,
        to: &str,
        cc: &str,
        baseline_id: Option<i64>,
    ) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO patchsets (message_id, subject, author, date, total_parts, received_parts, status, parser_version, to_recipients, cc_recipients, baseline_id) 
                 VALUES (?, ?, ?, ?, ?, 1, 'Pending', ?, ?, ?, ?) 
                 ON CONFLICT(message_id) DO UPDATE SET 
                    author = excluded.author,
                    subject = excluded.subject,
                    date = excluded.date,
                    parser_version = excluded.parser_version,
                    to_recipients = excluded.to_recipients,
                    cc_recipients = excluded.cc_recipients,
                    baseline_id = excluded.baseline_id",
                libsql::params![message_id, subject, author, date, total_parts, parser_version, to, cc, baseline_id],
            )
            .await?;

        let mut rows = self
            .conn
            .query(
                "SELECT id FROM patchsets WHERE message_id = ?",
                libsql::params![message_id],
            )
            .await?;
        if let Ok(Some(row)) = rows.next().await {
            let id: i64 = row.get(0)?;
            Ok(id)
        } else {
            Err(anyhow::anyhow!(
                "Failed to retrieve patchset ID after insert"
            ))
        }
    }

    pub async fn create_patch(
        &self,
        patchset_id: i64,
        message_id: &str,
        part_index: u32,
        body: &str,
        diff: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO patches (patchset_id, message_id, part_index, body, diff) VALUES (?, ?, ?, ?, ?)",
            libsql::params![patchset_id, message_id, part_index, body, diff]
        ).await?;
        Ok(())
    }

    pub async fn get_patchsets(&self, limit: usize, offset: usize) -> Result<Vec<PatchsetRow>> {
        let mut rows = self.conn.query(
            "SELECT id, message_id, subject, author, date, status FROM patchsets ORDER BY date DESC LIMIT ? OFFSET ?",
            libsql::params![limit as i64, offset as i64],
        ).await?;

        let mut patchsets = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            patchsets.push(PatchsetRow {
                id: row.get(0)?,
                message_id: row.get(1)?,
                subject: row.get(2).ok(),
                author: row.get(3).ok(),
                date: row.get(4).ok(),
                status: row.get(5).ok(),
            });
        }
        Ok(patchsets)
    }

    pub async fn count_patchsets(&self) -> Result<usize> {
        let mut rows = self.conn.query("SELECT COUNT(*) FROM patchsets", libsql::params![]).await?;
        if let Ok(Some(row)) = rows.next().await {
            let count: i64 = row.get(0)?;
            Ok(count as usize)
        } else {
            Ok(0)
        }
    }

    pub async fn get_patchset_details(
        &self,
        message_id: &str,
    ) -> Result<Option<serde_json::Value>> {
        // Fetch basic details + baseline
        let mut rows = self.conn.query(
            "SELECT p.id, p.subject, p.author, p.date, p.status, p.to_recipients, p.cc_recipients, 
                    b.repo_url, b.branch, b.last_known_commit
             FROM patchsets p 
             LEFT JOIN baselines b ON p.baseline_id = b.id
             WHERE p.message_id = ?",
            libsql::params![message_id],
        ).await?;

        if let Ok(Some(row)) = rows.next().await {
            let id: i64 = row.get(0)?;
            let subject: Option<String> = row.get(1).ok();
            let author: Option<String> = row.get(2).ok();
            let date: Option<i64> = row.get(3).ok();
            let status: Option<String> = row.get(4).ok();
            let to: Option<String> = row.get(5).ok();
            let cc: Option<String> = row.get(6).ok();
            let repo_url: Option<String> = row.get(7).ok();
            let branch: Option<String> = row.get(8).ok();
            let commit: Option<String> = row.get(9).ok();

            // Fetch reviews
            let mut reviews = Vec::new();
            let mut rev_rows = self
                .conn
                .query(
                    "SELECT r.model_name, r.summary, r.created_at, ai.input_context, ai.output_raw
                 FROM reviews r
                 LEFT JOIN ai_interactions ai ON r.interaction_id = ai.id
                 WHERE r.patchset_id = ?",
                    libsql::params![id],
                )
                .await?;

            while let Ok(Some(r)) = rev_rows.next().await {
                reviews.push(serde_json::json!({
                    "model": r.get::<Option<String>>(0).ok(),
                    "summary": r.get::<Option<String>>(1).ok(),
                    "created_at": r.get::<Option<i64>>(2).ok(),
                    "input": r.get::<Option<String>>(3).ok(),
                    "output": r.get::<Option<String>>(4).ok(),
                }));
            }

            Ok(Some(serde_json::json!({
                "id": message_id,
                "subject": subject,
                "author": author,
                "date": date,
                "status": status,
                "to": to,
                "cc": cc,
                "baseline": {
                    "repo_url": repo_url,
                    "branch": branch,
                    "commit": commit,
                },
                "reviews": reviews
            })))
        } else {
            Ok(None)
        }
    }
}
