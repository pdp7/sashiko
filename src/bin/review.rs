use anyhow::Result;
use clap::Parser;
use sashiko::{
    // agent::{Agent, tools::ToolBox, prompts::PromptRegistry},
    // ai::gemini::GeminiClient,
    db::Database,
    git_ops::GitWorktree,
    settings::Settings,
};
use std::path::PathBuf;
use tracing::info;
use serde_json::json;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long)]
    patchset: i64,

    /// Git revision to use as baseline (e.g. "HEAD", "v6.12", or commit hash)
    #[arg(long)]
    baseline: String,

    #[arg(long, default_value = "review-prompts")]
    prompts: PathBuf,

    #[arg(long, default_value = "gemini-1.5-pro-latest")]
    model: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let settings = Settings::new().unwrap();

    let db = Database::new(&settings.database).await?;

    // Check patchset exists
    let patchset_json = db.get_patchset_details(args.patchset).await?
        .ok_or_else(|| anyhow::anyhow!("Patchset {} not found", args.patchset))?;

    info!("Reviewing patchset: {}", patchset_json["subject"]);

    let repo_path = PathBuf::from(&settings.git.repository_path);
    // Use provided baseline
    let worktree = GitWorktree::new(&repo_path, &args.baseline).await?;

    info!("Created worktree at {:?}", worktree.path);

    let diffs = db.get_patch_diffs(args.patchset).await?;
    info!("Found {} patches to apply", diffs.len());
    
    let mut patch_results = Vec::new();

    for (idx, diff) in diffs {
        info!("Applying patch part {}", idx);
        match worktree.apply_raw_diff(&diff).await {
            Ok(output) => {
                let status = if output.status.success() { "applied" } else { "failed" };
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                
                if status == "failed" {
                     info!("Failed to apply patch {}: {}", idx, stderr);
                }

                patch_results.push(json!({
                    "index": idx,
                    "status": status,
                    "stdout": stdout,
                    "stderr": stderr,
                    "exit_code": output.status.code()
                }));
            },
            Err(e) => {
                info!("Error applying patch {}: {}", idx, e);
                patch_results.push(json!({
                    "index": idx,
                    "status": "error",
                    "error": e.to_string()
                }));
            }
        }
    }

    let result = json!({
        "patchset_id": args.patchset,
        "baseline": args.baseline,
        "patches": patch_results,
        "review": null
    });

    println!("{}", serde_json::to_string_pretty(&result)?);

    /*
    let client = GeminiClient::new(args.model)?;
    let tools = ToolBox::new(worktree.path.clone(), args.prompts.clone());
    let prompts = PromptRegistry::new(args.prompts);
    
    let mut agent = Agent::new(client, tools, prompts);
    
    match agent.run(patchset_json).await {
        Ok(review) => println!("Review:\n{}", review),
        Err(e) => eprintln!("Agent failed: {}", e),
    }
    */

    worktree.remove().await?;

    Ok(())
}