use std::path::Path;

use comfy_table::{Color, Table, presets::UTF8_FULL};
use inquire::{Select, Text};

use crate::executor::{
    ExecutionFailureContext, ExecutionRecoveryHandler, RecoveryDecision,
    RollbackActionReport, RollbackFailureDecision, RollbackReport, RollbackStatus,
    is_file_locked_error,
};
use crate::plan::{EffectiveActionKind, ValidatedPlan};
use crate::{FagentError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewChoice {
    Approve,
    Cancel,
    Edit(String),
}

#[derive(Debug, Clone)]
struct MenuOption(&'static str);

impl std::fmt::Display for MenuOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

pub fn review_plan(plan: &ValidatedPlan, instruction: &str) -> Result<ReviewChoice> {
    println!(
        "\nPlanned actions for workspace: {}\n",
        plan.workspace_root.display()
    );
    if !plan.warnings.is_empty() {
        for warning in &plan.warnings {
            println!("warning: {warning}");
        }
        println!();
    }

    println!("{}", render_plan_table(plan));
    print_action_warnings(plan);
    let options = vec![
        MenuOption("Approve"),
        MenuOption("Cancel"),
        MenuOption("Edit instruction"),
    ];

    let choice = match Select::new("How should Fagent proceed?", options).prompt() {
        Ok(choice) => choice,
        Err(inquire::error::InquireError::OperationCanceled)
        | Err(inquire::error::InquireError::OperationInterrupted) => {
            return Ok(ReviewChoice::Cancel);
        }
        Err(error) => return Err(FagentError::from(error)),
    };

    match choice.0 {
        "Approve" => {
            if confirm_risky_deletes(plan)? {
                Ok(ReviewChoice::Approve)
            } else {
                Ok(ReviewChoice::Cancel)
            }
        }
        "Cancel" => Ok(ReviewChoice::Cancel),
        "Edit instruction" => {
            let new_instruction = Text::new("Update the instruction:")
                .with_initial_value(instruction)
                .prompt()?;
            Ok(ReviewChoice::Edit(new_instruction))
        }
        _ => Err(FagentError::Validation("unsupported review option".into())),
    }
}

pub fn render_plan_table(plan: &ValidatedPlan) -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["ID", "Action", "Source", "Destination", "Why"]);

    for action in &plan.actions {
        let color = match action.effective_kind {
            EffectiveActionKind::DeletePermanent | EffectiveActionKind::DeleteToTrash => {
                Some(Color::Red)
            }
            EffectiveActionKind::MoveFile | EffectiveActionKind::RenamePath => Some(Color::Yellow),
            EffectiveActionKind::ZipPath | EffectiveActionKind::UnzipArchive => Some(Color::Blue),
            EffectiveActionKind::CreateDir | EffectiveActionKind::CreateFile => Some(Color::Green),
        };

        let label = match action.effective_kind {
            EffectiveActionKind::CreateDir => "create_dir",
            EffectiveActionKind::CreateFile => "create_file",
            EffectiveActionKind::MoveFile => "move_file",
            EffectiveActionKind::RenamePath => "rename_path",
            EffectiveActionKind::ZipPath => "zip_path",
            EffectiveActionKind::UnzipArchive => "unzip_archive",
            EffectiveActionKind::DeleteToTrash => "delete_to_trash",
            EffectiveActionKind::DeletePermanent => "delete_permanent",
        };

        let mut action_cell = comfy_table::Cell::new(label);
        if let Some(color) = color {
            action_cell = action_cell.fg(color);
        }

        table.add_row(vec![
            comfy_table::Cell::new(&action.id),
            action_cell,
            comfy_table::Cell::new(action.display_source.clone().unwrap_or_default()),
            comfy_table::Cell::new(action.display_destination.clone().unwrap_or_default()),
            comfy_table::Cell::new(action.rationale.clone().unwrap_or_default()),
        ]);
    }

    table
}

