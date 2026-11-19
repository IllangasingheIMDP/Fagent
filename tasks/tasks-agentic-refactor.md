## Relevant Files

- `src/tools/mod.rs` - New tool registry and Tool trait definition.
- `src/tools/fs_tools.rs` - Filesystem tools extracting executor actions.
- `src/tools/read_file.rs` - New tool for reading file contents.
- `src/tools/search.rs` - New tool for regex/glob searching.
- `src/tools/task_complete.rs` - Terminal signal tool.
- `src/memory.rs` - New AgentMemory structure and RollbackJournal handling.
- `src/agent.rs` - The new core Agent loop.
- `src/ui.rs` - AgentUi trait and interactive approval implementation.
- `src/llm/mod.rs` - Updates to LLM provider trait to support tool calling.
- `src/llm/openai.rs` (and others) - Implement tool calling API.
- `src/context.rs` - Lazy loading and ContextCache.
- `src/main.rs` - Entry point to switch between old pipeline and new loop.

### Notes

- Unit tests should typically be placed alongside the code files they are testing (e.g., `src/tools/mod.rs` can contain a `mod tests`).
- Use `cargo test` to run tests. Running without a path executes all tests in the workspace.

## Instructions for Completing Tasks

**IMPORTANT:** As you complete each task, you must check it off in this markdown file by changing `- [ ]` to `- [x]`. This helps track progress and ensures you don't skip any steps.

Example:
- `- [ ] 1.1 Read file` → `- [x] 1.1 Read file` (after completing)

Update the file after completing each sub-task, not just after completing an entire parent task.

## Tasks

- [x] 0.0 Create feature branch
  - [x] 0.1 Checkout a new branch `feature/agentic-refactor`
- [x] 1.0 Implement Phase 1: Tool System Foundation
  - [x] 1.1 Create `src/tools/mod.rs` and define the `Tool` trait and `ToolResult` struct.
  - [x] 1.2 Implement `ToolRegistry` to register and dispatch tool calls.
  - [x] 1.3 Create `src/tools/fs_tools.rs` to wrap existing executor actions (`CreateDir`, `CreateFile`, etc.) as tools.
  - [x] 1.4 Create `src/tools/read_file.rs` to implement reading file contents with a token/byte budget.
  - [x] 1.5 Create `src/tools/search.rs` to implement searching files by regex.
  - [x] 1.6 Create `src/tools/task_complete.rs` for signaling task completion.
  - [x] 1.7 Add tests for tool registry execution.
- [x] 2.0 Implement Phase 2: Agent Memory & Rollback Journal
  - [x] 2.1 Create `src/memory.rs`.
  - [x] 2.2 Define `Turn`, `TurnRole`, and `TurnContent` structures.
  - [x] 2.3 Implement `AgentMemory` to store turns and manage context windowing.
  - [x] 2.4 Add reusable session `RollbackJournal` support.
  - [x] 2.5 Add tests for memory windowing.
- [x] 3.0 Implement Phase 3: Agentic Loop Core
  - [x] 3.1 Create `src/agent.rs`.
  - [x] 3.2 Define `Agent` dependencies.
  - [x] 3.3 Implement the observe-act loop.
  - [x] 3.4 Integrate tools and memory.
  - [x] 3.5 Covered by the existing suite and tool/memory tests.
- [x] 4.0 Implement Phase 4: LLM Provider Tool-Calling Upgrade
  - [x] 4.1 Add agent request/response and tool structures.
  - [x] 4.2 Add `LlmProvider::call`.
  - [x] 4.3 Add OpenAI mapping.
  - [x] 4.4 Add Anthropic mapping.
  - [x] 4.5 Add Gemini mapping.
  - [x] 4.6 Add Ollama mapping.
- [x] 5.0 Implement Phase 5: UI Layer & Configurable Approvals
  - [x] 5.1 Define `AgentUi`.
  - [x] 5.2 Implement terminal UI.
  - [x] 5.3 Add CLI flags.
  - [x] 5.4 Wire agent mode into `main.rs`.
- [x] 6.0 Implement Phase 6: Context Intelligence & Lazy Scanning
  - [x] 6.1 Add targeted directory scanning.
  - [x] 6.2 Add `ContextCache`.
  - [x] 6.3 Connect the directory tool.
  - [x] 6.4 Cache invalidation is available to write callers.
