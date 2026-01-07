use crate::ai::gemini::{Content, CreateCachedContentRequest, GenAiClient, Part, Tool};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::fs;

pub struct CacheManager {
    base_dir: PathBuf,
    client: Box<dyn GenAiClient>,
    model: String,
    ttl: String,
    tools: Option<Vec<Tool>>,
}

impl CacheManager {
    pub fn new(
        base_dir: PathBuf,
        client: Box<dyn GenAiClient>,
        model: String,
        ttl: String,
        tools: Option<Vec<Tool>>,
    ) -> Self {
        Self {
            base_dir,
            client,
            model,
            ttl,
            tools,
        }
    }

    /// Builds the full context string from prompts and docs.
    async fn build_context(&self) -> Result<String> {
        let mut context = String::new();

        // 1. System Persona (Prompt + review-core.md)
        context.push_str("You're an expert Linux kernel developer and maintainer with deep knowledge of Linux, Operating Systems, modern hardware and Linux community standards and processes.\n\n");

        let core_path = self.base_dir.join("review-core.md");
        if core_path.exists() {
            context.push_str(&fs::read_to_string(&core_path).await?);
            context.push_str("\n\n");
        }

        // 2. Prompt Library
        context.push_str("# Subsystem Guidelines\n\n");

        // Read root of prompts dir
        let mut entries = fs::read_dir(&self.base_dir).await?;
        let mut paths = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            paths.push(entry.path());
        }
        paths.sort(); // Deterministic order

        for path in paths {
            if path.extension().is_some_and(|ext| ext == "md") {
                let fname = path.file_name().unwrap().to_string_lossy();
                if fname == "review-core.md"
                    || fname == "README.md"
                    || fname == "review-one.md"
                    || fname == "review-stat.md"
                {
                    continue;
                }
                context.push_str(&format!("## {}\n", fname));
                context.push_str(&fs::read_to_string(&path).await?);
                context.push_str("\n\n");
            }
        }

        // Patterns
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

