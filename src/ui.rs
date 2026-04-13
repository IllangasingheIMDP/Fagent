use comfy_table::{Color, Table, presets::UTF8_FULL};
use inquire::{Select, Text};

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
        "Approve" => Ok(ReviewChoice::Approve),
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
            EffectiveActionKind::CreateDir => Some(Color::Green),
        };

        let label = match action.effective_kind {
            EffectiveActionKind::CreateDir => "create_dir",
            EffectiveActionKind::MoveFile => "move_file",
            EffectiveActionKind::RenamePath => "rename_path",
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

pub fn print_execution_report(report: &crate::executor::ExecutionReport) {
    if report.succeeded() {
        println!("\nExecution completed successfully.");
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
}
