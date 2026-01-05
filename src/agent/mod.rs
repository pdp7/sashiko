pub mod prompts;
pub mod tools;

use crate::agent::prompts::PromptRegistry;
use crate::agent::tools::ToolBox;
use crate::ai::gemini::{Content, FunctionResponse, GeminiClient, GenerateContentRequest, Part};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use tracing::{info, warn};

pub struct Agent {
    client: GeminiClient,
    tools: ToolBox,
    prompts: PromptRegistry,
    history: Vec<Content>,
}

impl Agent {
    pub fn new(client: GeminiClient, tools: ToolBox, prompts: PromptRegistry) -> Self {
        Self {
            client,
            tools,
            prompts,
            history: Vec::new(),
        }
    }

    pub async fn run(&mut self, patchset: Value) -> Result<String> {
        let system_prompt = self.prompts.get_system_prompt().await?;
        let context_prompt = self.prompts.build_context_prompt(&patchset).await?;

        let initial_user_message = format!(
            "Review this patchset:\nSubject: {}\\nAuthor: {}\\n\n{}",
            patchset["subject"].as_str().unwrap_or("Unknown"),
            patchset["author"].as_str().unwrap_or("Unknown"),
            context_prompt
        );

        let system_content = Content {
            role: "user".to_string(), // Using user role for system instruction placeholder if needed, but we use the field.
            parts: vec![Part::Text {
                text: system_prompt,
                thought_signature: None,
            }],
        };

        self.history.push(Content {
            role: "user".to_string(),
            parts: vec![Part::Text {
                text: initial_user_message,
                thought_signature: None,
            }],
        });

        loop {
            let req = GenerateContentRequest {
                contents: self.history.clone(),
                tools: Some(vec![self.tools.get_declarations()]),
                system_instruction: Some(system_content.clone()),
            };

            info!("Sending request to Gemini...");
            let resp = self.client.generate_content(req).await?;

            let candidate = resp
                .candidates
                .as_ref()
                .and_then(|c| c.first())
                .ok_or_else(|| anyhow!("No candidates returned"))?;

            let content = &candidate.content;
            self.history.push(content.clone());

            // Check for function calls
            let mut function_responses = Vec::new();
            let mut has_calls = false;
            let mut final_text = String::new();

            for part in &content.parts {
                match part {
                    Part::FunctionCall {
                        function_call: call,
                        ..
                    } => {
                        has_calls = true;
                        info!("Tool Call: {} args: {}", call.name, call.args);

                        let result = match self.tools.call(&call.name, call.args.clone()).await {
                            Ok(val) => val,
                            Err(e) => {
                                warn!("Tool execution failed: {}", e);
                                json!({ "error": e.to_string() })
                            }
                        };

                        function_responses.push(Part::FunctionResponse {
                            function_response: FunctionResponse {
                                name: call.name.clone(),
                                response: result,
                            },
                        });
                    }
                    Part::Text { text, .. } => {
                        final_text.push_str(text);
                    }
                    _ => {}
                }
            }

            if has_calls {
                let response_content = Content {
                    role: "function".to_string(),
                    parts: function_responses,
                };
                self.history.push(response_content);
                // Continue loop to get model response to tool outputs
            } else {
                return Ok(final_text);
            }
        }
    }
}
