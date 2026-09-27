use clap::Parser;
use tracing_subscriber::EnvFilter;

use fagent::cli::{Cli, Command};
use fagent::config::{self, ResolvedConfig};
use fagent::context;
use fagent::executor::Executor;
use fagent::llm::{self, PlanRequest};
use fagent::plan;
use fagent::security::WorkspacePolicy;
use fagent::ui::{self, ReviewChoice};
use fagent::{FagentError, Result};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose)?;

    match cli.command {
        Some(Command::Setup) => {
            config::run_setup()?;
            return Ok(());
        }
        None => {}
    }

    let instruction = cli
        .instruction
        .clone()
        .ok_or_else(|| FagentError::Validation("an instruction is required".into()))?;
    let runtime = config::resolve_runtime(cli.provider.clone(), cli.model.clone())?;
    let workspace_root = std::env::current_dir()?;
    let policy = WorkspacePolicy::new(workspace_root, cli.allow_global, cli.permanent_delete)?;

    run_instruction_loop(instruction, cli.scan_depth, runtime, policy).await
}

fn init_tracing(verbose: bool) -> Result<()> {
    let env_filter = if verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };

    let builder = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false);

    let init_result = if verbose {
        builder
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .try_init()
    } else {
        builder
            .without_time()
            .try_init()
    };

    init_result.map_err(|error| {
        FagentError::Validation(format!("failed to initialize logging: {error}"))
    })?;

    Ok(())
}

async fn run_instruction_loop(
    mut instruction: String,
    scan_depth: usize,
    runtime: ResolvedConfig,
    policy: WorkspacePolicy,
) -> Result<()> {
    let provider = llm::build_provider(&runtime)?;
    let executor = Executor::new(policy.clone());

    loop {
        let context = context::scan_workspace(policy.root(), scan_depth)?;
        let request = PlanRequest::new(
            instruction.clone(),
            runtime.model.clone(),
            policy.root().display().to_string(),
            scan_depth,
            context.to_compact_json()?,
            policy.allow_global,
            policy.permanent_delete,
        );

        let raw_plan = provider.plan(&request).await?;
        let validated_plan = plan::validate_plan(raw_plan, &policy)?;

        match ui::review_plan(&validated_plan, &instruction)? {
            ReviewChoice::Approve => {
                let handler = ui::InteractiveRecoveryHandler;
                let report = executor.run_with_recovery(&validated_plan, &handler).await;
                ui::print_execution_report(&report);
                if let Some(rollback) = &report.rollback {
                    ui::print_rollback_summary(rollback);
                }
                if report.succeeded() {
                    return Ok(());
                }
                match ui::prompt_post_failure_action(&instruction)? {
                    ui::PostFailureChoice::Edit(new_instruction) => {
                        instruction = new_instruction;
                        continue;
                    }
                    ui::PostFailureChoice::Exit => {
                        return Err(FagentError::Execution(
                            "execution stopped after failure".into(),
                        ));
                    }
                }
            }
            ReviewChoice::Cancel => return Ok(()),
            ReviewChoice::Edit(new_instruction) => instruction = new_instruction,
        }
    }
}
