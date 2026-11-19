use super::{Tool, ToolResult};
use crate::context::list_directory;
use crate::executor::Executor;
use crate::plan::{ActionKind, ExecutionPlan, PlannedAction, validate_plan};
use crate::security::WorkspacePolicy;
use async_trait::async_trait;

pub struct ListDirectoryTool;
#[async_trait]
impl Tool for ListDirectoryTool {
    fn name(&self) -> &'static str {
        "list_directory"
    }
    fn description(&self) -> &'static str {
        "List a workspace directory at a requested depth."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"depth":{"type":"integer","minimum":0}}})
    }
    async fn call(&self, p: serde_json::Value, policy: &WorkspacePolicy) -> ToolResult {
        let raw = p.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let depth = p.get("depth").and_then(|v| v.as_u64()).unwrap_or(1).min(20) as usize;
        let path = match policy.resolve_path(raw) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(e.to_string()),
        };
        match list_directory(&path, depth) {
            Ok(c) => ToolResult::ok(c.to_compact_json().unwrap_or_else(|e| e.to_string())),
            Err(e) => ToolResult::error(e.to_string()),
        }
    }
}

#[derive(Clone, Copy)]
pub enum FilesystemToolKind {
    CreateDir,
    CreateFile,
    MoveFile,
    RenamePath,
    ZipPath,
    UnzipArchive,
    DeletePath,
}
impl FilesystemToolKind {
    pub fn all() -> [Self; 7] {
        [
            Self::CreateDir,
            Self::CreateFile,
            Self::MoveFile,
            Self::RenamePath,
            Self::ZipPath,
            Self::UnzipArchive,
            Self::DeletePath,
        ]
    }
    fn name(self) -> &'static str {
        match self {
            Self::CreateDir => "create_dir",
            Self::CreateFile => "create_file",
            Self::MoveFile => "move_file",
            Self::RenamePath => "rename_path",
            Self::ZipPath => "zip_path",
            Self::UnzipArchive => "unzip_archive",
            Self::DeletePath => "delete_path",
        }
    }
    fn kind(self) -> ActionKind {
        match self {
            Self::CreateDir => ActionKind::CreateDir,
            Self::CreateFile => ActionKind::CreateFile,
            Self::MoveFile => ActionKind::MoveFile,
            Self::RenamePath => ActionKind::RenamePath,
            Self::ZipPath => ActionKind::ZipPath,
            Self::UnzipArchive => ActionKind::UnzipArchive,
            Self::DeletePath => ActionKind::DeletePath,
        }
    }
}
pub struct FilesystemTool {
    kind: FilesystemToolKind,
}
impl FilesystemTool {
    pub fn new(kind: FilesystemToolKind) -> Self {
        Self { kind }
    }
}
#[async_trait]
impl Tool for FilesystemTool {
    fn name(&self) -> &'static str {
        self.kind.name()
    }
    fn description(&self) -> &'static str {
        "Perform a validated filesystem operation."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"source":{"type":"string"},"destination":{"type":"string"},"content":{"type":"string"},"rationale":{"type":"string"}}})
    }
    async fn call(&self, p: serde_json::Value, policy: &WorkspacePolicy) -> ToolResult {
        let plan = ExecutionPlan {
            workspace_root: None,
            warnings: vec![],
            actions: vec![PlannedAction {
                id: "agent-tool".into(),
                kind: self.kind.kind(),
                source: p.get("source").and_then(|v| v.as_str()).map(str::to_owned),
                destination: p
                    .get("destination")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                content: p.get("content").and_then(|v| v.as_str()).map(str::to_owned),
                rationale: p
                    .get("rationale")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
            }],
        };
        let validated = match validate_plan(plan, policy) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(e.to_string()),
        };
        match Executor::new(policy.clone())
            .execute_action(&validated.actions[0])
            .await
        {
            Ok(record) => ToolResult {
                output: format!("{} completed", self.name()),
                is_error: false,
                side_effects: vec![record],
                task_complete: false,
            },
            Err(e) => ToolResult::error(e.to_string()),
        }
    }
}
