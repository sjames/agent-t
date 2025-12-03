# Agent-T 

A powerful terminal-based coding agent built with Rust that brings AI-assisted development to your command line. Powered by local LLMs via Ollama, it provides intelligent code assistance with tool execution capabilities, similar to Claude Code but running entirely on your machine.

## Help Needed!
- Help me improve the system prompt and also write some good example agent prompts

## Features

### 🛠️ Comprehensive Tool Suite

- **File Operations**: Read, write, edit files with precision
- **Directory Navigation**: List and explore directory structures
- **Code Search**: Grep patterns and glob file matching
- **Shell Integration**: Execute bash commands with safety guards
- **Background Process Management**: Run, monitor, and control long-running processes
- **Web Capabilities**: Fetch web content and search the internet
- **Math Calculator**: Evaluate mathematical expressions
- **rust-analyzer Integration**: Full LSP support including:
  - Code completion
  - Go to definition
  - Find references
  - Hover documentation
  - Diagnostics
  - Code actions
  - Refactoring/rename
  - Symbol search
  - Code formatting
- **Memory Management**: Long-term memory with routine and key memory systems
- **Sub-agent System**: Spawn independent sub-agents for complex tasks

### 🎯 Advanced Features

- **Traffic Inspector**: Real-time web UI for monitoring LLM requests/responses and tool executions
- **Session Management**: Persistent sessions with save/load/resume capabilities
- **Git Integration**: Automatic repository detection and status tracking
- **Safety First**: Dangerous command detection with user confirmation prompts
- **Streaming Output**: Real-time response streaming for faster feedback
- **Customizable System Prompts**: Override or extend the default agent behavior
- **TUI Support**: Rich terminal user interface with colors and progress indicators

## Prerequisites

