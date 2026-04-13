use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

use crate::llm::{
    LlmProvider, PlanRequest, compose_user_prompt, parse_plan_response, system_prompt,
};
use crate::plan::ExecutionPlan;
use crate::{FagentError, Result};

#[derive(Debug, Clone)]
pub struct OllamaProvider {
    client: Client,
    base_url: String,
}

impl OllamaProvider {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn plan(&self, request: &PlanRequest) -> Result<ExecutionPlan> {
        let endpoint = format!("{}/api/chat", self.base_url);
        let payload = json!({
            "model": request.model,
            "stream": false,
            "format": "json",
            "messages": [
                { "role": "system", "content": system_prompt() },
                { "role": "user", "content": compose_user_prompt(request) }
            ]
        });

        let response = self
            .client
            .post(endpoint)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;
        let value: serde_json::Value = response.json().await?;
        let content = value
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .ok_or_else(|| {
                FagentError::Provider("Ollama response did not include message content".into())
            })?;

        parse_plan_response(content)
    }
}
