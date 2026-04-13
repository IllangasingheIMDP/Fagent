use std::io;
use std::path::{Path, PathBuf};

use tokio::fs;
use tracing::info;

use crate::plan::{EffectiveActionKind, ValidatedPlan};
use crate::security::WorkspacePolicy;
use crate::{FagentError, Result};

#[derive(Debug, Clone)]
pub struct ExecutionFailure {
    pub action_id: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ExecutionReport {
    pub completed: Vec<String>,
    pub failed: Option<ExecutionFailure>,
    pub pending: Vec<String>,
}

impl ExecutionReport {
    pub fn succeeded(&self) -> bool {
        self.failed.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct Executor {
    policy: WorkspacePolicy,
}

impl Executor {
    pub fn new(policy: WorkspacePolicy) -> Self {
        Self { policy }
    }

    pub async fn run(&self, plan: &ValidatedPlan) -> ExecutionReport {
        let mut completed = Vec::new();

        for (index, action) in plan.actions.iter().enumerate() {
            info!("executing action {}", action.id);
            let result = match action.effective_kind {
                EffectiveActionKind::CreateDir => {
                    self.create_dir(action.destination.as_ref().expect("validated"))
                        .await
                }
                EffectiveActionKind::CreateFile => {
                    self.create_file(
                        action.destination.as_ref().expect("validated"),
                        action.content.as_deref().expect("validated"),
                    )
                    .await
                }
                EffectiveActionKind::MoveFile | EffectiveActionKind::RenamePath => {
                    self.move_path(
                        action.source.as_ref().expect("validated"),
                        action.destination.as_ref().expect("validated"),
                    )
                    .await
                }
                EffectiveActionKind::DeleteToTrash => {
                    self.delete_to_trash(action.source.as_ref().expect("validated"))
                        .await
                }
                EffectiveActionKind::DeletePermanent => {
                    self.delete_permanent(action.source.as_ref().expect("validated"))
                        .await
                }
            };

            match result {
                Ok(()) => completed.push(action.id.clone()),
                Err(error) => {
                    return ExecutionReport {
                        completed,
                        failed: Some(ExecutionFailure {
                            action_id: action.id.clone(),
                            message: error.to_string(),
                        }),
                        pending: plan.actions[index + 1..]
                            .iter()
                            .map(|pending| pending.id.clone())
                            .collect(),
                    };
                }
            }
        }

        ExecutionReport {
            completed,
            failed: None,
            pending: Vec::new(),
        }
    }

    async fn create_dir(&self, destination: &Path) -> Result<()> {
        fs::create_dir_all(destination).await?;
        Ok(())
    }

    async fn create_file(&self, destination: &Path, content: &str) -> Result<()> {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(destination, content).await?;
        Ok(())
    }

    async fn move_path(&self, source: &Path, destination: &Path) -> Result<()> {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }

        match fs::rename(source, destination).await {
            Ok(()) => Ok(()),
            Err(error) if is_cross_device_error(&error) => {
                self.copy_then_remove(source.to_path_buf(), destination.to_path_buf())
                    .await
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn delete_to_trash(&self, source: &Path) -> Result<()> {
        let source = source.to_path_buf();
        tokio::task::spawn_blocking(move || {
            trash::delete(&source)
                .map_err(|error| FagentError::Execution(format!("trash failed: {error}")))
        })
        .await??;
        Ok(())
    }

    async fn delete_permanent(&self, source: &Path) -> Result<()> {
        if source.is_dir() {
            fs::remove_dir_all(source).await?;
        } else {
            fs::remove_file(source).await?;
        }
        Ok(())
    }

    async fn copy_then_remove(&self, source: PathBuf, destination: PathBuf) -> Result<()> {
        let source_for_copy = source.clone();
        let destination_for_copy = destination.clone();
        tokio::task::spawn_blocking(move || {
            copy_path_sync(&source_for_copy, &destination_for_copy)
        })
        .await??;
        if source.is_dir() {
            fs::remove_dir_all(&source).await?;
        } else {
            fs::remove_file(&source).await?;
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn policy(&self) -> &WorkspacePolicy {
        &self.policy
    }
}

fn is_cross_device_error(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(18) | Some(17))
}

fn copy_path_sync(source: &Path, destination: &Path) -> Result<()> {
    let metadata = std::fs::metadata(source)?;
    if metadata.is_dir() {
        std::fs::create_dir_all(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            let child_source = entry.path();
            let child_destination = destination.join(entry.file_name());
            copy_path_sync(&child_source, &child_destination)?;
        }
        return Ok(());
    }

    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, destination)?;
    preserve_unix_permissions(source, destination)?;
    Ok(())
}

fn preserve_unix_permissions(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(source)?.permissions().mode();
        let permissions = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(destination, permissions)?;
    }

    #[cfg(not(unix))]
    {
        let _ = (source, destination);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::Executor;
    use crate::plan::{ActionKind, EffectiveActionKind, ValidatedAction, ValidatedPlan};
    use crate::security::WorkspacePolicy;

    #[tokio::test]
    async fn move_creates_missing_parent_directories() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("note.txt"), "hello").unwrap();
        let policy = WorkspacePolicy::new(workspace.clone(), false, false).unwrap();
        let executor = Executor::new(policy);
        let source = workspace.join("note.txt");
        let destination = workspace.join("archive").join("note.txt");
        let plan = ValidatedPlan {
            workspace_root: workspace.clone(),
            warnings: vec![],
            actions: vec![ValidatedAction {
                id: "1".into(),
                kind: ActionKind::MoveFile,
                effective_kind: EffectiveActionKind::MoveFile,
                source: Some(source),
                destination: Some(destination.clone()),
                content: None,
                display_source: Some("note.txt".into()),
                display_destination: Some("archive/note.txt".into()),
                rationale: None,
            }],
        };

        let report = executor.run(&plan).await;
        assert!(report.succeeded());
        assert!(destination.exists());
    }

    #[tokio::test]
    async fn fail_fast_reports_remaining_actions() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let policy = WorkspacePolicy::new(workspace.clone(), false, true).unwrap();
        let executor = Executor::new(policy);
        let plan = ValidatedPlan {
            workspace_root: workspace.clone(),
            warnings: vec![],
            actions: vec![
                ValidatedAction {
                    id: "1".into(),
                    kind: ActionKind::DeletePath,
                    effective_kind: EffectiveActionKind::DeletePermanent,
                    source: Some(workspace.join("missing.txt")),
                    destination: None,
                    content: None,
                    display_source: Some("missing.txt".into()),
                    display_destination: None,
                    rationale: None,
                },
                ValidatedAction {
                    id: "2".into(),
                    kind: ActionKind::CreateDir,
                    effective_kind: EffectiveActionKind::CreateDir,
                    source: None,
                    destination: Some(workspace.join("later")),
                    content: None,
                    display_source: None,
                    display_destination: Some("later".into()),
                    rationale: None,
                },
            ],
        };

        let report = executor.run(&plan).await;
        assert!(!report.succeeded());
        assert_eq!(report.pending, vec!["2"]);
        assert!(!workspace.join("later").exists());
    }

    #[tokio::test]
    async fn create_file_writes_content() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let policy = WorkspacePolicy::new(workspace.clone(), false, false).unwrap();
        let executor = Executor::new(policy);
        let destination = workspace.join("scripts").join("hello.bat");
        let content = "@echo off\r\necho hello\r\n";
        let plan = ValidatedPlan {
            workspace_root: workspace.clone(),
            warnings: vec![],
            actions: vec![ValidatedAction {
                id: "1".into(),
                kind: ActionKind::CreateFile,
                effective_kind: EffectiveActionKind::CreateFile,
                source: None,
                destination: Some(destination.clone()),
                content: Some(content.into()),
                display_source: None,
                display_destination: Some("scripts/hello.bat".into()),
                rationale: None,
            }],
        };

        let report = executor.run(&plan).await;
        assert!(report.succeeded());
        assert_eq!(fs::read_to_string(destination).unwrap(), content);
    }
}
