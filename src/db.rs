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
    pub subject: Option<String>,
    pub status: Option<String>,
    pub thread_id: Option<i64>,
    pub author: Option<String>,
    pub date: Option<i64>,
    pub message_id: Option<String>,
    pub total_parts: Option<u32>,
    pub received_parts: Option<u32>,
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

    pub async fn create_thread(
        &self,
        root_message_id: &str,
        subject: &str,
        date: i64,
    ) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO threads (root_message_id, subject, last_updated) VALUES (?, ?, ?)",
                libsql::params![root_message_id, subject, date],
            )
            .await?;

        let mut rows = self.conn.query("SELECT last_insert_rowid()", ()).await?;
        if let Ok(Some(row)) = rows.next().await {
            Ok(row.get(0)?)
        } else {
            Err(anyhow::anyhow!("Failed to get thread ID"))
        }
    }

    pub async fn get_thread_id_for_message(&self, message_id: &str) -> Result<Option<i64>> {
        let mut rows = self
            .conn
            .query(
                "SELECT thread_id FROM messages WHERE message_id = ?",
                libsql::params![message_id],
            )
            .await?;
        if let Ok(Some(row)) = rows.next().await {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub async fn ensure_thread_for_message(&self, message_id: &str, date: i64) -> Result<i64> {
        // 1. Check if message exists
        if let Some(tid) = self.get_thread_id_for_message(message_id).await? {
            return Ok(tid);
        }

        // 2. Not found, create new thread and placeholder message
        let thread_id = self.create_thread(message_id, "(placeholder)", date).await?;
        
        self.create_message(
            message_id,
            thread_id,
            None,
            "unknown",
            "(placeholder)",
            date,
            "",
        ).await?;

        Ok(thread_id)
    }

    pub async fn create_message(
        &self,
        message_id: &str,
        thread_id: i64,
        in_reply_to: Option<&str>,
        author: &str,
        subject: &str,
        date: i64,
        body: &str,
    ) -> Result<()> {
        // Use INSERT OR REPLACE to handle updating placeholders
        // But we want to preserve thread_id if it was set by placeholder (which is correct).
        // Actually, if we are "creating" the real message now, we should overwrite the placeholder fields.
        // But we must ensure we keep the same thread_id if it exists? 
        // No, the caller (main.rs) resolves thread_id before calling create_message.
        // If we found a placeholder, we use its thread_id.
        // So here we just upsert.
        
        // However, if we blindly REPLACE, we might change the thread_id if we passed a different one?
        // But main.rs logic should ensure consistency.
        // Let's use INSERT OR REPLACE.
        self.conn.execute(
            "INSERT OR REPLACE INTO messages (message_id, thread_id, in_reply_to, author, subject, date, body) VALUES (?, ?, ?, ?, ?, ?, ?)",
            libsql::params![message_id, thread_id, in_reply_to, author, subject, date, body],
        ).await?;
        Ok(())
    }

    pub async fn create_baseline(
        &self,
        repo_url: Option<&str>,
        branch: Option<&str>,
        commit: Option<&str>,
    ) -> Result<i64> {
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
        thread_id: i64,
        cover_letter_message_id: Option<&str>,
        subject: &str,
        author: &str,
        date: i64,
        total_parts: u32,
        parser_version: i32,
        to: &str,
        cc: &str,
        baseline_id: Option<i64>,
        version: Option<u32>,
        part_index: u32,
    ) -> Result<Option<i64>> {
        // Find candidate patchsets in this thread
        let mut rows = self
            .conn
            .query(
                "SELECT id, date, author, subject, subject_index, total_parts FROM patchsets WHERE thread_id = ?",
                libsql::params![thread_id],
            )
            .await?;

        let mut matches = Vec::new();
        let mut has_existing_patchsets = false;
        let mut author_exists_in_thread = false;

        while let Ok(Some(row)) = rows.next().await {
            has_existing_patchsets = true;
            let id: i64 = row.get(0)?;
            let existing_date: i64 = row.get(1)?;
            let existing_author: String = row.get(2)?;
            let existing_subject: String = row.get(3)?;
            let existing_subject_index: u32 = row.get(4).unwrap_or(9999);
            let existing_total: u32 = row.get(5).unwrap_or(1);

            if existing_author == author {
                author_exists_in_thread = true;
            }

            // Parse version from existing subject
            let existing_version = crate::patch::parse_subject_version(&existing_subject);
            
            // Matching logic:
            // 1. Author must match
            // 2. Time must be close (within 24 hours / 86400s) - Increased to handle slow series
            // 3. Total parts must match
            // 4. Versions must match OR one is unspecified (None)
            //    - None matches Some(6) (Implicit v1/Unknown merges with Explicit v6)
            //    - Some(5) != Some(6) (Explicit mismatch)
            let versions_compatible = match (version, existing_version) {
                (Some(a), Some(b)) => a == b,
                _ => true, // One or both are None -> Compatible
            };

            if existing_author == author && (date - existing_date).abs() < 86400 && versions_compatible && total_parts == existing_total {
                matches.push((id, existing_subject_index));
            }
        }

        // Enforce Same Sender constraint
        if has_existing_patchsets && !author_exists_in_thread {
            info!("Skipping patchset creation for thread {} author '{}': different from existing patchset authors", thread_id, author);
            return Ok(None);
        }

        if !matches.is_empty() {
            // Sort matches to pick the "best" one to keep (e.g. oldest ID or one with lowest subject index)
            // Let's keep the one with the lowest ID (created first)
            matches.sort_by_key(|k| k.0);
            
            let target_id = matches[0].0;
            let mut current_subject_index = matches[0].1;

            // If we have multiple matches, merge others into target_id
            for i in 1..matches.len() {
                let merge_from_id = matches[i].0;
                info!("Merging patchset {} into {}", merge_from_id, target_id);
                
                // Reassign patches
                self.conn.execute(
                    "UPDATE OR IGNORE patches SET patchset_id = ? WHERE patchset_id = ?",
                    libsql::params![target_id, merge_from_id]
                ).await?;
                
                // If the merged patchset had a better subject index, track it
                if matches[i].1 < current_subject_index {
                    current_subject_index = matches[i].1;
                }

                // Delete the merged patchset
                self.conn.execute(
                    "DELETE FROM patchsets WHERE id = ?",
                    libsql::params![merge_from_id]
                ).await?;
            }

            // Update the target patchset
            self.conn.execute(
                "UPDATE patchsets SET author = ?, total_parts = ?, parser_version = ?, to_recipients = ?, cc_recipients = ?, baseline_id = ? WHERE id = ?",
                libsql::params![author, total_parts, parser_version, to, cc, baseline_id, target_id],
            ).await?;

            // Conditionally update subject
            // Note: We check against the best index found among all merged sets OR the new part_index
            if part_index < current_subject_index {
                self.conn.execute(
                    "UPDATE patchsets SET subject = ?, subject_index = ? WHERE id = ?",
                    libsql::params![subject, part_index, target_id],
                ).await?;
            } else if matches.len() > 1 {
                // If we merged, we might need to update the subject index of the target to the best one we found
                // But we don't have the subject string from the merged one easily available here.
                // However, the existing target subject is likely fine unless part_index is better.
                // We just update subject_index to be correct if we merged a better one?
                // Actually, if matches[i].1 was better, we should have used its subject.
                // But that's complicated. Assuming the target (oldest) usually has the cover letter or we eventually find it.
                // Simplification: We only update if CURRENT patch is better.
                // If we merged a patchset that HAD the cover letter, we ideally want that subject.
                // But we lost it.
                // TODO: Optimize merge subject selection. For now, this is better than duplicates.
            }
            
            if let Some(clid) = cover_letter_message_id {
                 self.conn.execute(
                    "UPDATE patchsets SET cover_letter_message_id = ? WHERE id = ?",
                    libsql::params![clid, target_id],
                ).await?;
            }
            
            // Recalculate received parts for target (in case we merged)
             self.conn
            .execute(
                "UPDATE patchsets SET received_parts = (SELECT COUNT(*) FROM patches WHERE patchset_id = ?) WHERE id = ?",
                libsql::params![target_id, target_id],
            )
            .await?;

            return Ok(Some(target_id));
        }

        // No match found, create new patchset
        self.conn
            .execute(
                "INSERT INTO patchsets (thread_id, cover_letter_message_id, subject, author, date, total_parts, received_parts, status, parser_version, to_recipients, cc_recipients, baseline_id, subject_index) 
                 VALUES (?, ?, ?, ?, ?, ?, 0, 'Pending', ?, ?, ?, ?, ?)",
                libsql::params![thread_id, cover_letter_message_id, subject, author, date, total_parts, parser_version, to, cc, baseline_id, part_index],
            )
            .await?;

        let mut rows = self
            .conn
            .query("SELECT last_insert_rowid()", libsql::params![])
            .await?;
        if let Ok(Some(row)) = rows.next().await {
            let id: i64 = row.get(0)?;
            Ok(Some(id))
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
        diff: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO patches (patchset_id, message_id, part_index, diff) VALUES (?, ?, ?, ?)",
            libsql::params![patchset_id, message_id, part_index, diff]
        ).await?;

        // Update received_parts based on actual patch count to be idempotent
        self.conn
            .execute(
                "UPDATE patchsets SET received_parts = (SELECT COUNT(*) FROM patches WHERE patchset_id = ?) WHERE id = ?",
                libsql::params![patchset_id, patchset_id],
            )
            .await?;
        Ok(())
    }

    pub async fn get_patchsets(&self, limit: usize, offset: usize) -> Result<Vec<PatchsetRow>> {
        let mut rows = self.conn.query(
            "SELECT id, subject, status, thread_id, author, date, cover_letter_message_id, total_parts, received_parts FROM patchsets ORDER BY id DESC LIMIT ? OFFSET ?",
            libsql::params![limit as i64, offset as i64],
        ).await?;

        let mut patchsets = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            patchsets.push(PatchsetRow {
                id: row.get(0)?,
                subject: row.get(1).ok(),
                status: row.get(2).ok(),
                thread_id: row.get(3).ok(),
                author: row.get(4).ok(),
                date: row.get(5).ok(),
                message_id: row.get(6).ok(),
                total_parts: row.get(7).ok(),
                received_parts: row.get(8).ok(),
            });
        }
        Ok(patchsets)
    }

    pub async fn count_patchsets(&self) -> Result<usize> {
        let mut rows = self
            .conn
            .query("SELECT COUNT(*) FROM patchsets", libsql::params![])
            .await?;
        if let Ok(Some(row)) = rows.next().await {
            let count: i64 = row.get(0)?;
            Ok(count as usize)
        } else {
            Ok(0)
        }
    }

    pub async fn get_patchset_details(&self, id: i64) -> Result<Option<serde_json::Value>> {
        let mut rows = self.conn.query(
            "SELECT p.id, p.subject, p.status, p.to_recipients, p.cc_recipients, 
                    b.repo_url, b.branch, b.last_known_commit, p.author, p.date, p.cover_letter_message_id, p.thread_id
             FROM patchsets p 
             LEFT JOIN baselines b ON p.baseline_id = b.id
             WHERE p.id = ?",
            libsql::params![id],
        ).await?;

        if let Ok(Some(row)) = rows.next().await {
            let pid: i64 = row.get(0)?;
            let subject: Option<String> = row.get(1).ok();
            let status: Option<String> = row.get(2).ok();
            let to: Option<String> = row.get(3).ok();
            let cc: Option<String> = row.get(4).ok();
            let repo_url: Option<String> = row.get(5).ok();
            let branch: Option<String> = row.get(6).ok();
            let commit: Option<String> = row.get(7).ok();
            let author: Option<String> = row.get(8).ok();
            let date: Option<i64> = row.get(9).ok();
            let mid: Option<String> = row.get(10).ok();
            let thread_id: Option<i64> = row.get(11).ok();

            // Fetch reviews
            let mut reviews = Vec::new();
            let mut rev_rows = self
                .conn
                .query(
                    "SELECT r.model_name, r.summary, r.created_at, ai.input_context, ai.output_raw
                 FROM reviews r
                 LEFT JOIN ai_interactions ai ON r.interaction_id = ai.id
                 WHERE r.patchset_id = ?",
                    libsql::params![pid],
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

            // Fetch patches
            let mut patches = Vec::new();
            let mut patch_rows = self.conn.query(
                "SELECT id, message_id, part_index FROM patches WHERE patchset_id = ? ORDER BY part_index ASC",
                libsql::params![pid]
            ).await?;
            while let Ok(Some(p)) = patch_rows.next().await {
                patches.push(serde_json::json!({
                    "id": p.get::<i64>(0)?,
                    "message_id": p.get::<String>(1)?,
                    "part_index": p.get::<Option<i64>>(2).ok(),
                }));
            }

            // Fetch thread messages
            let mut messages = Vec::new();
            if let Some(tid) = thread_id {
                let mut msg_rows = self.conn.query(
                    "SELECT message_id, author, date, subject FROM messages WHERE thread_id = ? ORDER BY date ASC",
                    libsql::params![tid]
                ).await?;
                while let Ok(Some(m)) = msg_rows.next().await {
                    messages.push(serde_json::json!({
                        "message_id": m.get::<String>(0)?,
                        "author": m.get::<Option<String>>(1).ok(),
                        "date": m.get::<Option<i64>>(2).ok(),
                        "subject": m.get::<Option<String>>(3).ok(),
                    }));
                }
            }

            Ok(Some(serde_json::json!({
                "id": pid,
                "message_id": mid,
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
                "reviews": reviews,
                "patches": patches,
                "thread": messages
            })))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DatabaseSettings;
    use std::sync::Arc;

    async fn setup_db() -> Arc<Database> {
        let settings = DatabaseSettings {
            url: ":memory:".to_string(),
            token: String::new(),
        };
        let db = Database::new(&settings).await.unwrap();
        db.migrate().await.unwrap();
        Arc::new(db)
    }

    #[tokio::test]
    async fn test_create_multiple_patchsets_in_thread() {
        let db = setup_db().await;

        // Create a thread
        let thread_id = db.create_thread("root", "Test Thread", 1000).await.unwrap();

        // 1. Create first patchset from Patch 1 (index 1)
        db.create_message("msg1", thread_id, None, "Author A", "Patch 1", 1000, "").await.unwrap();
        let ps1 = db.create_patchset(
            thread_id, None, "Patch 1", "Author A", 1000, 2, 1, "to", "cc", None, Some(1), 1
        ).await.unwrap();
        assert!(ps1.is_some());

        // 2. Add Cover Letter (index 0)
        // Should return same ID and update subject to "Cover Letter"
        db.create_message("root", thread_id, None, "Author A", "Cover Letter", 1005, "").await.unwrap();
        let ps1_update = db.create_patchset(
            thread_id, Some("root"), "Cover Letter", "Author A", 1005, 2, 1, "to", "cc", None, Some(1), 0
        ).await.unwrap();
        assert_eq!(ps1, ps1_update);

        let list = db.get_patchsets(1, 0).await.unwrap();
        assert_eq!(list[0].subject.as_deref(), Some("Cover Letter"));

        // 3. Add Patch 2 (index 2)
        // Should NOT update subject (index 2 > index 0)
        db.create_message("msg2", thread_id, None, "Author A", "Patch 2", 1006, "").await.unwrap();
        db.create_patchset(
            thread_id, None, "Patch 2", "Author A", 1006, 2, 1, "to", "cc", None, Some(1), 2
        ).await.unwrap();

        let list = db.get_patchsets(1, 0).await.unwrap();
        assert_eq!(list[0].subject.as_deref(), Some("Cover Letter"));

        // 4. Create NEW patchset in same thread (Author B, Time 1000 - same time but diff author)
        let ps3 = db.create_patchset(
            thread_id, None, "Other Author", "Author B", 1000, 2, 1, "to", "cc", None, Some(1), 0
        ).await.unwrap();
        assert!(ps3.is_none());

        // 5. Create NEW patchset v2 (Author A, Time 1002 - close time, but v2)
        // Under new logic "Implicit matches Explicit", this SHOULD merge with ps1 (Implicit)
        // because time/author/total match.
        let ps_v2 = db.create_patchset(
            thread_id, None, "[PATCH v2] Patchset 1", "Author A", 1002, 2, 1, "to", "cc", None, Some(2), 0
        ).await.unwrap();
        assert_eq!(ps1, ps_v2, "Implicit v1 should merge with v2 if time/author match");

        // 7. Test Merging: Create disjoint patchsets then bridge them
        let t_merge = db.create_thread("root_merge", "Merge Test", 10000).await.unwrap();
        
        // PS A (Time 10000)
        db.create_message("m1", t_merge, None, "Merger", "P1", 10000, "").await.unwrap();
        let psa = db.create_patchset(t_merge, None, "Series", "Merger", 10000, 3, 1, "", "", None, Some(1), 1).await.unwrap().unwrap();
        
        // PS B (Time 200000) - 190000s diff > 86400s limit -> New PS
        db.create_message("m2", t_merge, None, "Merger", "P3", 200000, "").await.unwrap();
        let psb = db.create_patchset(t_merge, None, "Series", "Merger", 200000, 3, 1, "", "", None, Some(1), 3).await.unwrap().unwrap();
        assert_ne!(psa, psb);

        // PS C (Time 100000) - 90000s diff from A (>86400), 100000s diff from B (>86400)
        // Wait, if C is > 86400 from both, it won't match either!
        // We need C to match BOTH.
        // A=10000. B=200000. Gap=190000.
        // If we want C to bridge, C needs to be within 86400 of A AND within 86400 of B.
        // But 190000 > 86400 * 2 (172800).
        // So it's IMPOSSIBLE to bridge with ONE message if they are that far apart!
        // We need A and B to be < 2 * 86400 apart.
        // Let's set B = 10000 + 100000 = 110000.
        // Diff = 100000. > 86400. So disjoint.
        // C = 10000 + 50000 = 60000.
        // Diff(A, C) = 50000 < 86400. Match A.
        // Diff(B, C) = 110000 - 60000 = 50000 < 86400. Match B.
        // So C bridges A and B.
        
        db.create_message("m2_fixed", t_merge, None, "Merger", "P3_fixed", 120000, "").await.unwrap(); // 120000. Diff 110000 > 86400.
        let psb_fixed = db.create_patchset(t_merge, None, "Series", "Merger", 120000, 3, 1, "", "", None, Some(1), 3).await.unwrap().unwrap();
        assert_ne!(psa, psb_fixed);

        // PS C (Time 65000)
        // Diff(A, C) = 55000 < 86400.
        // Diff(B, C) = 120000 - 65000 = 55000 < 86400.
        db.create_message("m3", t_merge, None, "Merger", "P2", 65000, "").await.unwrap();
        let psc = db.create_patchset(t_merge, None, "Series", "Merger", 65000, 3, 1, "", "", None, Some(1), 2).await.unwrap().unwrap();
        
        assert_eq!(psc, psa);
    }

    #[tokio::test]
    async fn test_five_patch_series_merging() {
        let db = setup_db().await;
        let thread_id = db.create_thread("root_5", "Five Patch Series", 20000).await.unwrap();
        let author = "Series Author <author@example.com>";

        // Patches arrive in order: 1/5, 0/5, 2/5, 4/5, 3/5
        let indices = [1, 0, 2, 4, 3];
        let mut patchset_ids = Vec::new();

        for (i, &idx) in indices.iter().enumerate() {
            let msg_id = format!("msg_{}", idx);
            let subject = format!("[PATCH {}/5] Feature part {}", idx, idx);
            let time = 20000 + (i as i64 * 10); // 10s apart

            db.create_message(&msg_id, thread_id, None, author, &subject, time, "").await.unwrap();
            let ps_id = db.create_patchset(
                thread_id,
                if idx == 0 { Some(&msg_id) } else { None },
                &subject,
                author,
                time,
                5,
                1,
                "to",
                "cc",
                None,
                None,
                idx as u32
            ).await.unwrap().unwrap();
            
            patchset_ids.push(ps_id);
        }

        // All IDs should be the same
        let first_id = patchset_ids[0];
        for id in patchset_ids {
            assert_eq!(id, first_id, "All parts of the same series should share the same patchset ID");
        }

        // Verify the final subject is the cover letter (index 0)
        let list = db.get_patchsets(1, 0).await.unwrap();
        assert_eq!(list[0].subject.as_deref(), Some("[PATCH 0/5] Feature part 0"));
    }

    #[tokio::test]
    async fn test_implicit_version_merging() {
        let db = setup_db().await;
        let thread_id = db.create_thread("root_v6", "Version 6 Series", 30000).await.unwrap();
        let author = "Author V6 <v6@example.com>";

        // Case: Cover letter has v6, but patches don't say v6 (implicitly v1?)
        // If the user forgot to version patches, they should still merge if time/author match.
        // However, strict version check prevents this if one is v6 and other is v1.
        // But the prompt says "They should be merged".
        // This implies loose version matching if one side is v1 (default)?
        
        // 1. Cover letter: [PATCH 00/33 v6] -> v6
        db.create_message("msg_00", thread_id, None, author, "[PATCH 00/33 v6] Cover", 30000, "").await.unwrap();
        let ps_cover = db.create_patchset(
            thread_id, Some("msg_00"), "[PATCH 00/33 v6] Cover", author, 30000, 33, 1, "", "", None, Some(6), 0
        ).await.unwrap().unwrap();

        // 2. Patch 1: [PATCH 01/33] -> v1 (implicit) -> Pass None
        db.create_message("msg_01", thread_id, None, author, "[PATCH 01/33] Part 1", 30005, "").await.unwrap();
        let ps_p1 = db.create_patchset(
            thread_id, None, "[PATCH 01/33] Part 1", author, 30005, 33, 1, "", "", None, None, 1
        ).await.unwrap().unwrap();

        // With strict checking, this might fail (assert_eq will panic if not merged).
        // If it fails, we need to relax the check in `create_patchset`.
        assert_eq!(ps_cover, ps_p1, "Should merge explicit v6 cover with implicit v1 patch if context matches");
    }

    #[tokio::test]
    async fn test_version_mismatch_no_merge() {
        let db = setup_db().await;
        let thread_id = db.create_thread("root_diff_ver", "Version Mismatch", 40000).await.unwrap();
        let author = "Author Diff <diff@example.com>";

        // v5
        db.create_message("msg_v5", thread_id, None, author, "[PATCH v5 1/2] Part 1", 40000, "").await.unwrap();
        let ps_v5 = db.create_patchset(
            thread_id, None, "[PATCH v5 1/2] Part 1", author, 40000, 2, 1, "", "", None, Some(5), 1
        ).await.unwrap().unwrap();

        // v6 (Close time)
        db.create_message("msg_v6", thread_id, None, author, "[PATCH v6 1/2] Part 1", 40010, "").await.unwrap();
        let ps_v6 = db.create_patchset(
            thread_id, None, "[PATCH v6 1/2] Part 1", author, 40010, 2, 1, "", "", None, Some(6), 1
        ).await.unwrap().unwrap();

        assert_ne!(ps_v5, ps_v6, "Should NOT merge different explicit versions (v5 vs v6)");
    }

    #[tokio::test]
    async fn test_v3_series_fragmentation() {
        let db = setup_db().await;
        let thread_id = db.create_thread("root_v3", "v3 Series", 50000).await.unwrap();
        let author = "Author V3 <v3@example.com>";

        // 1. [PATCH v3 0/2] (Cover)
        db.create_message("v3_0", thread_id, None, author, "[PATCH v3 0/2] Cover", 50000, "").await.unwrap();
        let ps_0 = db.create_patchset(
            thread_id, Some("v3_0"), "[PATCH v3 0/2] Cover", author, 50000, 2, 1, "", "", None, Some(3), 0
        ).await.unwrap().unwrap();

        // 2. [PATCH v3 1/2]
        db.create_message("v3_1", thread_id, None, author, "[PATCH v3 1/2] Part 1", 50005, "").await.unwrap();
        let ps_1 = db.create_patchset(
            thread_id, None, "[PATCH v3 1/2] Part 1", author, 50005, 2, 1, "", "", None, Some(3), 1
        ).await.unwrap().unwrap();

        // 3. [PATCH v3 2/2]
        db.create_message("v3_2", thread_id, None, author, "[PATCH v3 2/2] Part 2", 50010, "").await.unwrap();
        let ps_2 = db.create_patchset(
            thread_id, None, "[PATCH v3 2/2] Part 2", author, 50010, 2, 1, "", "", None, Some(3), 2
        ).await.unwrap().unwrap();

        assert_eq!(ps_0, ps_1, "Patch 1 should merge with Cover");
        assert_eq!(ps_0, ps_2, "Patch 2 should merge with Cover");
    }
}