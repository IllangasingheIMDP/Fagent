use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

use crate::llm::{
    LlmProvider, PlanRequest, compose_user_prompt, extract_text_from_content_array,
    parse_plan_response, system_prompt,
};
use crate::plan::ExecutionPlan;
use crate::{FagentError, Result};

#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    client: Client,
    api_key: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
        }
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn plan(&self, request: &PlanRequest) -> Result<ExecutionPlan> {
        let payload = json!({
            "model": request.model,
            "max_tokens": 1800,
            "temperature": 0,
            "system": system_prompt(),
            "messages": [
                { "role": "user", "content": compose_user_prompt(request) }
            ]
        });

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;
        let value: serde_json::Value = response.json().await?;
        let content = extract_text_from_content_array(value.get("content").ok_or_else(|| {
            FagentError::Provider("Anthropic response did not include content".into())
        })?)
        .ok_or_else(|| {
            FagentError::Provider("Anthropic response did not include text content".into())
        })?;

        parse_plan_response(&content)
    }
}
