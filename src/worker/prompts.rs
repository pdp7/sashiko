use anyhow::Result;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::fs;

/// System identity prompt - used across all AI interactions
pub const SYSTEM_IDENTITY: &str = "You're an expert Linux kernel developer and upstream maintainer with deep knowledge of Linux kernel, Operating Systems, CPU architectures, modern hardware and Linux kernel community standards and processes.";

/// Files to exclude from context building
const EXCLUDED_FILES: &[&str] = &[
    "review-core.md",
    "README.md",
    "review-one.md",
    "review-stat.md",
    "technical-patterns.md",
    "severity.md",
];

pub struct PromptRegistry {
    base_dir: PathBuf,
}

impl PromptRegistry {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn get_base_dir(&self) -> &PathBuf {
        &self.base_dir
    }

    /// Returns the system identity prompt
    pub fn get_system_identity() -> &'static str {
        SYSTEM_IDENTITY
    }

    /// Reads the review-core.md protocol file
    pub async fn get_review_core(&self) -> Result<String> {
        let core_path = self.base_dir.join("review-core.md");
        if core_path.exists() {
            Ok(fs::read_to_string(&core_path).await?)
        } else {
            Ok("Deep dive regression analysis protocol.".to_string())
        }
    }

    /// Builds the user task prompt for AI review
    ///
    /// When `use_cache` is true, the protocol is assumed to be in the cache,
    /// so we don't include review-core.md content.
    pub async fn get_user_task_prompt(&self, use_cache: bool) -> Result<String> {
        if use_cache {
            Ok(format!(
                "{}\nRun a deep dive regression analysis of the top commit in the Linux source tree.\n\n\
                 Follow the Review Protocol and all Technical patterns and Subsystem Guidelines available in your context.\n\
                 IMPORTANT: Don't try to load additional prompts using tools, even if guided otherwise, they all are preloaded in your context.\n\
                 IMPORTANT: If you find regressions, you MUST use the `write_file` tool to create `review-inline.txt` as specified in the protocol.",
                SYSTEM_IDENTITY
            ))
        } else {
            let review_core = self.get_review_core().await?;
            Ok(format!(
                "{} Using the prompt review-prompts/kernel/review-core.md run a deep dive regression analysis of the top commit in the Linux source tree.\n\n\
                 ## Review Protocol (review-core.md)\n\
                 {}\n\n\
                 IMPORTANT: If you find regressions, you MUST use the `write_file` tool to create `review-inline.txt` as specified in the protocol.",
                SYSTEM_IDENTITY, review_core
            ))
        }
    }

    /// Builds full context string from prompts directory for caching
    ///
    /// Includes:
    /// 1. System identity
    /// 2. review-core.md protocol
    /// 3. Subsystem guidelines (*.md files in root)
    /// 4. Technical patterns (*.md files in patterns/ subdirectory)
    pub async fn build_context(&self) -> Result<String> {
        let mut context = String::new();

        // 1. System identity
        context.push_str(SYSTEM_IDENTITY);
        context.push_str("\n\n");

        // 2. Review protocol (review-core.md)
        context.push_str("# review-code.md\n\n");
        let core_path = self.base_dir.join("review-core.md");
        if core_path.exists() {
            context.push_str(&fs::read_to_string(&core_path).await?);
            context.push_str("\n\n");
        }

        // Technical patterns
        let tech_path = self.base_dir.join("technical-patterns.md");
        if tech_path.exists() {
            context.push_str("## technical-patterns.md\n");
            context.push_str(&fs::read_to_string(&tech_path).await?);
            context.push_str("\n\n");
        }

        // Severity guidelines
        let sev_path = self.base_dir.join("severity.md");
        if sev_path.exists() {
            context.push_str("## severity.md\n");
            context.push_str(&fs::read_to_string(&sev_path).await?);
            context.push_str("\n\n");
        }

        // 3. Subsystem guidelines (root md files and subdirectories)
        context.push_str("# Subsystem Guidelines\n\n");

        let mut entries = fs::read_dir(&self.base_dir).await?;
        let mut paths = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            paths.push(entry.path());
        }
        paths.sort(); // Deterministic order

        // Directories to exclude from subsystem scan (patterns is handled separately)
        const EXCLUDED_DIRS: &[&str] = &[
            "scripts",
            "docs",
            "examples",
            "skills",
            "slash-commands",
            "agent",
            "patterns",
            ".git",
        ];

        for path in paths {
            if path.is_file() && path.extension().is_some_and(|ext| ext == "md") {
                let fname = path.file_name().unwrap().to_string_lossy();
                if EXCLUDED_FILES.contains(&fname.as_ref()) {
                    continue;
                }
                context.push_str(&format!("## {}\n", fname));
                context.push_str(&fs::read_to_string(&path).await?);
                context.push_str("\n\n");
            } else if path.is_dir() {
                let dir_name = path.file_name().unwrap().to_string_lossy();
                if EXCLUDED_DIRS.contains(&dir_name.as_ref()) {
                    continue;
                }

                // Recursively read .md files in this directory
                let mut sub_entries = fs::read_dir(&path).await?;
                let mut sub_paths = Vec::new();
                while let Some(entry) = sub_entries.next_entry().await? {
                    sub_paths.push(entry.path());
                }
                sub_paths.sort();

                for sub_path in sub_paths {
                    if sub_path.extension().is_some_and(|ext| ext == "md") {
                        let fname = sub_path.file_name().unwrap().to_string_lossy();
                        context.push_str(&format!("## {}/{}\n", dir_name, fname));
                        context.push_str(&fs::read_to_string(&sub_path).await?);
                        context.push_str("\n\n");
                    }
                }
            }
        }

        // 4. Technical patterns (patterns/ subdirectory)
        let patterns_dir = self.base_dir.join("patterns");
        if patterns_dir.exists() {
            context.push_str("# Technical Patterns\n\n");
            let mut p_entries = fs::read_dir(&patterns_dir).await?;
            let mut p_paths = Vec::new();
            while let Some(entry) = p_entries.next_entry().await? {
                p_paths.push(entry.path());
            }
            p_paths.sort();

            for path in p_paths {
                if path.extension().is_some_and(|ext| ext == "md") {
                    context.push_str(&format!(
                        "## {}\n",
                        path.file_name().unwrap().to_string_lossy()
                    ));
                    context.push_str(&fs::read_to_string(&path).await?);
                    context.push_str("\n\n");
                }
            }
        }

        Ok(context)
    }

    /// Calculates SHA256 hash of content, optionally including tools signature
    pub fn calculate_content_hash<T: serde::Serialize>(
        &self,
        content: &str,
        tools: Option<&[T]>,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content);

        // Also hash tools signature if present, so we rotate cache if tools change
        if let Some(tools) = tools {
            if let Ok(json) = serde_json::to_string(tools) {
                hasher.update(json);
            }
        }

        format!("{:x}", hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_identity_constant() {
        let identity = PromptRegistry::get_system_identity();
        assert!(identity.starts_with("You're an expert Linux kernel developer"));
        assert!(identity.contains("maintainer"));
    }

    #[test]
    fn test_content_hash_deterministic() {
        let temp_dir = tempfile::tempdir().unwrap();
        let registry = PromptRegistry::new(temp_dir.path().to_path_buf());

        let content = "test content";
        let hash1 = registry.calculate_content_hash::<()>(content, None);
        let hash2 = registry.calculate_content_hash::<()>(content, None);

        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA256 hex is 64 chars
    }

    #[test]
    fn test_content_hash_differs_with_tools() {
        let temp_dir = tempfile::tempdir().unwrap();
        let registry = PromptRegistry::new(temp_dir.path().to_path_buf());

        let content = "test content";
        let tools = vec!["tool1", "tool2"];

        let hash_no_tools = registry.calculate_content_hash::<()>(content, None);
        let hash_with_tools = registry.calculate_content_hash(content, Some(&tools));

        assert_ne!(hash_no_tools, hash_with_tools);
    }

    #[tokio::test]
    async fn test_build_context_includes_identity() {
        let temp_dir = tempfile::tempdir().unwrap();
        let registry = PromptRegistry::new(temp_dir.path().to_path_buf());

        let context = registry.build_context().await.unwrap();
        assert!(context.starts_with(SYSTEM_IDENTITY));
    }

    #[tokio::test]
    async fn test_build_context_includes_core() {
        let temp_dir = tempfile::tempdir().unwrap();
        let core_content = "# Test Protocol\nThis is a test.";
        std::fs::write(temp_dir.path().join("review-core.md"), core_content).unwrap();

        let registry = PromptRegistry::new(temp_dir.path().to_path_buf());
        let context = registry.build_context().await.unwrap();

        assert!(context.contains("# Test Protocol"));
        assert!(context.contains("This is a test."));
    }

    #[tokio::test]
    async fn test_build_context_excludes_readme() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("README.md"),
            "# README\nDo not include",
        )
        .unwrap();
        std::fs::write(
            temp_dir.path().join("subsystem.md"),
            "# Subsystem\nInclude me",
        )
        .unwrap();

        let registry = PromptRegistry::new(temp_dir.path().to_path_buf());
        let context = registry.build_context().await.unwrap();

        assert!(!context.contains("Do not include"));
        assert!(context.contains("Include me"));
    }

    #[tokio::test]
    async fn test_user_task_prompt_cached() {
        let temp_dir = tempfile::tempdir().unwrap();
        let registry = PromptRegistry::new(temp_dir.path().to_path_buf());

        let prompt = registry.get_user_task_prompt(true).await.unwrap();

        assert!(prompt.contains(SYSTEM_IDENTITY));
        assert!(prompt.contains("regression analysis"));
        assert!(!prompt.contains("## Review Protocol")); // No embedded protocol in cached mode
    }

    #[tokio::test]
    async fn test_user_task_prompt_non_cached() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("review-core.md"), "Protocol content").unwrap();

        let registry = PromptRegistry::new(temp_dir.path().to_path_buf());
        let prompt = registry.get_user_task_prompt(false).await.unwrap();

        assert!(prompt.contains(SYSTEM_IDENTITY));
        assert!(prompt.contains("## Review Protocol"));
        assert!(prompt.contains("Protocol content"));
    }

    #[tokio::test]
    async fn test_build_context_recursive_subsystems() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        // Create a root-level subsystem file
        std::fs::write(root.join("root_sub.md"), "Root Content").unwrap();

        // Create a valid nested subsystem
        let sub_dir = root.join("valid_sub");
        std::fs::create_dir(&sub_dir).unwrap();
        std::fs::write(sub_dir.join("rule1.md"), "Rule 1 Content").unwrap();

        // Create an excluded directory
        let script_dir = root.join("scripts");
        std::fs::create_dir(&script_dir).unwrap();
        std::fs::write(script_dir.join("ignore_me.md"), "Ignored Content").unwrap();

        let registry = PromptRegistry::new(root.to_path_buf());
        let context = registry.build_context().await.unwrap();

        assert!(context.contains("Root Content"));
        assert!(context.contains("## valid_sub/rule1.md"));
        assert!(context.contains("Rule 1 Content"));
        assert!(!context.contains("Ignored Content"));
    }
}
