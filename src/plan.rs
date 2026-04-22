use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::security::WorkspacePolicy;
use crate::{FagentError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    CreateDir,
    CreateFile,
    MoveFile,
    RenamePath,
    ZipPath,
    UnzipArchive,
    DeletePath,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedAction {
    pub id: String,
    pub kind: ActionKind,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub destination: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlan {
    #[serde(default)]
    pub workspace_root: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub actions: Vec<PlannedAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectiveActionKind {
    CreateDir,
    CreateFile,
    MoveFile,
    RenamePath,
    ZipPath,
    UnzipArchive,
    DeleteToTrash,
    DeletePermanent,
}

#[derive(Debug, Clone)]
pub struct ValidatedAction {
    pub id: String,
    pub kind: ActionKind,
    pub effective_kind: EffectiveActionKind,
    pub source: Option<PathBuf>,
    pub destination: Option<PathBuf>,
    pub content: Option<String>,
    pub display_source: Option<String>,
    pub display_destination: Option<String>,
    pub rationale: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ValidatedPlan {
    pub workspace_root: PathBuf,
    pub warnings: Vec<String>,
    pub actions: Vec<ValidatedAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathKind {
    File,
    Dir,
}

type AvailablePaths = HashMap<String, PathKind>;

pub fn validate_plan(plan: ExecutionPlan, policy: &WorkspacePolicy) -> Result<ValidatedPlan> {
    if plan.actions.is_empty() {
        return Err(FagentError::Validation(
            "the model returned an empty action list".into(),
        ));
    }

    let mut warnings = plan.warnings;
    if plan
        .actions
        .iter()
        .any(|action| action.kind == ActionKind::DeletePath)
        && !policy.permanent_delete
    {
        warnings.push("Delete actions will be routed to the OS trash or recycle bin.".into());
    }

    let mut available = AvailablePaths::new();
    let mut removed = HashSet::new();
    let mut validated_actions = Vec::new();

    for action in plan.actions {
        let validated = match action.kind {
            ActionKind::CreateDir => validate_create_dir(action, policy, &mut available)?,
            ActionKind::CreateFile => validate_create_file(action, policy, &mut available)?,
            ActionKind::MoveFile => validate_move_like(
                action,
                policy,
                &mut available,
                &mut removed,
                EffectiveActionKind::MoveFile,
                true,
            )?,
            ActionKind::RenamePath => validate_move_like(
                action,
                policy,
                &mut available,
                &mut removed,
                EffectiveActionKind::RenamePath,
                false,
            )?,
            ActionKind::ZipPath => {
                validate_zip_path(action, policy, &mut available, &mut removed)?
            }
            ActionKind::UnzipArchive => {
                validate_unzip_archive(action, policy, &mut available, &mut removed)?
            }
            ActionKind::DeletePath => {
                validate_delete(action, policy, &mut available, &mut removed)?
            }
        };
        validated_actions.push(validated);
    }

    if validated_actions
        .iter()
        .any(|action| !action.warnings.is_empty())
    {
        warnings.push("High-risk delete actions require an extra confirmation step.".into());
    }

    Ok(ValidatedPlan {
        workspace_root: policy.root().to_path_buf(),
        warnings,
        actions: validated_actions,
    })
}

fn validate_create_dir(
    action: PlannedAction,
    policy: &WorkspacePolicy,
    available: &mut AvailablePaths,
) -> Result<ValidatedAction> {
    let destination_raw = require_destination(&action)?;
    let destination = policy.resolve_path(destination_raw)?;
    if destination.exists() && !destination.is_dir() {
        return Err(FagentError::Validation(format!(
            "create_dir target already exists and is not a directory: {destination_raw}"
        )));
    }
    available.insert(policy.path_key(&destination), PathKind::Dir);

    Ok(ValidatedAction {
        id: action.id,
        kind: action.kind,
        effective_kind: EffectiveActionKind::CreateDir,
        source: None,
        destination: Some(destination.clone()),
        content: None,
        display_source: None,
        display_destination: Some(policy.display_path(&destination)),
        rationale: action.rationale,
        warnings: Vec::new(),
    })
}

fn validate_create_file(
    action: PlannedAction,
    policy: &WorkspacePolicy,
    available: &mut AvailablePaths,
) -> Result<ValidatedAction> {
    let destination_raw = require_destination(&action)?;
    let content = require_content(&action)?.to_string();
    let destination = policy.resolve_path(destination_raw)?;
    let destination_key = policy.path_key(&destination);

    if destination.exists() || available.contains_key(&destination_key) {
        return Err(FagentError::Validation(format!(
            "create_file target already exists or is claimed by an earlier action: {destination_raw}"
        )));
    }

    available.insert(destination_key, PathKind::File);

    Ok(ValidatedAction {
        id: action.id,
        kind: action.kind,
        effective_kind: EffectiveActionKind::CreateFile,
        source: None,
        destination: Some(destination.clone()),
        content: Some(content),
        display_source: None,
        display_destination: Some(policy.display_path(&destination)),
        rationale: action.rationale,
        warnings: Vec::new(),
    })
}

fn validate_move_like(
    action: PlannedAction,
    policy: &WorkspacePolicy,
    available: &mut AvailablePaths,
    removed: &mut HashSet<String>,
    effective_kind: EffectiveActionKind,
    must_be_file: bool,
) -> Result<ValidatedAction> {
    let source_raw = require_source(&action)?;
    let destination_raw = require_destination(&action)?;
    let source = policy.resolve_path(source_raw)?;
    let destination = policy.resolve_path(destination_raw)?;
    ensure_source_available(policy, &source, available, removed, source_raw)?;
    let source_kind = source_kind(policy, &source, available);

    if must_be_file && source_kind == PathKind::Dir {
        return Err(FagentError::Validation(format!(
            "move_file requires a file source, but `{source_raw}` is a directory"
        )));
    }

    let destination_key = policy.path_key(&destination);
    if destination.exists() || available.contains_key(&destination_key) {
        return Err(FagentError::Validation(format!(
            "destination already exists or is claimed by an earlier action: {destination_raw}"
        )));
    }

    let source_key = policy.path_key(&source);
    let destination_kind = match effective_kind {
        EffectiveActionKind::MoveFile => PathKind::File,
        EffectiveActionKind::RenamePath => source_kind,
        _ => source_kind,
    };

    removed.insert(source_key.clone());
    available.remove(&source_key);
    available.insert(destination_key, destination_kind);

    Ok(ValidatedAction {
        id: action.id,
        kind: action.kind,
        effective_kind,
        source: Some(source.clone()),
        destination: Some(destination.clone()),
        content: None,
        display_source: Some(policy.display_path(&source)),
        display_destination: Some(policy.display_path(&destination)),
        rationale: action.rationale,
        warnings: Vec::new(),
    })
}

fn validate_delete(
    action: PlannedAction,
    policy: &WorkspacePolicy,
    available: &mut AvailablePaths,
    removed: &mut HashSet<String>,
) -> Result<ValidatedAction> {
    let source_raw = require_source(&action)?;
    let source = policy.resolve_path(source_raw)?;
    ensure_source_available(policy, &source, available, removed, source_raw)?;
    ensure_delete_target_allowed(policy, &source, source_raw)?;
    let source_kind = source_kind(policy, &source, available);
    let warnings = delete_warnings(&action.id, policy, &source, source_kind);

    let source_key = policy.path_key(&source);
    removed.insert(source_key.clone());
    available.remove(&source_key);

    Ok(ValidatedAction {
        id: action.id,
        kind: action.kind,
        effective_kind: if policy.permanent_delete {
            EffectiveActionKind::DeletePermanent
        } else {
            EffectiveActionKind::DeleteToTrash
        },
        source: Some(source.clone()),
        destination: None,
        content: None,
        display_source: Some(policy.display_path(&source)),
        display_destination: None,
        rationale: action.rationale,
        warnings,
    })
}

fn validate_zip_path(
    action: PlannedAction,
    policy: &WorkspacePolicy,
    available: &mut AvailablePaths,
    removed: &mut HashSet<String>,
) -> Result<ValidatedAction> {
    let source_raw = require_source(&action)?;
    let destination_raw = require_destination(&action)?;
    let source = policy.resolve_path(source_raw)?;
    let destination = policy.resolve_path(destination_raw)?;
    ensure_source_available(policy, &source, available, removed, source_raw)?;

    let destination_key = policy.path_key(&destination);
    if destination.exists() || available.contains_key(&destination_key) {
        return Err(FagentError::Validation(format!(
            "destination already exists or is claimed by an earlier action: {destination_raw}"
        )));
    }

    if source.is_dir() && destination.starts_with(&source) {
        return Err(FagentError::Validation(format!(
            "zip_path destination cannot be inside the source directory: {destination_raw}"
        )));
    }

    available.insert(destination_key, PathKind::File);

    Ok(ValidatedAction {
        id: action.id,
        kind: action.kind,
        effective_kind: EffectiveActionKind::ZipPath,
        source: Some(source.clone()),
        destination: Some(destination.clone()),
        content: None,
        display_source: Some(policy.display_path(&source)),
        display_destination: Some(policy.display_path(&destination)),
        rationale: action.rationale,
        warnings: Vec::new(),
    })
}

fn validate_unzip_archive(
    action: PlannedAction,
    policy: &WorkspacePolicy,
    available: &mut AvailablePaths,
    removed: &mut HashSet<String>,
) -> Result<ValidatedAction> {
    let source_raw = require_source(&action)?;
    let destination_raw = require_destination(&action)?;
    let source = policy.resolve_path(source_raw)?;
    let destination = policy.resolve_path(destination_raw)?;
    ensure_source_available(policy, &source, available, removed, source_raw)?;
    let source_kind = source_kind(policy, &source, available);

    if source_kind == PathKind::Dir {
        return Err(FagentError::Validation(format!(
            "unzip_archive requires a file source, but `{source_raw}` is a directory"
        )));
    }

    let destination_key = policy.path_key(&destination);
    if destination.exists() && !destination.is_dir() {
        return Err(FagentError::Validation(format!(
            "unzip_archive destination exists and is not a directory: {destination_raw}"
        )));
    }
    if matches!(available.get(&destination_key), Some(PathKind::File)) {
        return Err(FagentError::Validation(format!(
            "unzip_archive destination is already claimed as a file by an earlier action: {destination_raw}"
        )));
    }

    available.insert(destination_key, PathKind::Dir);

    Ok(ValidatedAction {
        id: action.id,
        kind: action.kind,
        effective_kind: EffectiveActionKind::UnzipArchive,
        source: Some(source.clone()),
        destination: Some(destination.clone()),
        content: None,
        display_source: Some(policy.display_path(&source)),
        display_destination: Some(policy.display_path(&destination)),
        rationale: action.rationale,
        warnings: Vec::new(),
    })
}

fn ensure_source_available(
    policy: &WorkspacePolicy,
    source: &PathBuf,
    available: &AvailablePaths,
    removed: &HashSet<String>,
    raw: &str,
) -> Result<()> {
    let key = policy.path_key(source);
    if removed.contains(&key) && !available.contains_key(&key) {
        return Err(FagentError::Validation(format!(
            "action order is invalid because `{raw}` was already moved or deleted earlier in the plan"
        )));
    }

    if available.contains_key(&key) || source.exists() {
        return Ok(());
    }

    Err(FagentError::Validation(format!(
        "source path does not exist: {raw}"
    )))
}

fn source_kind(policy: &WorkspacePolicy, source: &Path, available: &AvailablePaths) -> PathKind {
    available
        .get(&policy.path_key(source))
        .copied()
        .unwrap_or_else(|| {
            if source.is_dir() {
                PathKind::Dir
            } else {
                PathKind::File
            }
        })
}

fn ensure_delete_target_allowed(policy: &WorkspacePolicy, source: &Path, raw: &str) -> Result<()> {
    if source.parent().is_none() {
        return Err(FagentError::Validation(format!(
            "delete_path cannot target a filesystem root: {raw}"
        )));
    }

    if policy.is_workspace_root(source) {
        return Err(FagentError::Validation(format!(
            "delete_path cannot target the workspace root: {raw}"
        )));
    }

    if let Some(component) = protected_delete_component(source) {
        return Err(FagentError::Validation(format!(
            "delete_path cannot target repository metadata `{component}`: {raw}"
        )));
    }

    Ok(())
}

fn protected_delete_component(path: &Path) -> Option<String> {
    for component in path.components() {
        if let Component::Normal(value) = component {
            let component = value.to_string_lossy();
            if is_protected_delete_component(&component) {
                return Some(component.into_owned());
            }
        }
    }

    None
}

fn is_protected_delete_component(component: &str) -> bool {
    const PROTECTED_COMPONENTS: &[&str] = &[".git", ".hg", ".svn", ".jj"];

    #[cfg(windows)]
    {
        PROTECTED_COMPONENTS
            .iter()
            .any(|candidate| component.eq_ignore_ascii_case(candidate))
    }

    #[cfg(not(windows))]
    {
        PROTECTED_COMPONENTS.contains(&component)
    }
}

fn delete_warnings(
    action_id: &str,
    policy: &WorkspacePolicy,
    source: &Path,
    source_kind: PathKind,
) -> Vec<String> {
    let display = policy.display_path(source);
    let mut warnings = Vec::new();

    if source_kind == PathKind::Dir {
        warnings.push(format!(
            "action `{action_id}` deletes a directory recursively: {display}"
        ));
    }

    if policy.permanent_delete {
        warnings.push(format!(
            "action `{action_id}` will permanently delete `{display}`"
        ));
    }

    if !policy.is_within_workspace(source) {
        warnings.push(format!(
            "action `{action_id}` targets a path outside the workspace: {display}"
        ));
    }

    warnings
}

fn require_source(action: &PlannedAction) -> Result<&str> {
    action.source.as_deref().ok_or_else(|| {
        FagentError::Validation(format!("action `{}` is missing a source", action.id))
    })
}

fn require_destination(action: &PlannedAction) -> Result<&str> {
    action.destination.as_deref().ok_or_else(|| {
        FagentError::Validation(format!("action `{}` is missing a destination", action.id))
    })
}

fn require_content(action: &PlannedAction) -> Result<&str> {
    action.content.as_deref().ok_or_else(|| {
        FagentError::Validation(format!("action `{}` is missing content", action.id))
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{ActionKind, ExecutionPlan, PlannedAction, validate_plan};
    use crate::security::WorkspacePolicy;

    #[test]
    fn conflicting_destinations_are_rejected() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("a.txt"), "a").unwrap();
        fs::write(workspace.join("b.txt"), "b").unwrap();
        let policy = WorkspacePolicy::new(workspace, false, false).unwrap();

        let plan = ExecutionPlan {
            workspace_root: None,
            warnings: vec![],
            actions: vec![
                PlannedAction {
                    id: "1".into(),
                    kind: ActionKind::MoveFile,
                    source: Some("a.txt".into()),
                    destination: Some("out.txt".into()),
                    content: None,
                    rationale: None,
                },
                PlannedAction {
                    id: "2".into(),
                    kind: ActionKind::MoveFile,
                    source: Some("b.txt".into()),
                    destination: Some("out.txt".into()),
                    content: None,
                    rationale: None,
                },
            ],
        };

        let result = validate_plan(plan, &policy);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_action_order_is_rejected() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("a.txt"), "a").unwrap();
        let policy = WorkspacePolicy::new(workspace, false, false).unwrap();

        let plan = ExecutionPlan {
            workspace_root: None,
            warnings: vec![],
            actions: vec![
                PlannedAction {
                    id: "1".into(),
                    kind: ActionKind::DeletePath,
                    source: Some("a.txt".into()),
                    destination: None,
                    content: None,
                    rationale: None,
                },
                PlannedAction {
                    id: "2".into(),
                    kind: ActionKind::RenamePath,
                    source: Some("a.txt".into()),
                    destination: Some("b.txt".into()),
                    content: None,
                    rationale: None,
                },
            ],
        };

        let result = validate_plan(plan, &policy);
        assert!(result.is_err());
    }

    #[test]
    fn create_file_requires_content() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let policy = WorkspacePolicy::new(workspace, false, false).unwrap();

        let plan = ExecutionPlan {
            workspace_root: None,
            warnings: vec![],
            actions: vec![PlannedAction {
                id: "1".into(),
                kind: ActionKind::CreateFile,
                source: None,
                destination: Some("notes.txt".into()),
                content: None,
                rationale: None,
            }],
        };

        let result = validate_plan(plan, &policy);
        assert!(result.is_err());
    }

    #[test]
    fn create_file_can_be_moved_later_in_plan() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let policy = WorkspacePolicy::new(workspace, false, false).unwrap();

        let plan = ExecutionPlan {
            workspace_root: None,
            warnings: vec![],
            actions: vec![
                PlannedAction {
                    id: "1".into(),
                    kind: ActionKind::CreateFile,
                    source: None,
                    destination: Some("script.bat".into()),
                    content: Some("@echo off\r\necho hi\r\n".into()),
                    rationale: None,
                },
                PlannedAction {
                    id: "2".into(),
                    kind: ActionKind::RenamePath,
                    source: Some("script.bat".into()),
                    destination: Some("archive/script.bat".into()),
                    content: None,
                    rationale: None,
                },
            ],
        };

        let result = validate_plan(plan, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn delete_rejects_workspace_root() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let policy = WorkspacePolicy::new(workspace, false, false).unwrap();

        let plan = ExecutionPlan {
            workspace_root: None,
            warnings: vec![],
            actions: vec![PlannedAction {
                id: "1".into(),
                kind: ActionKind::DeletePath,
                source: Some(".".into()),
                destination: None,
                content: None,
                rationale: None,
            }],
        };

        let error = validate_plan(plan, &policy).unwrap_err();
        assert!(error.to_string().contains("workspace root"));
    }

    #[test]
    fn delete_rejects_repository_metadata() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(workspace.join(".git")).unwrap();
        let policy = WorkspacePolicy::new(workspace, false, false).unwrap();

        let plan = ExecutionPlan {
            workspace_root: None,
            warnings: vec![],
            actions: vec![PlannedAction {
                id: "1".into(),
                kind: ActionKind::DeletePath,
                source: Some(".git".into()),
                destination: None,
                content: None,
                rationale: None,
            }],
        };

        let error = validate_plan(plan, &policy).unwrap_err();
        assert!(error.to_string().contains("repository metadata"));
    }

    #[test]
    fn move_file_rejects_directory_created_earlier_in_plan() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let policy = WorkspacePolicy::new(workspace, false, false).unwrap();

        let plan = ExecutionPlan {
            workspace_root: None,
            warnings: vec![],
            actions: vec![
                PlannedAction {
                    id: "1".into(),
                    kind: ActionKind::CreateDir,
                    source: None,
                    destination: Some("drafts".into()),
                    content: None,
                    rationale: None,
                },
                PlannedAction {
                    id: "2".into(),
                    kind: ActionKind::MoveFile,
                    source: Some("drafts".into()),
                    destination: Some("drafts.txt".into()),
                    content: None,
                    rationale: None,
                },
            ],
        };

        let error = validate_plan(plan, &policy).unwrap_err();
        assert!(error.to_string().contains("is a directory"));
    }

    #[test]
    fn risky_delete_warnings_are_recorded() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(outside.join("logs")).unwrap();
        fs::write(outside.join("logs").join("app.log"), "log").unwrap();
        let policy = WorkspacePolicy::new(workspace, true, true).unwrap();

        let plan = ExecutionPlan {
            workspace_root: None,
            warnings: vec![],
            actions: vec![PlannedAction {
                id: "1".into(),
                kind: ActionKind::DeletePath,
                source: Some(outside.join("logs").to_string_lossy().into_owned()),
                destination: None,
                content: None,
                rationale: None,
            }],
        };

        let validated = validate_plan(plan, &policy).unwrap();
        let warnings = &validated.actions[0].warnings;

        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("directory recursively"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("permanently delete"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("outside the workspace"))
        );
        assert!(
            validated
                .warnings
                .iter()
                .any(|warning| warning.contains("extra confirmation"))
        );
    }

    #[test]
    fn unzip_rejects_directory_source() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(workspace.join("archive_dir")).unwrap();
        let policy = WorkspacePolicy::new(workspace, false, false).unwrap();

        let plan = ExecutionPlan {
            workspace_root: None,
            warnings: vec![],
            actions: vec![PlannedAction {
                id: "1".into(),
                kind: ActionKind::UnzipArchive,
                source: Some("archive_dir".into()),
                destination: Some("out".into()),
                content: None,
                rationale: None,
            }],
        };

        let error = validate_plan(plan, &policy).unwrap_err();
        assert!(error.to_string().contains("requires a file source"));
    }

    #[test]
    fn zip_rejects_destination_inside_source_directory() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(workspace.join("docs")).unwrap();
        fs::write(workspace.join("docs").join("readme.txt"), "hello").unwrap();
        let policy = WorkspacePolicy::new(workspace, false, false).unwrap();

        let plan = ExecutionPlan {
            workspace_root: None,
            warnings: vec![],
            actions: vec![PlannedAction {
                id: "1".into(),
                kind: ActionKind::ZipPath,
                source: Some("docs".into()),
                destination: Some("docs/archive.zip".into()),
                content: None,
                rationale: None,
            }],
        };

        let error = validate_plan(plan, &policy).unwrap_err();
        assert!(error.to_string().contains("cannot be inside the source directory"));
    }
}
