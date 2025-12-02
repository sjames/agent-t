# Terminal Coding Agent Implementation Plan

A terminal-based coding agent built with Rust and rig-core, similar to Claude Code.

## Overview

This agent uses:
- **rig-core** - LLM framework for Rust
- **Ollama** - Local LLM provider
- **qwen3-coder** - Code-focused model

## Project Structure

```
src/
├── main.rs           # Entry point & chat loop
├── agent.rs          # Agent configuration & loop
├── tools/
│   ├── mod.rs        # Tool exports & ToolSet
│   ├── read_file.rs  # Read file contents
│   ├── write_file.rs # Write/create files
│   ├── edit_file.rs  # Edit existing files
│   ├── list_dir.rs   # List directory contents
│   ├── bash.rs       # Execute shell commands
│   ├── grep.rs       # Search file contents
│   └── glob.rs       # Find files by pattern
└── error.rs          # Common error types
```

---

## Phase 1: Project Structure & Dependencies

### Dependencies to Add

| Crate | Purpose |
|-------|---------|
| `serde` | Serialization for tool args |
| `serde_json` | JSON schema for tool definitions |
| `glob` | File pattern matching |
| `thiserror` | Error type derivation |

### Tasks
- [x] Analyze current codebase
- [x] Update Cargo.toml with dependencies
- [x] Create tools/ module structure
- [x] Create error.rs with common error types

---

## Phase 2: Core Tool Implementations

### Tool Trait Pattern

Each tool implements `rig::tool::Tool`:

```rust
impl Tool for MyTool {
    const NAME: &'static str = "tool_name";
    type Error = ToolError;
    type Args = MyToolArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition { ... }
    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> { ... }
}
```

### Tools to Implement

| Tool | Priority | Description |
|------|----------|-------------|
| `ReadFile` | P0 | Read file contents with optional line range |
| `WriteFile` | P0 | Create or overwrite files |
| `Bash` | P0 | Execute shell commands |
| `ListDir` | P1 | List directory contents |
| `EditFile` | P1 | Replace text in existing files |
| `Grep` | P1 | Search for patterns in files |
| `Glob` | P2 | Find files matching patterns |

### Tasks
- [x] Implement ReadFile tool
- [x] Implement WriteFile tool
- [x] Implement Bash tool
- [x] Implement ListDir tool
- [x] Implement EditFile tool
- [x] Implement Grep tool
- [x] Implement Glob tool

---

## Phase 3: Agent Loop with Tool Execution

### Agentic Loop Flow

```
┌─────────────────────────────────────────────────┐
│                  User Input                      │
└─────────────────────┬───────────────────────────┘
                      ▼
┌─────────────────────────────────────────────────┐
│              Send to Model                       │
│         (with conversation history)              │
└─────────────────────┬───────────────────────────┘
                      ▼
┌─────────────────────────────────────────────────┐
│            Parse Model Response                  │
│    ┌────────────────┴────────────────┐          │
│    ▼                                 ▼          │
│ Tool Call?                      Text Response   │
│    │                                 │          │
│    ▼                                 ▼          │
│ Execute Tool(s)                 Display to User │
│    │                                 │          │
│    ▼                                 │          │
│ Send Results                         │          │
│ Back to Model ──────────────────────►│          │
└─────────────────────────────────────────────────┘
```

### Tasks
- [x] Implement conversation history management
- [x] Implement tool execution loop
- [x] Handle multi-tool calls in single response
- [ ] Add streaming output support (future enhancement)

---

## Phase 4: Traffic Inspector (COMPLETED)

A web-based traffic visualization feature to debug LLM communication.

### Features
- [x] CLI flag `--inspector` / `-i` to enable
- [x] Configurable port via `--inspector-port`
- [x] Real-time WebSocket updates
- [x] Message history with JSON viewer
- [x] Filter by message type (Request/Response/Tool/System)
- [x] Duration timing for requests and tool executions
- [x] Syntax-highlighted JSON content

### Usage
```bash
# Run with traffic inspector enabled on port 8080
cargo run -- --inspector

# Run with custom port
cargo run -- --inspector --inspector-port 9000

# Run with different model
cargo run -- -m llama3 --inspector
```

Then open http://localhost:8080 in your browser.

---

## Phase 5: Enhanced UX (COMPLETED)

### Safety & UX
- [x] Confirmation prompts for dangerous operations (rm -rf, sudo, etc.)
- [x] Colored terminal output with syntax highlighting
- [x] Session persistence with auto-save
- [x] Streaming output support

### New CLI Options
```bash
--inspector, -i      Enable traffic inspector
--inspector-port     Port for inspector (default: 8080)
--model, -m          Ollama model to use
--resume, -r         Resume most recent session
--session <id>       Load specific session
--no-confirm         Disable dangerous command confirmations
--ollama-url, -u     Custom Ollama server URL (default: http://localhost:11434)
--streaming, -s      Enable streaming output (tokens printed as they arrive)
```

### New Commands
| Command | Description |
|---------|-------------|
| `save` | Save current session |
| `sessions` | List saved sessions |
| `load <id>` | Load a saved session |
| `clear` | Clear conversation history |
| `exit` | Save and quit |

### Session Storage
Sessions are stored in:
- Linux: `~/.local/share/agent-t/sessions/`
- macOS: `~/Library/Application Support/agent-t/sessions/`

---

## Phase 6: Enhanced Features (COMPLETED)

### Implemented Features
- [x] Working directory context tracking
- [x] Progress indicators for long operations (spinners for LLM and tool execution)
- [x] File change summaries (`changes` command)
- [x] Git integration awareness (`git` command, status at startup)
- [x] Token usage tracking (`usage` command, estimated tokens)

---

## Technical Notes

### Rig-core Version
Using `rig-core = "0.24.0"` with Ollama provider.

### Tool Definition JSON Schema
Tools must provide a JSON schema describing their parameters:

```rust
async fn definition(&self, _prompt: String) -> ToolDefinition {
    ToolDefinition {
        name: "read_file".to_string(),
        description: "Read contents of a file".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file"
                }
            },
            "required": ["file_path"]
        })
    }
}
```

### Error Handling Strategy
- Tools return `Result<Output, ToolError>`
- Errors are sent back to the model as tool results
- Model can retry or explain the failure to user

---

## Resources

- [rig-core docs](https://docs.rs/rig-core/latest/rig/)
- [Rig documentation](https://docs.rig.rs/)
- [Rig GitHub](https://github.com/0xPlaygrounds/rig)
