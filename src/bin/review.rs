use anyhow::Result;
use clap::Parser;
use sashiko::{
    agent::{Agent, prompts::PromptRegistry, tools::ToolBox},
    ai::gemini::GeminiClient,
    db::Database,
    git_ops::GitWorktree,
    settings::Settings,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Read;
use std::path::PathBuf;
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long)]
    patchset: Option<i64>,

    #[arg(long)]
    message_id: Option<String>,

    /// Read patchset data from JSON via Stdin.
    #[arg(long)]
    json: bool,

    /// Git revision to use as baseline (e.g. "HEAD", "v6.12", or commit hash).
    /// Defaults to "HEAD" if not specified.
    #[arg(long)]
    baseline: Option<String>,

    /// Parent directory for creating worktrees.
    #[arg(long)]
    worktree_dir: Option<PathBuf>,

    #[arg(long, default_value = "review-prompts")]
    prompts: PathBuf,

    #[arg(long)]
    model: Option<String>,
}

#[derive(Deserialize, Serialize, Debug)]
struct PatchInput {
    index: i64,
    diff: String,
}

#[derive(Deserialize, Serialize, Debug)]
struct ReviewInput {
    id: i64,
    subject: String,
    patches: Vec<PatchInput>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let settings = Settings::new().unwrap();

    // Data Loading Strategy: DB vs JSON Stdin
    let (patchset_id, subject, patches) = if args.json {
        // Read from Stdin
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        let input: ReviewInput = serde_json::from_str(&buffer)?;

        info!(
            "Loaded patchset via JSON: {} (ID: {})",
            input.subject, input.id
        );
        (input.id, input.subject, input.patches)
    } else {
        // Read from DB
        let db = Database::new(&settings.database).await?;

        // Check patchset exists
        let patchset_json = if let Some(id) = args.patchset {
            db.get_patchset_details(id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Patchset {} not found", id))?
        } else if let Some(ref msg_id) = args.message_id {
            db.get_patchset_details_by_msgid(msg_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Patchset for message ID {} not found", msg_id))?
        } else {
            return Err(anyhow::anyhow!(
                "Either --patchset, --message-id, or --json must be provided"
            ));
        };

        let pid = patchset_json["id"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("Patchset ID not found in database response"))?;
        let subj = patchset_json["subject"]
            .as_str()
            .unwrap_or("Unknown")
            .to_string();

        info!("Reviewing patchset: {} (ID: {})", subj, pid);

        let db_diffs = db.get_patch_diffs(pid).await?;
        let patches = db_diffs
            .into_iter()
            .map(|(idx, diff)| PatchInput { index: idx, diff })
            .collect();

        (pid, subj, patches)
    };

    let baseline = args.baseline.unwrap_or_else(|| "HEAD".to_string());
    info!("Using baseline: {}", baseline);

    let repo_path = PathBuf::from(&settings.git.repository_path);
    // Use provided or default baseline
    let worktree = GitWorktree::new(&repo_path, &baseline, args.worktree_dir.as_deref()).await?;

    info!("Created worktree at {:?}", worktree.path);
    info!("Found {} patches to apply", patches.len());

    let mut patch_results = Vec::new();
    let mut all_applied = true;

    for p in &patches {
        info!("Applying patch part {}", p.index);
        match worktree.apply_raw_diff(&p.diff).await {
            Ok(output) => {
                let status = if output.status.success() {
                    "applied"
                } else {
                    all_applied = false;
                    "failed"
                };
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                if status == "failed" {
                    info!("Failed to apply patch {}: {}", p.index, stderr);
                }

                patch_results.push(json!({
                    "index": p.index,
                    "status": status,
                    "stdout": stdout,
                    "stderr": stderr,
                    "exit_code": output.status.code()
                }));
            }
            Err(e) => {
                all_applied = false;
                info!("Error applying patch {}: {}", p.index, e);
                patch_results.push(json!({
                    "index": p.index,
                    "status": "error",
                    "error": e.to_string()
                }));
            }
        }
    }

    let mut review_content = None;

    if all_applied {
        info!("All patches applied. Starting AI review...");
        let model_name = args.model.unwrap_or_else(|| settings.ai.model.clone());
        let client = GeminiClient::new(model_name);
        let tools = ToolBox::new(worktree.path.clone(), args.prompts.clone());
        let prompts = PromptRegistry::new(args.prompts.clone());
        let mut agent = Agent::new(client, tools, prompts);

        let patchset_val = json!({
            "id": patchset_id,
            "subject": subject,
            "patches": patches
        });

        match agent.run(patchset_val).await {
            Ok(review) => {
                info!("AI review completed.");
                review_content = Some(review);
            }
            Err(e) => {
                error!("AI review failed: {}", e);
            }
        }
    } else {
        info!("Not all patches applied successfully. Skipping AI review.");
    }

    let result = json!({
        "patchset_id": patchset_id,
        "baseline": baseline,
        "patches": patch_results,
        "review": review_content
    });

    println!("{}", serde_json::to_string_pretty(&result)?);

    worktree.remove().await?;

    Ok(())
}
