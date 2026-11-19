use crate::executor::Executor;
use crate::llm::{AgentRequest, LlmProvider, LlmResponse, ToolCallRequest, stage_tool_spec};
use crate::memory::AgentMemory;
use crate::plan::{ActionKind, ExecutionPlan, PlannedAction, validate_plan};
use crate::security::WorkspacePolicy;
use crate::tools::ToolRegistry;
use crate::ui::AgentUi;
use crate::{FagentError, Result};
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub max_turns: usize,
    pub max_tool_retries: usize,
}
impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: 30,
            max_tool_retries: 1,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentOutcome {
    Done { summary: String },
    MaxTurns,
}
pub struct Agent {
    pub provider: Box<dyn LlmProvider>,
    pub tools: ToolRegistry,
    pub memory: AgentMemory,
    pub policy: WorkspacePolicy,
    pub model: String,
    pub config: AgentConfig,
}
impl Agent {
    pub fn new(
        provider: Box<dyn LlmProvider>,
        goal: String,
        policy: WorkspacePolicy,
        model: String,
        config: AgentConfig,
    ) -> Self {
        Self {
            provider,
            tools: ToolRegistry::standard(),
            memory: AgentMemory::new(goal, 8_000),
            policy,
            model,
            config,
        }
    }
    pub async fn run(&mut self, ui: &dyn AgentUi) -> Result<AgentOutcome> {
        for turn in 0..self.config.max_turns {
            ui.print_turn_header(turn + 1);
            let response = self
                .provider
                .call(&AgentRequest {
                    messages: self.memory.messages(),
                    tools: self.agent_tools(),
                    model: self.model.clone(),
                })
                .await?;
            match response {
                LlmResponse::TaskComplete { summary } => {
                    ui.print_summary(&summary);
                    return Ok(AgentOutcome::Done { summary });
                }
                LlmResponse::Message(text) => {
                    ui.print_message(&text);
                    self.memory.push_assistant_message(text);
                }
                LlmResponse::ToolCalls(calls) => {
                    for call in calls {
                        if call.name == "execute_stage" {
                            self.execute_stage(call, ui).await?;
                            continue;
                        }
                        self.memory
                            .push_tool_call(call.name.clone(), call.params.clone());
                        let result = self.tools.call(&call.name, call.params, &self.policy).await;
                        ui.print_tool_result(&call.name, &result);
                        let complete = result.task_complete;
                        let summary = result.output.clone();
                        self.memory.push_tool_result(call.name, result);
                        if complete {
                            ui.print_summary(&summary);
                            return Ok(AgentOutcome::Done { summary });
                        }
                    }
                }
            }
        }
        Err(FagentError::Execution(format!(
            "agent stopped after {} turns",
            self.config.max_turns
        )))
    }
    pub async fn rollback(&self) -> crate::executor::RollbackReport {
        self.memory
            .journal
            .rollback(&Executor::new(self.policy.clone()))
            .await
    }

    fn agent_tools(&self) -> Vec<crate::tools::ToolSpec> {
        let mut tools = self.tools.specs_named(&[
            "list_directory",
            "read_file",
            "search_paths",
            "task_complete",
        ]);
        tools.push(stage_tool_spec());
        tools
    }

    async fn execute_stage(&mut self, call: ToolCallRequest, ui: &dyn AgentUi) -> Result<()> {
        let stage_name = call
            .params
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Unnamed stage")
            .to_owned();
        let objective = call
            .params
            .get("objective")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        let actions = call
            .params
            .get("actions")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                FagentError::Provider("execute_stage requires an actions array".into())
            })?;
        let actions = actions
            .iter()
            .enumerate()
            .map(|(index, action)| {
                let kind = match action.get("kind").and_then(|v| v.as_str()) {
                    Some("create_dir") => ActionKind::CreateDir,
                    Some("create_file") => ActionKind::CreateFile,
                    Some("move_file") => ActionKind::MoveFile,
                    Some("rename_path") => ActionKind::RenamePath,
                    Some("zip_path") => ActionKind::ZipPath,
                    Some("unzip_archive") => ActionKind::UnzipArchive,
                    Some("delete_path") => ActionKind::DeletePath,
                    _ => {
                        return Err(FagentError::Provider(format!(
                            "stage `{stage_name}` has an unsupported action kind"
                        )));
                    }
                };
                Ok(PlannedAction {
                    id: format!("stage-{}", index + 1),
                    kind,
                    source: action
                        .get("source")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                    destination: action
                        .get("destination")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                    content: action
                        .get("content")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                    rationale: action
                        .get("rationale")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let plan = validate_plan(
            ExecutionPlan {
                workspace_root: None,
                warnings: vec![],
                actions,
            },
            &self.policy,
        )?;
        self.memory
            .push_tool_call("execute_stage".into(), call.params);
        if !ui.should_approve_stage(&stage_name, &objective, &plan)? {
            self.memory.push_user_message(format!(
                "The user declined stage `{stage_name}`. Revise the plan or ask for clarification."
            ));
            return Ok(());
        }
        let executor = Executor::new(self.policy.clone());
        for action in &plan.actions {
            match executor.execute_action(action).await {
                Ok(record) => self.memory.journal.push(vec![record]),
                Err(error) => {
                    return Err(FagentError::Execution(format!(
                        "stage `{stage_name}` failed on {}: {error}",
                        action.id
                    )));
                }
            }
        }
        let result =
            crate::tools::ToolResult::ok(format!("Stage `{stage_name}` completed: {objective}"));
        ui.print_stage_result(&stage_name, &result);
        self.memory.push_tool_result("execute_stage".into(), result);
        Ok(())
    }
}