    fn calculate_hash(&self, content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content);
        // Also hash tools signature if present, so we rotate cache if tools change
        if let Some(tools) = &self.tools {
            if let Ok(json) = serde_json::to_string(tools) {
                hasher.update(json);
            }
        }
        format!("{:x}", hasher.finalize())
    }

    /// Ensures a valid cache exists for the current content.
    /// Returns the cache resource name (e.g., "cachedContents/123...").
    pub async fn ensure_cache(&self) -> Result<String> {
        let context_str = self.build_context().await?;
        let hash = self.calculate_hash(&context_str);
        // Short hash for readability
        let short_hash = &hash[..8];
        let expected_display_name = format!("sashiko-reviewer-v1-{}", short_hash);
        // The caching API requires the model name to start with "models/"
        let model_name = format!("models/{}", self.model);

        // List existing caches
        let existing = self.client.list_cached_contents().await?;

        for cache in existing {
            if let Some(dn) = &cache.display_name {
                if dn == &expected_display_name && cache.model == model_name {
                    if let Some(name) = cache.name {
                        tracing::info!(
                            "Found existing cache: {} ({} for {})",
                            name,
                            expected_display_name,
                            model_name
                        );
                        return Ok(name);
                    }
                }
            }
        }

        tracing::info!("Creating new cache: {}", expected_display_name);

        // Create new cache
        // model_name is already defined above

        let request = CreateCachedContentRequest {
            model: model_name,
            display_name: Some(expected_display_name),
            system_instruction: Some(Content {
                role: "system".to_string(),
                parts: vec![Part::Text {
                    text: "You are an expert Linux kernel reviewer.".to_string(), // Brief intro
                    thought_signature: None,
                }],
            }),
            contents: Some(vec![Content {
                role: "user".to_string(),
                parts: vec![Part::Text {
                    text: context_str,
                    thought_signature: None,
                }],
            }]),
            tools: self.tools.clone(),
            ttl: Some(self.ttl.clone()),
        };

        let cached_content = self.client.create_cached_content(request).await?;
        cached_content.name.context("Created cache has no name")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::gemini::{
        CachedContent, CreateCachedContentRequest, GenerateContentRequest, GenerateContentResponse,
        GenerateContentWithCacheRequest,
    };
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    struct MockGenAiClient {
        created_request: Arc<Mutex<Option<CreateCachedContentRequest>>>,
    }

    impl MockGenAiClient {}

    #[async_trait]
    impl GenAiClient for MockGenAiClient {
        async fn generate_content(
            &self,
            _request: GenerateContentRequest,
        ) -> Result<GenerateContentResponse> {
            unimplemented!()
        }

        async fn create_cached_content(
            &self,
            request: CreateCachedContentRequest,
        ) -> Result<CachedContent> {
            *self.created_request.lock().unwrap() = Some(request);
            Ok(CachedContent {
                name: Some("cachedContents/test".to_string()),
                display_name: None,
                model: "models/test".to_string(),
                system_instruction: None,
                contents: None,
                tools: None,
                create_time: None,
                update_time: None,
                expire_time: None,
                ttl: None,
            })
        }

        async fn list_cached_contents(&self) -> Result<Vec<CachedContent>> {
            Ok(vec![])
        }

        async fn generate_content_with_cache(
            &self,
            _request: GenerateContentWithCacheRequest,
        ) -> Result<GenerateContentResponse> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn test_ensure_cache_creates_with_correct_ttl() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_dir = temp_dir.path().to_path_buf();

        let captured = Arc::new(Mutex::new(None));
        let mock_client = MockGenAiClient {
            created_request: captured.clone(),
        };

        let manager = CacheManager::new(
            base_dir,
            Box::new(mock_client),
            "gemini-1.5-flash-002".to_string(),
            "60s".to_string(),
            None,
        );

        let res = manager.ensure_cache().await;
        assert!(res.is_ok());

        let request = captured
            .lock()
            .unwrap()
            .take()
            .expect("create_cached_content not called");
        assert_eq!(request.ttl, Some("60s".to_string()));
        // Also verify model name is prefixed
        assert_eq!(request.model, "models/gemini-1.5-flash-002");
    }

    struct MockGenAiClientWithExisting {
        existing: Vec<CachedContent>,
        created_request: Arc<Mutex<Option<CreateCachedContentRequest>>>,
    }

    #[async_trait]
    impl GenAiClient for MockGenAiClientWithExisting {
        async fn generate_content(
            &self,
            _request: GenerateContentRequest,
        ) -> Result<GenerateContentResponse> {
            unimplemented!()
        }

        async fn create_cached_content(
            &self,
            request: CreateCachedContentRequest,
        ) -> Result<CachedContent> {
            *self.created_request.lock().unwrap() = Some(request);
            Ok(CachedContent {
                name: Some("cachedContents/new".to_string()),
                display_name: None,
                model: "models/test".to_string(),
                system_instruction: None,
                contents: None,
                tools: None,
                create_time: None,
                update_time: None,
                expire_time: None,
                ttl: None,
            })
        }

        async fn list_cached_contents(&self) -> Result<Vec<CachedContent>> {
            Ok(self.existing.clone())
        }

        async fn generate_content_with_cache(
            &self,
            _request: GenerateContentWithCacheRequest,
        ) -> Result<GenerateContentResponse> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn test_ensure_cache_ignores_wrong_model() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_dir = temp_dir.path().to_path_buf();

        // Construct the expected context string for an empty dir
        let context_str = "You're an expert Linux kernel developer and maintainer with deep knowledge of Linux, Operating Systems, modern hardware and Linux community standards and processes.\n\n# Subsystem Guidelines\n\n";
        let mut hasher = Sha256::new();
        hasher.update(context_str);
        // Tools are None
        let hash = format!("{:x}", hasher.finalize());
        let short_hash = &hash[..8];
        let expected_dn = format!("sashiko-reviewer-v1-{}", short_hash);

        let wrong_model_cache = CachedContent {
            name: Some("cachedContents/wrong".to_string()),
            display_name: Some(expected_dn.clone()),
            model: "models/gemini-wrong".to_string(), // Mismatch
            system_instruction: None,
            contents: None,
            tools: None,
            create_time: None,
            update_time: None,
            expire_time: None,
            ttl: None,
        };

        let captured = Arc::new(Mutex::new(None));
        let mock_client = MockGenAiClientWithExisting {
            existing: vec![wrong_model_cache],
            created_request: captured.clone(),
        };

        let manager = CacheManager::new(
            base_dir,
            Box::new(mock_client),
            "gemini-right".to_string(),
            "60s".to_string(),
            None,
        );

        // This should trigger creation because existing cache has wrong model
        let res = manager.ensure_cache().await;
        assert!(res.is_ok());

        let request = captured
            .lock()
            .unwrap()
            .take()
            .expect("create_cached_content SHOULD be called when model mismatches");

        assert_eq!(request.model, "models/gemini-right");
    }
}
