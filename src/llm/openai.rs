use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

use crate::llm::{
    LlmProvider, PlanRequest, compose_user_prompt, map_http_error, parse_plan_response,
    system_prompt,
};
use crate::plan::ExecutionPlan;
use crate::{FagentError, Result};

#[derive(Debug, Clone)]
pub struct OpenAiProvider {
    client: Client,
    api_key: String,
}

impl OpenAiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn plan(&self, request: &PlanRequest) -> Result<ExecutionPlan> {
        let payload = json!({
            "model": request.model,
            "temperature": 0,
            "response_format": { "type": "json_object" },
            "messages": [
                { "role": "system", "content": system_prompt() },
                { "role": "user", "content": compose_user_prompt(request) }
            ]
        });

        let response = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .map_err(|error| map_http_error("OpenAI request", error))?
            .error_for_status()
            .map_err(|error| map_http_error("OpenAI response status", error))?;
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|error| map_http_error("OpenAI response decode", error))?;
        let content = value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .ok_or_else(|| {
                FagentError::Provider("OpenAI response did not include message content".into())
            })?;

        parse_plan_response(content)
    }
}