- **Rust** (1.70+): Install via [rustup](https://rustup.rs/)
- **Ollama**: Download from [ollama.ai](https://ollama.ai/)
- **rust-analyzer** (optional): For LSP features, ensure it's in your PATH

### Recommended Models

Pull a model from Ollama:
```bash
# Default model
ollama pull qwen3-coder

# Alternative models
ollama pull llama3
ollama pull codellama
ollama pull deepseek-coder
```

## Installation

```bash
# Clone the repository
git clone https://github.com/sabaton-systems/agent-t
cd agent-t

# Build the project
cargo build --release

# Run directly
cargo run --release

# Or install to PATH
cargo install --path .
```

## Quick Start

### Basic Usage

```bash
# Start with default model (qwen3-coder)
cargo run

# Use a specific model
cargo run -- -m llama3

# Enable streaming output
cargo run -- --streaming

# Resume most recent session
cargo run -- --resume
```

### With Traffic Inspector

Monitor all LLM interactions via web UI:

```bash
cargo run -- --inspector

# Custom port
cargo run -- --inspector --inspector-port 3000
```

Then open http://localhost:8080 in your browser to watch real-time traffic.

## Command-Line Options

```
Options:
  -i, --inspector               Enable the traffic inspector web interface
      --inspector-port <PORT>   Port for the traffic inspector [default: 8080]
  -m, --model <MODEL>           Ollama model to use [default: qwen3-coder]
  -r, --resume                  Resume the most recent session
      --session <ID>            Load a specific session by ID
      --no-confirm              Disable dangerous command confirmations
  -u, --ollama-url <URL>        Ollama server URL [default: http://localhost:11434]
  -s, --streaming               Enable streaming output
  -c, --context-size <SIZE>     Context window size (num_ctx) [default: 8192]
  -I, --instructions <TEXT>     Special instructions to append to system prompt
                                (Use @filename to load from file)
  -S, --system-prompt <TEXT>    Override the default system prompt
                                (Use @filename to load from file)
  -h, --help                    Print help
```

## Available Tools

The agent has access to the following tools:

### File Operations
- `read_file` - Read file contents with optional line ranges
- `write_file` - Create or completely overwrite files
- `edit_file` - Replace specific text matches in files
- `list_dir` - List directory contents
- `glob_files` - Find files matching glob patterns
- `grep_search` - Search for patterns using ripgrep

### Execution
- `bash` - Execute shell commands with timeout
- `bash_status` - Check status of background processes
- `bash_output` - Read output from background processes
- `bash_kill` - Terminate background processes
- `bash_list` - List all running background processes

### Web Access
- `web_fetch` - Fetch and process web page content
- `web_search` - Search the web for information

### Code Intelligence (rust-analyzer)
- `ra_completion` - Get code completions
- `ra_goto_definition` - Jump to symbol definitions
- `ra_find_references` - Find all references to a symbol
- `ra_hover` - Get hover documentation
- `ra_diagnostics` - Get compiler errors and warnings
- `ra_code_actions` - Get available code actions/quick fixes
- `ra_rename` - Refactor/rename symbols
- `ra_symbols` - Search workspace symbols
- `ra_format` - Format code using rustfmt

### Memory Management
- `store_key_memory` - Store important information in long-term memory
- `search_routine_memory` - Search past conversation history
- `search_key_memory` - Search curated important memories

### Sub-agent System
- `spawn_agent` - Spawn independent sub-agents for complex tasks

### Utilities
- `math_calc` - Evaluate mathematical expressions

## In-Session Commands

While interacting with the agent, you can use these commands:

- `exit` or `quit` - Exit the session
- `clear` - Clear the conversation history
- `save [name]` - Save the current session
- `sessions` - List all saved sessions
- `load <id>` - Load a saved session
- `changes` - Show all file modifications made in this session
- `git` - Show git repository status
- `usage` - Display token usage statistics

## Architecture

### Core Components

- **main.rs** - CLI entry point with argument parsing and REPL loop
- **agent_loop.rs** - Agentic loop controller managing conversation history, tool execution, and iteration limits (max 25)
- **tools/mod.rs** - Tool registry where all tools implement `rig::tool::Tool` trait
- **inspector.rs** - Web-based traffic visualization using axum/WebSocket
- **session.rs** - Session persistence in `~/.local/share/agent-t/sessions/` (Linux) 
- **terminal.rs** - Colored output, progress spinners, and dangerous command detection
- **rust_analyzer.rs** - LSP client for rust-analyzer integration
- **process_manager.rs** - Background process lifecycle management

### Tool Execution Flow

1. User input → LLM with tool definitions
2. LLM responds with tool calls
3. Tools executed with results captured
4. Results sent back to LLM
5. Repeat until text response (max 25 iterations)

### Safety Features

- Dangerous command patterns detected (`rm -rf`, `sudo`, etc.)
- Dangerous path protection (`/`, `/etc`, `/usr`, etc.)
- User confirmation prompts for risky operations
- Can be disabled with `--no-confirm` flag

## Session Management

Sessions are automatically saved and can be:

- **Resumed**: Continue your last conversation
- **Loaded**: Restore any previous session by ID
- **Saved**: Explicitly save with a custom name
- **Listed**: View all saved sessions

Sessions are stored as JSON files containing the complete conversation history and metadata.

## Development

### Adding New Tools

1. Create a new file in `src/tools/`
2. Implement the `rig::tool::Tool` trait
3. Add the tool to `src/tools/mod.rs`
4. Register it in `agent_loop.rs`

### Building and Testing

```bash
# Development build
cargo build

# Release build (optimized)
cargo build --release

# Run tests
cargo test

# Run with logging
RUST_LOG=debug cargo run

# Check code
cargo clippy
```

## Examples

### Advanced Usage Example

Run agent-t with inspector, large context window, vector database, and remote Ollama server:

```bash
agent-t -i -c 100000 --vecdb -u http://192.168.1.9:11434
```

This command:
- `-i` - Enables the traffic inspector web UI at http://localhost:8080
- `-c 100000` - Sets a large context window of 100k tokens (model must support this)
- `--vecdb` - Enables vector database for enhanced context retrieval
- `-u http://192.168.1.9:11434` - Connects to Ollama running on a network machine at 192.168.1.9

## Tips

- Use `--streaming` for faster feedback on long responses
- Enable `--inspector` when debugging tool execution
- Use `--no-confirm` in trusted environments to skip confirmations
- Provide custom instructions with `-I @path/to/instructions.txt`
- Adjust context size with `-c` for larger codebases (requires model support)

## Troubleshooting

**Ollama connection failed**: Ensure Ollama is running (`ollama serve`) and the URL is correct

**rust-analyzer tools not working**: Make sure `rust-analyzer` is installed and in your PATH

**Model not found**: Pull the model first: `ollama pull qwen3-coder`

**Out of context**: Reduce context with `clear` command or use a smaller model, or increase with `-c`

## Contributing

Contributions are welcome! Please feel free to submit issues or pull requests.

## License

This project is licensed under either of:

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Acknowledgments

- Built with [rig-core](https://github.com/0xPlaygrounds/rig) for LLM integration
- Powered by [Ollama](https://ollama.ai/) for local LLM inference
- Inspired by Claude Code and similar AI coding assistants
