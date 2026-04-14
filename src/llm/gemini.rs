use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;
use tracing::info;

use crate::llm::{
    LlmProvider, PlanRequest, compose_user_prompt, map_http_error, parse_plan_response,
    system_prompt,
};
use crate::plan::ExecutionPlan;
use crate::{FagentError, Result};

#[derive(Debug, Clone)]
pub struct GeminiProvider {
    client: Client,
    api_key: String,
}

impl GeminiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
        }
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn plan(&self, request: &PlanRequest) -> Result<ExecutionPlan> {
        let endpoint = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
            request.model
        );
        let payload = json!({
            "systemInstruction": {
                "parts": [{ "text": system_prompt() }]
            },
            "contents": [{
                "role": "user",
                "parts": [{ "text": compose_user_prompt(request) }]
            }],
            "generationConfig": {
                "temperature": 0,
                "responseMimeType": "application/json"
            }
        });

        info!(model = %request.model, endpoint = %endpoint, "sending Gemini request");
        info!(payload = %payload, "Gemini request payload");

        let response = self
            .client
            .post(&endpoint)
            .header("x-goog-api-key", &self.api_key)
            .json(&payload)
            .send()
            .await
            .map_err(|error| map_http_error("Gemini request", error))?
            .error_for_status()
            .map_err(|error| map_http_error("Gemini response status", error))?;
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|error| map_http_error("Gemini response decode", error))?;
        info!(response = %value, "Gemini raw response");
        let content = value
            .get("candidates")
            .and_then(|candidates| candidates.get(0))
            .and_then(|candidate| candidate.get("content"))
            .and_then(|content| content.get("parts"))
            .and_then(|parts| parts.get(0))
            .and_then(|part| part.get("text"))
            .and_then(|text| text.as_str())
            .ok_or_else(|| {
                FagentError::Provider("Gemini response did not include text content".into())
            })?;

        info!(content = %content, "Gemini extracted response content");

        parse_plan_response(content)
    }
}
