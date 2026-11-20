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
    endpoint: String,
    provider_name: &'static str,
}

impl OpenAiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            endpoint: "https://api.openai.com/v1/chat/completions".into(),
            provider_name: "OpenAI",
        }
    }

    pub fn new_groq(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            endpoint: "https://api.groq.com/openai/v1/chat/completions".into(),
            provider_name: "Groq",
        }
    }

    fn http_error(&self, stage: &str, error: reqwest::Error) -> FagentError {
        map_http_error(&format!("{} {stage}", self.provider_name), error)
    }

    fn messages(&self, request: &crate::llm::AgentRequest) -> Vec<serde_json::Value> {
        if self.provider_name == "Groq" {
            // Memory stores tool calls as assistant text, without protocol call IDs.
            // Send observations as user messages so Groq receives valid history.
            request
                .messages
                .iter()
                .map(|message| {
                    if message.role == "tool" {
                        json!({"role": "user", "content": format!(
                            "Tool result for {} (is_error={}):\n{}",
                            message.tool_name.as_deref().unwrap_or("tool"),
                            message.is_error,
                            message.content
                        )})
                    } else {
                        json!({"role": message.role, "content": message.content})
                    }
                })
                .collect()
        } else {
            crate::llm::openai_messages(&request.messages)
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
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .map_err(|error| self.http_error("request", error))?
            .error_for_status()
            .map_err(|error| self.http_error("response status", error))?;
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|error| self.http_error("response decode", error))?;
        let content = value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .ok_or_else(|| {
                FagentError::Provider(format!(
                    "{} response did not include message content",
                    self.provider_name
                ))
            })?;

        parse_plan_response(content)
    }
    async fn call(&self, request: &crate::llm::AgentRequest) -> Result<crate::llm::LlmResponse> {
        let payload = json!({"model":request.model,"temperature":0,"messages":self.messages(request),"tools":request.tools.iter().map(|t|json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}})).collect::<Vec<_>>()});
        let value: serde_json::Value = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .map_err(|e| self.http_error("request", e))?
            .error_for_status()
            .map_err(|e| self.http_error("response status", e))?
            .json()
            .await
            .map_err(|e| self.http_error("response decode", e))?;
        crate::llm::parse_openai_response(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{AgentRequest, LlmResponse, Message, stage_tool_spec};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    // Exercise HTTP serialization and response parsing without credentials or a live API.
    fn mock_response(
        body: serde_json::Value,
    ) -> (String, std::thread::JoinHandle<serde_json::Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/chat/completions", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut received = Vec::new();
            let mut buf = [0; 4096];
            let (header_end, content_length) = loop {
                let count = stream.read(&mut buf).unwrap();
                assert!(count > 0);
                received.extend_from_slice(&buf[..count]);
                if let Some(pos) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&received[..pos]).to_lowercase();
                    assert!(headers.starts_with("post /chat/completions http/1.1"));
                    assert!(headers.contains("authorization: bearer test-key"));
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse::<usize>()
                        .unwrap();
                    break (pos + 4, length);
                }
            };
            while received.len() < header_end + content_length {
                let count = stream.read(&mut buf).unwrap();
                assert!(count > 0);
                received.extend_from_slice(&buf[..count]);
            }
            let payload =
                serde_json::from_slice(&received[header_end..header_end + content_length]).unwrap();
            let body = body.to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            payload
        });
        (endpoint, handle)
    }

    #[tokio::test]
    async fn groq_planning_sends_json_mode_and_parses_plan() {
        let content = json!({"workspace_root":null,"warnings":[],"actions":[]}).to_string();
        let (endpoint, server) =
            mock_response(json!({"choices":[{"message":{"content":content}}]}));
        let mut provider = OpenAiProvider::new_groq("test-key".into());
        assert_eq!(
            provider.endpoint,
            "https://api.groq.com/openai/v1/chat/completions"
        );
        provider.endpoint = endpoint;
        let request = PlanRequest::new(
            "inspect".into(),
            "openai/gpt-oss-120b".into(),
            ".".into(),
            1,
            "{}".into(),
            false,
            false,
        );
        let plan = provider.plan(&request).await.unwrap();
        assert!(plan.actions.is_empty());
        let payload = server.join().unwrap();
        assert_eq!(payload["model"], "openai/gpt-oss-120b");
        assert_eq!(payload["response_format"]["type"], "json_object");
    }

    #[tokio::test]
    async fn groq_agent_sends_valid_observations_and_parses_tool_calls() {
        let (endpoint, server) = mock_response(json!({"choices":[{"message":{"tool_calls":[{
            "id":"call-1", "type":"function", "function":{"name":"execute_stage", "arguments":"{\"name\":\"stage\"}"}
        }]}}]}));
        let mut provider = OpenAiProvider::new_groq("test-key".into());
        provider.endpoint = endpoint;
        let request = AgentRequest {
            model: "openai/gpt-oss-120b".into(),
            messages: vec![
                Message::system("inspect first"),
                Message::user("organize"),
                Message::assistant("Calling list_directory"),
                Message::tool("list_directory", "file.txt", false),
            ],
            tools: vec![stage_tool_spec()],
        };
        let LlmResponse::ToolCalls(calls) = provider.call(&request).await.unwrap() else {
            panic!("expected tool calls");
        };
        assert_eq!(calls[0].id, "call-1");
        assert_eq!(calls[0].name, "execute_stage");
        assert_eq!(calls[0].params["name"], "stage");
        let payload = server.join().unwrap();
        assert_eq!(payload["model"], "openai/gpt-oss-120b");
        assert_eq!(payload["tools"][0]["function"]["name"], "execute_stage");
        assert_eq!(payload["messages"][3]["role"], "user");
        assert!(
            payload["messages"][3]["content"]
                .as_str()
                .unwrap()
                .contains("file.txt")
        );
        assert!(
            payload["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|message| message.get("name").is_none())
        );
    }
}
