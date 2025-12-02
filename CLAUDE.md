# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

agent-t is a terminal-based coding agent built with Rust and rig-core. It uses local LLMs via Ollama (default: qwen3-coder) to provide Claude Code-like functionality with tool execution capabilities.

## Build & Run Commands

```bash
# Build
cargo build

# Run (default model: qwen3-coder)
cargo run

# Run with traffic inspector enabled (web UI at localhost:8080)
cargo run -- --inspector

# Run with specific model
cargo run -- -m llama3

# Run with streaming output
cargo run -- --streaming

# Resume most recent session
cargo run -- --resume

# Custom Ollama URL
cargo run -- -u http://custom-host:11434
```

## Architecture

### Core Components

- **`main.rs`** - CLI entry point using clap. Handles args, session management, and the main REPL loop. Defines the system prompt and in-session commands (exit, clear, save, sessions, load, changes, git, usage).

- **`agent_loop.rs`** - The agentic loop controller (`AgentLoop<M: CompletionModel>`). Manages conversation history, tool definitions, tool execution dispatch, file change tracking, and token usage estimation. Limits iterations to 25 to prevent infinite loops.

- **`tools/mod.rs`** - Tool registry. Each tool implements `rig::tool::Tool` trait with `NAME`, `Args`, `Output`, `definition()`, and `call()`.

### Tool Implementations

All tools are in `src/tools/`:
- `read_file.rs` - Read file contents with optional line range
- `write_file.rs` - Create or overwrite files
- `edit_file.rs` - Replace exact text matches in files
- `list_dir.rs` - List directory contents
- `bash.rs` - Execute shell commands with timeout
- `grep.rs` - Search patterns using ripgrep
- `glob_files.rs` - Find files by glob pattern

### Supporting Modules

- **`inspector.rs`** - Web-based traffic visualization using axum/WebSocket. Real-time monitoring of LLM requests/responses and tool executions.

- **`session.rs`** - Session persistence. Stores sessions as JSON in `~/.local/share/agent-t/sessions/` (Linux) .

- **`terminal.rs`** - Colored output, progress spinners (indicatif), dangerous command detection, and user confirmation prompts.

- **`git.rs`** - Git repository status detection.

### Key Patterns

1. **Tool execution flow**: User input → LLM with tool definitions → tool calls parsed → tools executed → results sent back to LLM → repeat until text response.

2. **Dangerous operation guards**: The bash and write_file tools check against patterns in `terminal.rs` (`DANGEROUS_PATTERNS`, `DANGEROUS_PATHS`) and prompt for confirmation unless `--no-confirm` is set.

3. **Working directory context**: Tools receive the working directory and resolve relative paths against it.

## Interactive Shell Commands

When using agent-t in interactive mode, you can execute shell commands directly by prefixing them with `!`:

```
!ls -la
!git status
!cargo test
```

### Behavior

- **Direct execution**: Commands are executed immediately without going through the LLM agent
- **Working directory**: Commands run in the current working directory
- **Output handling**:
  - Standard output is displayed as info messages
  - Standard error is appended after stdout (with "--- stderr ---" separator)
  - Non-zero exit codes are highlighted as warnings with the exit code displayed
- **Timeout**: Commands have a 600-second (10 minute) timeout
- **Error handling**: Failures to execute are shown as error messages

### Examples

```
# List files
!ls -la

# Check git status
!git status

# Run tests
!cargo test

# Chain commands
!git add . && git commit -m "Update"

# Use pipes
!ps aux | grep rust
```

### Implementation Details

Shell commands are processed in `main.rs` (lines 816-898) before regular command processing. They bypass the agent loop entirely for faster execution.

## Agent-Specific System Prompts

Each agent can have a custom system prompt file that is appended to the base generic prompt and supports template variable substitution.

### File Location

`~/.agent-t/agents/<name>/system_prompt.md`

### Behavior

- **Appended to base**: Content is appended to the base prompt (from `prompts/system.txt` or CLI `--system-prompt`)
- **Template support**: Supports template variables like `{{working_dir}}`, `{{model}}`, `{{project_name}}`, etc.
- **Precedence**: Takes complete precedence over `agent.json` fields (`system_prompt_override`, `system_prompt_additions`, `personality`, `description`)
- **Graceful fallback**: Empty files or read errors fall back to `agent.json` configuration
- **One-time load**: Read once at startup (restart to reload changes)

### Priority Order

System prompt is built in this order:
1. CLI `--system-prompt` (overrides base if provided)
2. Base prompt (`prompts/system.txt` or default)
3. **`system_prompt.md` file** (appended if exists) ← Takes precedence over agent.json
4. `agent.json` fields (fallback if no file: `system_prompt_override`, `personality`, `description`, `system_prompt_additions`)
5. Template variable substitution (applied to result of steps 1-4)
6. CLI `--instructions` (appended if provided)
7. Session context (appended if memory enabled)

### Available Template Variables

- `{{date}}` - Current date (YYYY-MM-DD)
- `{{time}}` - Current time with timezone
- `{{datetime}}` - Full date and time
- `{{timezone}}` - Current timezone
- `{{username}}` - System username
- `{{hostname}}` - System hostname
- `{{os}}` - Operating system
- `{{platform}}` - OS and architecture
- `{{working_dir}}` - Current working directory (absolute path)
- `{{project_name}}` - Name of current directory
- `{{git_branch}}` - Current git branch (empty if not a git repo)
- `{{git_status}}` - "dirty" or "clean"
- `{{model}}` - LLM model name (e.g., "qwen3-coder")
- `{{agent_name}}` - Name of the agent

### Example Usage

**Create agent-specific prompt:**
```bash
mkdir -p ~/.agent-t/agents/rust-expert
cat > ~/.agent-t/agents/rust-expert/system_prompt.md << 'EOF'
# Specialized Rust Development Agent

You are {{agent_name}}, an expert Rust developer working on {{project_name}}.

## Focus Areas
- Performance optimization and zero-cost abstractions
- Memory safety and lifetime analysis
- Async/await patterns with Tokio
- Error handling with anyhow/thiserror

## Context
- Working directory: {{working_dir}}
- Git branch: {{git_branch}}
- Model: {{model}}
- Date: {{date}}

## Approach
1. Always explain ownership and borrowing decisions
2. Prefer zero-copy operations when possible
3. Write comprehensive tests for all new code
4. Use clippy suggestions to improve code quality
EOF
```

**Use the agent:**
```bash
cargo run -- --agent rust-expert
# [INFO] Loaded agent-specific system prompt from ~/.agent-t/agents/rust-expert/system_prompt.md
```

### Troubleshooting

**File not being loaded?**
- Check file exists: `ls -la ~/.agent-t/agents/<name>/system_prompt.md`
- Check file is not empty
- Check file permissions: `chmod 644 ~/.agent-t/agents/<name>/system_prompt.md`
- Look for error messages when starting agent

**Template variables not substituted?**
- Verify variable name is correct (case-sensitive)
- Check spelling against available variables list
- Unknown variables are left as-is (e.g., `{{typo}}` stays `{{typo}}`)

**Want to use agent.json instead?**
- Delete or rename `system_prompt.md` file
- Or create empty `system_prompt.md` file

## Dependencies

Key crates: `rig-core` (LLM framework), `tokio` (async runtime), `axum` (web server for inspector), `clap` (CLI), `indicatif` (progress bars), `colored` (terminal colors), `chrono`/`uuid`/`dirs` (session management).
