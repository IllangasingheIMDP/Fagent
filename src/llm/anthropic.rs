use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

use crate::llm::{
    LlmProvider, PlanRequest, compose_user_prompt, extract_text_from_content_array, map_http_error,
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
            .await
            .map_err(|error| map_http_error("Anthropic request", error))?
            .error_for_status()
            .map_err(|error| map_http_error("Anthropic response status", error))?;
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|error| map_http_error("Anthropic response decode", error))?;
        let content = extract_text_from_content_array(value.get("content").ok_or_else(|| {
            FagentError::Provider("Anthropic response did not include content".into())
        })?)
        .ok_or_else(|| {
            FagentError::Provider("Anthropic response did not include text content".into())
        })?;

        parse_plan_response(&content)
    }
    async fn call(&self, request: &crate::llm::AgentRequest) -> Result<crate::llm::LlmResponse> {
        let system = request
            .messages
            .iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let messages=request.messages.iter().filter(|m|m.role!="system").map(|m|json!({"role":if m.role=="assistant"{"assistant"}else{"user"},"content":m.content})).collect::<Vec<_>>();
        let payload = json!({"model":request.model,"max_tokens":1800,"system":system,"messages":messages,"tools":request.tools.iter().map(|t|json!({"name":t.name,"description":t.description,"input_schema":t.parameters})).collect::<Vec<_>>()});
        let value: serde_json::Value = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&payload)
            .send()
            .await
            .map_err(|e| map_http_error("Anthropic request", e))?
            .error_for_status()
            .map_err(|e| map_http_error("Anthropic response status", e))?
            .json()
            .await
            .map_err(|e| map_http_error("Anthropic response decode", e))?;
        if let Some(calls) = value
            .get("content")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter(|p| p.get("type").and_then(|x| x.as_str()) == Some("tool_use"))
                    .collect::<Vec<_>>()
            })
            .filter(|a| !a.is_empty())
        {
            return Ok(crate::llm::LlmResponse::ToolCalls(
                calls
                    .into_iter()
                    .map(|p| crate::llm::ToolCallRequest {
                        id: p
                            .get("id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("tool-call")
                            .into(),
                        name: p
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or_default()
                            .into(),
                        params: p.get("input").cloned().unwrap_or_else(|| json!({})),
                    })
                    .collect(),
            ));
        }
        Ok(crate::llm::LlmResponse::Message(
            extract_text_from_content_array(value.get("content").unwrap_or(&json!([])))
                .unwrap_or_default(),
        ))
    }
}
