# Fagent

Fagent is an AI-assisted filesystem agent written in Rust. It converts a natural-language instruction into a safe, reviewable action plan, asks for your approval, and then executes the plan.

It is designed for practical local file operations with strong guardrails:
- Workspace jail by default (blocks path escape and traversal)
- Human approval gate before execution
- Action-by-action validation and fail-fast execution
- Soft delete by default (trash/recycle bin) unless you opt into permanent deletion

## What Fagent Can Do

Current action kinds:
- `create_dir`
- `create_file` (text file with explicit content)
- `move_file`
- `rename_path`
- `zip_path` (create a `.zip` archive from a file or directory)
- `unzip_archive` (extract a `.zip` file into a destination directory)
- `delete_path` (to trash by default, permanent with flag)

Typical use cases:
- Organize messy folders
- Create missing project scaffolding files
- Rename or move files in bulk (via planned sequential actions)
- Clean up generated artifacts after review

## How It Works

1. You run Fagent with an instruction.
2. Fagent scans the workspace (bounded depth).
3. It sends instruction + workspace context to the selected LLM provider.
4. The model returns a JSON execution plan.
5. Fagent validates the plan against safety and consistency rules.
6. You review actions in a terminal table and choose:
	 - Approve
	 - Cancel
	 - Edit instruction
7. If approved, actions execute in order with fail-fast behavior.

## Safety Model

- Workspace jail: relative paths are resolved under the current working directory by default.
- Escape protection: traversal and symlink escapes are blocked unless `--allow-global` is enabled.
- Reserved name checks: Windows device names are rejected.
- Plan consistency checks:
	- required fields per action
	- destination conflicts
	- invalid action ordering
- Deletion behavior:
	- default: route deletes to OS trash/recycle bin
	- `--permanent-delete`: permanently remove files/directories
	- workspace-root deletion is blocked
	- repository metadata directories such as `.git` are protected from deletion
	- risky deletes (permanent, recursive directory, outside-workspace) require extra confirmation

## Supported Providers

- OpenAI
- Anthropic
- Gemini
- Ollama

Default models:
- OpenAI: `gpt-4.1-mini`
- Anthropic: `claude-3-7-sonnet-latest`
- Gemini: `gemini-2.5-flash`
- Ollama: `llama3.1:8b`

## Requirements

- Rust toolchain (stable)
- Network access for cloud providers (OpenAI/Anthropic/Gemini)
- Local Ollama instance if using Ollama provider

## Build and Run

From the project root:

```bash
cargo build
```

Run in debug mode:

```bash
cargo run -- "organize files by extension"
```

Run with verbose logs:

```bash
cargo run -- --verbose "create a notes folder and move all .md files into it"
```

Build optimized binary:

```bash
cargo build --release
```

Binary locations:
- Debug: `target/debug/fagent` (or `fagent.exe` on Windows)
- Release: `target/release/fagent` (or `fagent.exe` on Windows)

## First-Time Setup

Run interactive setup:

```bash
cargo run -- setup
```

Setup flow:
- Choose default provider
- Choose default model
- Enter API key (for non-Ollama providers)
- API key is stored in OS keychain
- Config is written to platform config directory

Config path resolution:
- Windows: `%APPDATA%/fagent/config.toml`
- Linux/BSD: `$XDG_CONFIG_HOME/fagent/config.toml` or `$HOME/.config/fagent/config.toml`

## Authentication and Config Precedence

For cloud providers, Fagent resolves API keys in this order:
1. Environment variable
2. OS keychain entry

Environment variables:
- OpenAI: `OPENAI_API_KEY`
- Anthropic: `ANTHROPIC_API_KEY`
- Gemini: `GEMINI_API_KEY`
- Ollama base URL: `OLLAMA_BASE_URL` (default: `http://127.0.0.1:11434`)

Runtime provider/model selection precedence:
1. CLI flags (`--provider`, `--model`)
2. Config file defaults
3. Built-in defaults

## CLI Reference

```text
fagent [OPTIONS] <INSTRUCTION>
fagent setup
```

Options:
- `--provider <openai|anthropic|gemini|ollama>`
- `--model <MODEL>`
- `--scan-depth <N>` (default: `1`)
- `--allow-global`
- `--permanent-delete`
- `-v, --verbose`

## Usage Examples

Create a text file via natural language:

```bash
cargo run -- "create a file docs/hello.txt with content hello world"
```

Use Gemini explicitly:

```bash
cargo run -- --provider gemini --model gemini-2.5-flash "rename src/old.txt to src/new.txt"
```

Use Ollama with a local model:

```bash
cargo run -- --provider ollama --model llama3.1:8b "create scripts/run.bat with content @echo off"
```

## Logging and Sensitive Data

- Use `--verbose` to enable info-level logs.
- Provider error mapping is sanitized to avoid leaking credential-bearing URLs in error messages.
- If you add custom logs, avoid printing secrets from headers, environment variables, or keychain values.

## Development

Run tests:

```bash
cargo test
```

Quick type and compile checks:

```bash
cargo check
```

## Project Structure

```text
src/
	cli.rs        # clap CLI definitions
	config.rs     # setup, config loading, keychain/env resolution
	context.rs    # workspace scan and compact context JSON
	executor.rs   # action execution engine
	llm/          # provider clients and prompt composition
	plan.rs       # plan schema and validation
	security.rs   # workspace jail and path safety
	ui.rs         # interactive review and result output
	main.rs       # orchestration entry point
```

## Current Limitations

- `create_file` is text-oriented (binary file generation is not modeled).
- Planning quality depends on provider/model and context depth.
- Very large workspaces are truncated in context to stay within size limits.

## Contributing

Contributions are welcome. Please open an issue or PR with:
- Clear problem statement
- Reproduction steps (if bug)
- Expected behavior
- Proposed fix and tests

## License

No license file is currently included in this repository. 
