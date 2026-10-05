use super::{Tool, ToolResult};
use crate::security::WorkspacePolicy;
use async_trait::async_trait;
pub struct TaskCompleteTool;
#[async_trait]
impl Tool for TaskCompleteTool {
    fn name(&self) -> &'static str {
        "task_complete"
    }
    fn description(&self) -> &'static str {
        "Signal that the task is complete."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"summary":{"type":"string"}}})
    }
    async fn call(&self, p: serde_json::Value, _: &WorkspacePolicy) -> ToolResult {
        ToolResult {
            output: p
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("Task complete.")
                .into(),
            is_error: false,
            side_effects: vec![],
            task_complete: true,
        }
    }
}
