use crate::executor::ExecutedActionRecord;
use crate::security::WorkspacePolicy;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

pub mod fs_tools;
pub mod read_file;
pub mod search;
pub mod task_complete;
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub is_error: bool,
    pub side_effects: Vec<ExecutedActionRecord>,
    pub task_complete: bool,
}
impl ToolResult {
    pub fn ok(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            side_effects: vec![],
            task_complete: false,
        }
    }
    pub fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
            side_effects: vec![],
            task_complete: false,
        }
    }
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters_schema(&self) -> serde_json::Value;
    async fn call(&self, params: serde_json::Value, policy: &WorkspacePolicy) -> ToolResult;
}
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}
impl ToolRegistry {
    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        self.tools.insert(tool.name().into(), Arc::new(tool));
    }
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .values()
            .map(|t| ToolSpec {
                name: t.name().into(),
                description: t.description().into(),
                parameters: t.parameters_schema(),
            })
            .collect()
    }
    pub fn specs_named(&self, names: &[&str]) -> Vec<ToolSpec> {
        self.tools
            .values()
            .filter(|tool| names.contains(&tool.name()))
            .map(|tool| ToolSpec {
                name: tool.name().into(),
                description: tool.description().into(),
                parameters: tool.parameters_schema(),
            })
            .collect()
    }
    pub async fn call(
        &self,
        name: &str,
        params: serde_json::Value,
        policy: &WorkspacePolicy,
    ) -> ToolResult {
        match self.tools.get(name) {
            Some(t) => t.call(params, policy).await,
            None => ToolResult::error(format!("unknown tool `{name}`")),
        }
    }
    pub fn standard() -> Self {
        let mut r = Self::default();
        r.register(fs_tools::ListDirectoryTool);
        r.register(read_file::ReadFileTool);
        r.register(search::SearchPathsTool);
        for k in fs_tools::FilesystemToolKind::all() {
            r.register(fs_tools::FilesystemTool::new(k));
        }
        r.register(task_complete::TaskCompleteTool);
        r
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn unknown_tool_is_error() {
        let r = ToolRegistry::default();
        let p =
            crate::security::WorkspacePolicy::new(std::env::current_dir().unwrap(), false, false)
                .unwrap();
        assert!(r.call("nope", serde_json::json!({}), &p).await.is_error);
    }
}
