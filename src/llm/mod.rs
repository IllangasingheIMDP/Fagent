mod anthropic;
mod gemini;
mod ollama;
mod openai;

use async_trait::async_trait;

use crate::config::{ProviderKind, ResolvedConfig};
use crate::plan::ExecutionPlan;
use crate::{FagentError, Result};

pub use anthropic::AnthropicProvider;
pub use gemini::GeminiProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAiProvider;

#[derive(Debug, Clone)]
pub struct PlanRequest {
    pub instruction: String,
    pub model: String,
    pub workspace_root: String,
    pub scan_depth: usize,
    pub workspace_context_json: String,
    pub allow_global: bool,
    pub permanent_delete: bool,
}

impl PlanRequest {
    pub fn new(
        instruction: String,
        model: String,
        workspace_root: String,
        scan_depth: usize,
        workspace_context_json: String,
        allow_global: bool,
        permanent_delete: bool,
    ) -> Self {
        Self {
            instruction,
            model,
            workspace_root,
            scan_depth,
            workspace_context_json,
            allow_global,
            permanent_delete,
        }
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn plan(&self, request: &PlanRequest) -> Result<ExecutionPlan>;
}

pub fn build_provider(config: &ResolvedConfig) -> Result<Box<dyn LlmProvider>> {
    match config.provider {
        ProviderKind::OpenAi => Ok(Box::new(OpenAiProvider::new(
            config
                .api_key
                .clone()
                .ok_or_else(|| FagentError::Config("missing OpenAI API key".into()))?,
        ))),
        ProviderKind::Anthropic => Ok(Box::new(AnthropicProvider::new(
            config
                .api_key
                .clone()
                .ok_or_else(|| FagentError::Config("missing Anthropic API key".into()))?,
        ))),
        ProviderKind::Gemini => Ok(Box::new(GeminiProvider::new(
            config
                .api_key
                .clone()
                .ok_or_else(|| FagentError::Config("missing Gemini API key".into()))?,
        ))),
        ProviderKind::Ollama => Ok(Box::new(OllamaProvider::new(
            config.ollama_base_url.clone(),
        ))),
    }
}

pub fn system_prompt() -> String {
    r#"You are Fagent, a filesystem planning model.

Return only valid JSON matching this schema:
{
  "workspace_root": "string or null",
  "warnings": ["string", "..."],
  "actions": [
    {
      "id": "string",
      "kind": "create_dir | move_file | rename_path | delete_path",
      "source": "string or null",
      "destination": "string or null",
      "rationale": "string or null"
    }
  ]
}

Rules:
- Prefer relative paths within the provided workspace.
- Do not invent files that are not present in the workspace context.
- Use delete_path only when the instruction clearly asks for deletion.
- Include create_dir before writing into a new directory when that makes the plan clearer.
- Keep the action list minimal and sequential.
- Never include explanations outside the JSON payload."#
        .into()
}

pub fn compose_user_prompt(request: &PlanRequest) -> String {
    format!(
        "Instruction:\n{}\n\nWorkspace root:\n{}\n\nScan depth:\n{}\n\nPolicy:\n- allow_global: {}\n- permanent_delete: {}\n\nWorkspace context JSON:\n{}",
        request.instruction,
        request.workspace_root,
        request.scan_depth,
        request.allow_global,
        request.permanent_delete,
        request.workspace_context_json
    )
}

pub fn parse_plan_response(response: &str) -> Result<ExecutionPlan> {
    let trimmed = response.trim();
    let candidates = [
        trimmed.to_string(),
        trimmed
            .strip_prefix("```json")
            .and_then(|value| value.strip_suffix("```"))
            .map(str::trim)
            .unwrap_or(trimmed)
            .to_string(),
        trimmed
            .strip_prefix("```")
            .and_then(|value| value.strip_suffix("```"))
            .map(str::trim)
            .unwrap_or(trimmed)
            .to_string(),
    ];

    for candidate in candidates {
        if let Ok(plan) = serde_json::from_str::<ExecutionPlan>(&candidate) {
            return Ok(plan);
        }
    }

    Err(FagentError::Provider(
        "model response did not contain a valid execution plan JSON object".into(),
    ))
}

pub fn extract_text_from_content_array(value: &serde_json::Value) -> Option<String> {
    value.as_array().and_then(|parts| {
        let mut text = String::new();
        for part in parts {
            if let Some(chunk) = part.get("text").and_then(|item| item.as_str()) {
                text.push_str(chunk);
            }
        }
        (!text.is_empty()).then_some(text)
    })
}

#[cfg(test)]
mod tests {
    use crate::plan::ActionKind;

    use super::parse_plan_response;

    #[test]
    fn parses_json_code_fence() {
        let response = r#"```json
{"workspace_root":null,"warnings":[],"actions":[{"id":"1","kind":"create_dir","source":null,"destination":"docs","rationale":"prepare folder"}]}
```"#;

        let plan = parse_plan_response(response).unwrap();
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[0].kind, ActionKind::CreateDir);
    }
}
