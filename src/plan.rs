use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::security::WorkspacePolicy;
use crate::{FagentError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    CreateDir,
    MoveFile,
    RenamePath,
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
    MoveFile,
    RenamePath,
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
    pub display_source: Option<String>,
    pub display_destination: Option<String>,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ValidatedPlan {
    pub workspace_root: PathBuf,
    pub warnings: Vec<String>,
    pub actions: Vec<ValidatedAction>,
}

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

    let mut available = HashSet::new();
    let mut removed = HashSet::new();
    let mut validated_actions = Vec::new();

    for action in plan.actions {
        let validated = match action.kind {
            ActionKind::CreateDir => validate_create_dir(action, policy, &mut available)?,
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
            ActionKind::DeletePath => {
                validate_delete(action, policy, &mut available, &mut removed)?
            }
        };
        validated_actions.push(validated);
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
    available: &mut HashSet<String>,
) -> Result<ValidatedAction> {
    let destination_raw = require_destination(&action)?;
    let destination = policy.resolve_path(destination_raw)?;
    if destination.exists() && !destination.is_dir() {
        return Err(FagentError::Validation(format!(
            "create_dir target already exists and is not a directory: {destination_raw}"
        )));
    }
    available.insert(policy.path_key(&destination));

    Ok(ValidatedAction {
        id: action.id,
        kind: action.kind,
        effective_kind: EffectiveActionKind::CreateDir,
        source: None,
        destination: Some(destination.clone()),
        display_source: None,
        display_destination: Some(policy.display_path(&destination)),
        rationale: action.rationale,
    })
}

fn validate_move_like(
    action: PlannedAction,
    policy: &WorkspacePolicy,
    available: &mut HashSet<String>,
    removed: &mut HashSet<String>,
    effective_kind: EffectiveActionKind,
    must_be_file: bool,
) -> Result<ValidatedAction> {
    let source_raw = require_source(&action)?;
    let destination_raw = require_destination(&action)?;
    let source = policy.resolve_path(source_raw)?;
    let destination = policy.resolve_path(destination_raw)?;
    ensure_source_available(policy, &source, available, removed, source_raw)?;

    if must_be_file && source.exists() && source.is_dir() {
        return Err(FagentError::Validation(format!(
            "move_file requires a file source, but `{source_raw}` is a directory"
        )));
    }

    let destination_key = policy.path_key(&destination);
    if destination.exists() || available.contains(&destination_key) {
        return Err(FagentError::Validation(format!(
            "destination already exists or is claimed by an earlier action: {destination_raw}"
        )));
    }

    removed.insert(policy.path_key(&source));
    available.remove(&policy.path_key(&source));
    available.insert(destination_key);

    Ok(ValidatedAction {
        id: action.id,
        kind: action.kind,
        effective_kind,
        source: Some(source.clone()),
        destination: Some(destination.clone()),
        display_source: Some(policy.display_path(&source)),
        display_destination: Some(policy.display_path(&destination)),
        rationale: action.rationale,
    })
}

fn validate_delete(
    action: PlannedAction,
    policy: &WorkspacePolicy,
    available: &mut HashSet<String>,
    removed: &mut HashSet<String>,
) -> Result<ValidatedAction> {
    let source_raw = require_source(&action)?;
    let source = policy.resolve_path(source_raw)?;
    ensure_source_available(policy, &source, available, removed, source_raw)?;

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
        display_source: Some(policy.display_path(&source)),
        display_destination: None,
        rationale: action.rationale,
    })
}

fn ensure_source_available(
    policy: &WorkspacePolicy,
    source: &PathBuf,
    available: &HashSet<String>,
    removed: &HashSet<String>,
    raw: &str,
) -> Result<()> {
    let key = policy.path_key(source);
    if removed.contains(&key) && !available.contains(&key) {
        return Err(FagentError::Validation(format!(
            "action order is invalid because `{raw}` was already moved or deleted earlier in the plan"
        )));
    }

    if available.contains(&key) || source.exists() {
        return Ok(());
    }

    Err(FagentError::Validation(format!(
        "source path does not exist: {raw}"
    )))
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
                    rationale: None,
                },
                PlannedAction {
                    id: "2".into(),
                    kind: ActionKind::MoveFile,
                    source: Some("b.txt".into()),
                    destination: Some("out.txt".into()),
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
                    rationale: None,
                },
                PlannedAction {
                    id: "2".into(),
                    kind: ActionKind::RenamePath,
                    source: Some("a.txt".into()),
                    destination: Some("b.txt".into()),
                    rationale: None,
                },
            ],
        };

        let result = validate_plan(plan, &policy);
        assert!(result.is_err());
    }
}
