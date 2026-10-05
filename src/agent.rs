use crate::executor::Executor;
use crate::llm::{AgentRequest, LlmProvider, LlmResponse};
use crate::memory::AgentMemory;
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
                    tools: self.tools.specs(),
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
                    self.memory.push_user_message(ui.prompt_user()?);
                }
                LlmResponse::ToolCalls(calls) => {
                    for call in calls {
                        self.memory
                            .push_tool_call(call.name.clone(), call.params.clone());
                        if ui.should_approve(&call)? {
                            let result =
                                self.tools.call(&call.name, call.params, &self.policy).await;
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
}
