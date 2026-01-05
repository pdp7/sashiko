use anyhow::Result;
use std::path::PathBuf;

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

    pub async fn get_system_prompt(&self) -> Result<String> {
        let identity = "You're an expert Linux kernel developer and maintainer with deepk knowledge of Linux, Operating Sytems, modern hardware and Linux community standards and processes.";
        let json_protocol = r#"
## Output Format
You must respond with a valid JSON object. Do not include markdown code blocks (```json ... ```) around the output, just the raw JSON. The JSON must adhere to this schema:

{
  "analysis_trace": [
    "string" // Step-by-step reasoning
  ],
  "summary": "Brief summary of the patchset",
  "score": number, // 0-10, where 10 is perfect
  "verdict": "string", // "Reviewed-by", "Acked-by", "Changes Requested"
  "findings": [
    {
      "file": "string",
      "line": number,
      "severity": "string", // "High", "Medium", "Low", "Style"
      "message": "string", // Technical explanation
      "suggestion": "string" // Optional: suggested fix
    }
  ]
}
"#;
        Ok(format!("{}\n{}", identity, json_protocol))
    }
}
