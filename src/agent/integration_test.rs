#[cfg(test)]
mod tests {
    use crate::agent::{Agent, prompts::PromptRegistry, tools::ToolBox};
    use crate::ai::gemini::{
        Candidate, Content, GenAiClient, GenerateContentRequest, GenerateContentResponse, Part,
        UsageMetadata,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::path::PathBuf;

    struct MockClient;

    #[async_trait]
    impl GenAiClient for MockClient {
        async fn generate_content(
            &self,
            _req: GenerateContentRequest,
        ) -> anyhow::Result<GenerateContentResponse> {
            let mock_response = json!({
                "analysis_trace": ["Trace 1", "Trace 2"],
                "summary": "Mock summary",
                "score": 10,
                "verdict": "Pass",
                "findings": []
            });

            let content = Content {
                role: "model".to_string(),
                parts: vec![Part::Text {
                    text: format!("```json\n{}\n```", mock_response.to_string()),
                    thought_signature: None,
                }],
            };

            Ok(GenerateContentResponse {
                candidates: Some(vec![Candidate {
                    content,
                    finish_reason: Some("STOP".to_string()),
                }]),
                usage_metadata: Some(UsageMetadata {
                    prompt_token_count: 10,
                    candidates_token_count: Some(20),
                    total_token_count: 30,
                    extra: None,
                }),
            })
        }
    }

    fn get_test_paths() -> (PathBuf, PathBuf) {
        let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let linux_path = root.join("linux");
        let prompts_path = root.join("review-prompts");
        (linux_path, prompts_path)
    }

    #[tokio::test]
    async fn test_agent_integration_sanity() {
        let _ = tracing_subscriber::fmt::try_init();

        let (linux_path, prompts_path) = get_test_paths();

        // Setup dependencies
        // Use MockClient to simulate LLM interaction without network/API key
        let client = Box::new(MockClient);
        let tools = ToolBox::new(linux_path, prompts_path);
        let prompts = PromptRegistry::new(PathBuf::from("review-prompts"));

        let mut agent = Agent::new(client, tools, prompts, 150_000);

        // Create a dummy patchset that invites checking a file
        let patchset = json!({
            "subject": "Documentation: Fix typo in README",
            "author": "Test User <test@example.com>",
            "patches": [
                {
                    "index": 1,
                    "diff": "diff --git a/README b/README\nindex 1234567..89abcdef 100644\n--- a/README\n+++ b/README\n@@ -1,1 +1,1 @@\n-Linux kernel\n+The Linux kernel\n"
                }
            ]
        });

        let result = agent.run(patchset).await;

        match result {
            Ok(agent_res) => {
                if let Some(err) = agent_res.error {
                    panic!("Agent returned error: {}", err);
                }

                let review = agent_res
                    .output
                    .expect("Review output should be present on success");

                assert_eq!(review["summary"], "Mock summary");
                assert_eq!(review["verdict"], "Pass");
                println!("Agent review output: {}", review);
            }
            Err(e) => {
                panic!("Agent run failed: {}", e);
            }
        }
    }
}