fn print_action_warnings(plan: &ValidatedPlan) {
    let warnings = plan
        .actions
        .iter()
        .flat_map(|action| action.warnings.iter());

    let mut printed_any = false;
    for warning in warnings {
        if !printed_any {
            println!();
            printed_any = true;
        }
        println!("warning: {warning}");
    }

    if printed_any {
        println!();
    }
}

fn confirm_risky_deletes(plan: &ValidatedPlan) -> Result<bool> {
    if !plan
        .actions
        .iter()
        .any(|action| !action.warnings.is_empty())
    {
        return Ok(true);
    }

    let confirmation =
        match Text::new("Type DELETE to continue with the high-risk delete actions:").prompt() {
            Ok(value) => value,
            Err(inquire::error::InquireError::OperationCanceled)
            | Err(inquire::error::InquireError::OperationInterrupted) => return Ok(false),
            Err(error) => return Err(FagentError::from(error)),
        };

    Ok(confirmation == "DELETE")
}

pub fn print_execution_report(report: &crate::executor::ExecutionReport) {
    if report.succeeded() {
        println!("\nExecution completed successfully.");
    } else if report.rollback.is_some() {
        println!("\nExecution stopped and completed actions were rolled back.");
    } else {
        println!("\nExecution stopped after a failure.");
    }

    if !report.completed.is_empty() {
        println!("Completed: {}", report.completed.join(", "));
    }

    if let Some(failed) = &report.failed {
        println!("Failed: {} ({})", failed.action_id, failed.message);
    }

    if !report.pending.is_empty() {
        println!("Pending: {}", report.pending.join(", "));
    }

    if report.retries > 0 {
        println!("Retries: {}", report.retries);
    }
}

