use super::{Tool, ToolResult};
use crate::security::WorkspacePolicy;
use async_trait::async_trait;
use tokio::io::AsyncReadExt;
pub struct ReadFileTool;
#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn description(&self) -> &'static str {
        "Read a UTF-8 file within a byte budget."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","required":["path"],"properties":{"path":{"type":"string"},"max_bytes":{"type":"integer","minimum":1}}})
    }
    async fn call(&self, p: serde_json::Value, policy: &WorkspacePolicy) -> ToolResult {
        let Some(path) = p.get("path").and_then(|v| v.as_str()) else {
            return ToolResult::error("path is required");
        };
        let max = p
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(32_000)
            .min(1_000_000) as usize;
        let path = match policy.resolve_path(path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(e.to_string()),
        };
        let mut f = match tokio::fs::File::open(&path).await {
            Ok(f) => f,
            Err(e) => return ToolResult::error(e.to_string()),
        };
        let mut buf = vec![0; max];
        match f.read(&mut buf).await {
            Ok(n) => ToolResult::ok(String::from_utf8_lossy(&buf[..n]).to_string()),
            Err(e) => ToolResult::error(e.to_string()),
        }
    }
}
