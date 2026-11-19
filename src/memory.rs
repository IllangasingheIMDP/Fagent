use crate::executor::{ExecutedActionRecord, Executor, RollbackReport};
use crate::llm::Message;
use crate::tools::ToolResult;

#[derive(Debug, Clone, Default)]
pub struct RollbackJournal {
    pub completed: Vec<ExecutedActionRecord>,
}
impl RollbackJournal {
    pub fn push(&mut self, records: Vec<ExecutedActionRecord>) {
        self.completed.extend(records);
    }
    pub fn is_empty(&self) -> bool {
        self.completed.is_empty()
    }
    pub async fn rollback(&self, executor: &Executor) -> RollbackReport {
        executor.rollback(&self.completed).await
    }
}

#[derive(Debug, Clone)]
pub enum TurnRole {
    System,
    User,
    Assistant,
    ToolResult,
}
#[derive(Debug, Clone)]
pub enum TurnContent {
    Text(String),
    ToolCall {
        name: String,
        params: serde_json::Value,
    },
    ToolResult {
        name: String,
        result: ToolResult,
    },
}
#[derive(Debug, Clone)]
pub struct Turn {
    pub role: TurnRole,
    pub content: TurnContent,
}
#[derive(Debug, Clone)]
pub struct AgentMemory {
    pub goal: String,
    pub turns: Vec<Turn>,
    pub journal: RollbackJournal,
    pub token_budget: usize,
}
impl AgentMemory {
    pub fn new(goal: String, token_budget: usize) -> Self {
        Self {
            goal,
            turns: Vec::new(),
            journal: RollbackJournal::default(),
            token_budget,
        }
    }
    pub fn push_user_message(&mut self, text: String) {
        self.turns.push(Turn {
            role: TurnRole::User,
            content: TurnContent::Text(text),
        });
        self.trim();
    }
    pub fn push_assistant_message(&mut self, text: String) {
        self.turns.push(Turn {
            role: TurnRole::Assistant,
            content: TurnContent::Text(text),
        });
        self.trim();
    }
    pub fn push_tool_call(&mut self, name: String, params: serde_json::Value) {
        self.turns.push(Turn {
            role: TurnRole::Assistant,
            content: TurnContent::ToolCall { name, params },
        });
        self.trim();
    }
    pub fn push_tool_result(&mut self, name: String, result: ToolResult) {
        self.journal.push(result.side_effects.clone());
        self.turns.push(Turn {
            role: TurnRole::ToolResult,
            content: TurnContent::ToolResult { name, result },
        });
        self.trim();
    }
    pub fn messages(&self) -> Vec<Message> {
        let mut messages = vec![
            Message::system(
                "You are Fagent, a careful staged filesystem agent. First inspect the environment and identify constraints, ambiguity, and information needed to safely fulfill the goal. Use only list_directory, read_file, and search_paths while gathering evidence; their results are automatically observed. Then submit one coherent ordered batch with execute_stage for the next stage. A stage contains all dependent filesystem actions that can safely be executed together, not one action per stage. The user approves each execute_stage batch once. After every stage, use observations to plan the next stage. Do not call filesystem mutation tools directly; execute_stage is the only mutation route. Call task_complete only when the goal is complete.",
            ),
            Message::user(self.goal.clone()),
        ];
        for turn in &self.turns {
            match &turn.content {
                TurnContent::Text(text) => messages.push(match turn.role {
                    TurnRole::User => Message::user(text.clone()),
                    _ => Message::assistant(text.clone()),
                }),
                TurnContent::ToolCall { name, params } => {
                    messages.push(Message::assistant(format!("Calling {name} with {params}")))
                }
                TurnContent::ToolResult { name, result } => messages.push(Message::tool(
                    name.clone(),
                    result.output.clone(),
                    result.is_error,
                )),
            }
        }
        messages
    }
    fn trim(&mut self) {
        while self
            .turns
            .iter()
            .map(|t| format!("{:?}", t).len())
            .sum::<usize>()
            > self.token_budget.saturating_mul(4)
            && self.turns.len() > 2
        {
            self.turns.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn window_keeps_recent_turns() {
        let mut m = AgentMemory::new("g".into(), 1);
        m.push_user_message("a".repeat(100));
        m.push_user_message("b".repeat(100));
        assert!(m.turns.len() <= 2);
    }
}