pub fn print_rollback_summary(report: &RollbackReport) {
    println!("\nRollback Summary:");
    let succeeded = report
        .rolled_back
        .iter()
        .filter(|r| matches!(r.status, RollbackStatus::Success(_)))
        .count();
    let skipped = report
        .rolled_back
        .iter()
        .filter(|r| matches!(r.status, RollbackStatus::Skipped(_)))
        .count();
    let failed = report
        .rolled_back
        .iter()
        .filter(|r| matches!(r.status, RollbackStatus::Failed(_)))
        .count();

    println!("  Total actions considered: {}", report.rolled_back.len());
    println!("  Successfully restored:    {succeeded}");
    if skipped > 0 {
        println!("  Skipped (e.g. permanent): {skipped}");
    }
    if failed > 0 {
        println!("  Failed:                   {failed}");
    }
    if report.success {
        println!("All reversible actions were successfully rolled back.");
    } else {
        println!("Some actions could not be rolled back.");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostFailureChoice {
    Edit(String),
    Exit,
}

pub fn prompt_post_failure_action(current_instruction: &str) -> Result<PostFailureChoice> {
    println!("\nExecution did not complete.");
    let options = vec![
        "Edit instruction and re-plan",
        "Exit",
    ];
    let choice = match Select::new("What would you like to do next?", options).prompt() {
        Ok(c) => c,
        Err(inquire::error::InquireError::OperationCanceled)
        | Err(inquire::error::InquireError::OperationInterrupted) => {
            return Ok(PostFailureChoice::Exit);
        }
        Err(err) => return Err(FagentError::from(err)),
    };

    match choice {
        choice if choice.starts_with("Edit instruction") => {
            let new_instruction = Text::new("Update the instruction:")
                .with_initial_value(current_instruction)
                .prompt()?;
            Ok(PostFailureChoice::Edit(new_instruction))
        }
        _ => Ok(PostFailureChoice::Exit),
    }
}

#[derive(Debug, Clone, Default)]
pub struct InteractiveRecoveryHandler;

impl ExecutionRecoveryHandler for InteractiveRecoveryHandler {
    fn on_action_failure(&self, context: &ExecutionFailureContext) -> Result<RecoveryDecision> {
        let action_desc = format_action_summary(
            &context.effective_kind,
            context.source.as_deref(),
            context.destination.as_deref(),
        );

        println!(
            "\n❌ Execution failed on action '{}' ({})",
            context.action_id, action_desc
        );
        println!("Error: {}\n", context.error);

        if is_file_locked_error(&context.error) {
            println!("💡 File Lock Detected: The file appears to be in use by another process.");
            println!("   Please close applications using this file (or terminate the process in Task Manager),");
            println!("   then choose 'Try again'.\n");
        }

        let mut options = vec!["Try again (retry from the failed action)"];
        if context.completed_actions_count > 0 {
            options.push("Roll back (undo completed actions in reverse order)");
        }
        options.push("Abort (stop execution without rollback)");

        let choice = match Select::new("How would you like to handle this failure?", options).prompt() {
            Ok(c) => c,
            Err(inquire::error::InquireError::OperationCanceled)
            | Err(inquire::error::InquireError::OperationInterrupted) => {
                return Ok(RecoveryDecision::Abort);
            }
            Err(err) => return Err(FagentError::from(err)),
        };

        match choice {
            c if c.starts_with("Try again") => Ok(RecoveryDecision::Retry),
            c if c.starts_with("Roll back") => Ok(RecoveryDecision::Rollback),
            _ => Ok(RecoveryDecision::Abort),
        }
    }

    fn on_rollback_step(&self, report: &RollbackActionReport) {
        match &report.status {
            RollbackStatus::Success(msg) => {
                println!("  ✓ Rolled back [{}]: {}", report.action_id, msg);
            }
            RollbackStatus::Skipped(msg) => {
                println!("  ⚠ Skipped [{}]: {}", report.action_id, msg);
            }
            RollbackStatus::Failed(msg) => {
                println!("  ✗ Failed to roll back [{}]: {}", report.action_id, msg);
            }
        }
    }

    fn on_rollback_failure(
        &self,
        action_id: &str,
        error: &str,
    ) -> Result<RollbackFailureDecision> {
        println!("\n❌ Failed to roll back action '{action_id}': {error}");
        if is_file_locked_error(error) {
            println!("💡 Notice: The file appears to be locked by another process.");
        }

        let options = vec![
            "Try again (retry rollback of this action)",
            "Skip (skip this action and continue rolling back remaining actions)",
            "Abort (stop rollback here)",
        ];

        let choice = match Select::new("What would you like to do?", options).prompt() {
            Ok(c) => c,
            Err(inquire::error::InquireError::OperationCanceled)
            | Err(inquire::error::InquireError::OperationInterrupted) => {
                return Ok(RollbackFailureDecision::Skip);
            }
            Err(err) => return Err(FagentError::from(err)),
        };

        match choice {
            c if c.starts_with("Try again") => Ok(RollbackFailureDecision::Retry),
            c if c.starts_with("Skip") => Ok(RollbackFailureDecision::Skip),
            _ => Ok(RollbackFailureDecision::Abort),
        }
    }
}

fn format_action_summary(
    kind: &EffectiveActionKind,
    source: Option<&Path>,
    destination: Option<&Path>,
) -> String {
    let label = match kind {
        EffectiveActionKind::CreateDir => "create_dir",
        EffectiveActionKind::CreateFile => "create_file",
        EffectiveActionKind::MoveFile => "move_file",
        EffectiveActionKind::RenamePath => "rename_path",
        EffectiveActionKind::ZipPath => "zip_path",
        EffectiveActionKind::UnzipArchive => "unzip_archive",
        EffectiveActionKind::DeleteToTrash => "delete_to_trash",
        EffectiveActionKind::DeletePermanent => "delete_permanent",
    };

    match (source, destination) {
        (Some(src), Some(dst)) => format!("{label}: {} -> {}", src.display(), dst.display()),
        (Some(src), None) => format!("{label}: {}", src.display()),
        (None, Some(dst)) => format!("{label}: {}", dst.display()),
        (None, None) => label.to_string(),
    }
}
