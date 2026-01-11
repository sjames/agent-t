use crate::error::ToolError;
use crate::inspector::TrafficHandle;
use crate::memory::types::RoutineMemoryChunk;
use crate::permissions::GrantedPermissions;
use crate::terminal;
use crate::vecdb::VectorDB;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use crate::tools::{
    ra_common, BashCommand, BashKill, BashList, BashOutput, BashStatus, EditFile, GlobFiles,
    GrepSearch, ListDir, MathCalc, RaCodeActions, RaCompletion, RaDiagnostics, RaFindReferences,
    RaFormat, RaGotoDefinition, RaHover, RaRename, RaSymbols, ReadFile, SearchKeyMemory,
    SearchRoutineMemory, StoreKeyMemory, WebFetch, WebSearch, WriteFile,
};
use crate::tui::TuiEvent;
use anyhow::{anyhow, Result};
use futures::StreamExt;
use rig::completion::message::{AssistantContent, ToolCall, ToolResultContent};
use rig::completion::{CompletionModel, Message, ToolDefinition};
use rig::message::{ToolResult, UserContent};
use rig::one_or_many::OneOrMany;
use rig::streaming::StreamedAssistantContent;
use rig::tool::Tool;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use tokio::sync::mpsc::Sender;
use serde::Deserialize;

/// Arguments for spawning a sub-agent
#[derive(Debug, Deserialize)]
pub struct SpawnAgentArgs {
    /// Instructions for the sub-agent
    pub instructions: String,
    /// Optional max iterations for the sub-agent (default: 100)
    pub max_iterations: Option<usize>,
    /// Optional timeout in seconds (default: 300)
    pub timeout_secs: Option<u64>,
    /// Optional additional content to append to the system prompt/preamble
    pub preamble_append: Option<String>,
}

/// Default maximum iterations for main agents
const DEFAULT_MAX_ITERATIONS: usize = 100;

/// Default maximum iterations for sub-agents
const DEFAULT_SUBAGENT_MAX_ITERATIONS: usize = 100;

/// Maximum agent nesting depth to prevent infinite recursion
const MAX_DEPTH: usize = 3;

/// Tracks a file modification
#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: String,
    pub operation: FileOperation,
    /// Timestamp when the change was recorded (for future use in displaying change history)
    #[allow(dead_code)]
    pub timestamp: Instant,
}

/// Token usage tracking
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
    pub request_count: usize,
}

impl TokenUsage {
    /// Estimate tokens from text (rough: ~4 chars per token)
    pub fn estimate_tokens(text: &str) -> usize {
        text.len().div_ceil(4) // Round up
    }

    /// Add estimated usage for a request/response pair
    pub fn add_estimated(&mut self, prompt: &str, completion: &str) {
        let prompt_est = Self::estimate_tokens(prompt);
        let completion_est = Self::estimate_tokens(completion);

        self.prompt_tokens += prompt_est;
        self.completion_tokens += completion_est;
        self.total_tokens += prompt_est + completion_est;
        self.request_count += 1;
    }
}

/// Type of file operation
#[derive(Debug, Clone, PartialEq)]
pub enum FileOperation {
    Created,
    Modified,
    /// Reserved for future use when file deletion tracking is implemented
    #[allow(dead_code)]
    Deleted,
}

impl std::fmt::Display for FileOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileOperation::Created => write!(f, "created"),
            FileOperation::Modified => write!(f, "modified"),
            FileOperation::Deleted => write!(f, "deleted"),
        }
    }
}

/// The agentic loop controller that manages conversations with tool execution
pub struct AgentLoop<M: CompletionModel> {
    model: M,
    preamble: String,
    chat_history: Vec<Message>,
    traffic: TrafficHandle,
    confirm_dangerous: bool,
    streaming: bool,
    working_directory: String,
    /// Tracks file changes made during the session
    file_changes: HashMap<String, FileChange>,
    /// Tracks token usage
    token_usage: TokenUsage,
    /// Optional TUI event sender (None = use direct terminal printing)
    tui_tx: Option<Sender<TuiEvent>>,
    /// Tools that have been approved for all future uses
    approved_tools: HashSet<String>,
    /// Context window size (num_ctx parameter for LLM)
    context_size: usize,
    /// Optional vector database for code context
    vecdb: Option<Arc<tokio::sync::Mutex<VectorDB>>>,
    /// Optional memory manager for long-term memory
    memory_manager: Option<Arc<tokio::sync::Mutex<crate::memory::MemoryManager>>>,
    /// Session ID for tracking memory context
    session_id: Option<String>,
    /// Agent nesting depth (0 = main agent, 1+ = sub-agent)
    depth: usize,
    /// Number of iterations used in the current chat session
    iteration_count: usize,
    /// Maximum iterations allowed for this agent
    max_iterations: usize,
    /// Agent ID for event routing ("main" or UUID for sub-agents)
    agent_id: String,
    /// Cancellation token for interrupting agent execution
    cancel_token: CancellationToken,
    /// Granted permissions for batch mode
    permissions: GrantedPermissions,
    /// Model name for memory tracking
    model_name: String,
}

impl<M: CompletionModel> AgentLoop<M> {
    pub fn new(
        model: M,
        preamble: String,
        traffic: TrafficHandle,
        confirm_dangerous: bool,
        streaming: bool,
        working_directory: String,
        context_size: usize,
        vecdb: Option<Arc<tokio::sync::Mutex<VectorDB>>>,
        memory_manager: Option<Arc<tokio::sync::Mutex<crate::memory::MemoryManager>>>,
        session_id: Option<String>,
        depth: usize,
        cancel_token: CancellationToken,
        permissions: GrantedPermissions,
        model_name: String,
    ) -> Self {
        // Use DEFAULT_MAX_ITERATIONS for main agents (depth 0)
        let max_iterations = if depth == 0 {
            DEFAULT_MAX_ITERATIONS
        } else {
            DEFAULT_SUBAGENT_MAX_ITERATIONS
        };

        // Main agent has "main" ID, will be set properly for sub-agents later
        let agent_id = if depth == 0 {
            "main".to_string()
        } else {
            // This will be overridden when creating sub-agents
            format!("agent-{}", uuid::Uuid::new_v4())
        };

        Self {
            model,
            preamble,
            chat_history: Vec::new(),
            traffic,
            confirm_dangerous,
            streaming,
            working_directory,
            file_changes: HashMap::new(),
            token_usage: TokenUsage::default(),
            tui_tx: None,
            approved_tools: HashSet::new(),
            context_size,
            vecdb,
            memory_manager,
            session_id,
            depth,
            iteration_count: 0,
            max_iterations,
            agent_id,
            cancel_token,
            permissions,
            model_name,
        }
    }

    /// Set the TUI event sender for event-driven output
    pub fn set_tui_sender(&mut self, tx: Sender<TuiEvent>) {
        self.tui_tx = Some(tx);
    }

    /// Set the maximum iterations for this agent
    pub fn set_max_iterations(&mut self, max_iterations: usize) {
        self.max_iterations = max_iterations;
    }

    /// Set the agent ID (for sub-agents)
    pub fn set_agent_id(&mut self, agent_id: String) {
        self.agent_id = agent_id;
    }

    /// Get current token usage
    pub fn get_token_usage(&self) -> &TokenUsage {
        &self.token_usage
    }

    /// Get the number of iterations used in the current chat session
    pub fn iteration_count(&self) -> usize {
        self.iteration_count
    }

    /// Run a sub-agent with the given instructions (sequential execution)
    fn run_subagent(
        &self,
        mut agent: AgentLoop<M>,
        args: SpawnAgentArgs,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, anyhow::Error>> + Send + '_>>
    where
        M: Clone,
    {
        Box::pin(async move {
            use tokio::time::{timeout, Duration};

            // Generate unique agent ID and create tab
            let agent_id = uuid::Uuid::new_v4().to_string();
            let agent_name = format!("Agent-{}", &agent_id[..8]);

            // Set the sub-agent's ID for event routing
            agent.set_agent_id(agent_id.clone());

            // Notify TUI to create a tab for this sub-agent
            if let Some(ref tx) = self.tui_tx {
                let _ = tx.try_send(TuiEvent::TabCreate {
                    agent_id: agent_id.clone(),
                    name: agent_name.clone(),
                });
            }

            let timeout_duration = Duration::from_secs(args.timeout_secs.unwrap_or(300));

            // Execute with timeout (this blocks until sub-agent completes)
            //eprintln!("DEBUG run_subagent: Starting sub-agent {} with instructions: {}",
            //         agent_id, &args.instructions[..args.instructions.len().min(100)]);
            let start_time = std::time::Instant::now();
            let result = timeout(timeout_duration, agent.chat(&args.instructions)).await;
            let elapsed = start_time.elapsed();
            //eprintln!("DEBUG run_subagent: Sub-agent {} completed in {:?}", agent_id, elapsed);

            // Notify TUI of completion or failure
            match result {
                Ok(Ok(response)) => {
                    // Sub-agent succeeded
                    let file_count = agent.file_changes_count();
                    let iterations = agent.iteration_count();

                    // Create completion summary
                    let summary = format!(
                        "Sub-agent completed successfully.\n\
                         Result: {}\n\
                         Files changed: {}\n\
                         Iterations used: {}",
                        response, file_count, iterations
                    );

                    // Display final response and completion info in the sub-agent's tab
                    if let Some(ref tx) = self.tui_tx {
                        // Show the agent's final response
                        let _ = tx.try_send(TuiEvent::AssistantMessage {
                            agent_id: agent_id.clone(),
                            text: response.clone(),
                        });

                        // Show completion info
                        let _ = tx.try_send(TuiEvent::Info {
                            agent_id: agent_id.clone(),
                            text: format!("✓ Task completed\n\nFiles changed: {}\nIterations: {}", file_count, iterations),
                        });

                        // Notify tab completion
                        let _ = tx.try_send(TuiEvent::TabComplete {
                            agent_id: agent_id.clone(),
                        });
                    }

                    Ok(summary)
                }
                Ok(Err(e)) => {
                    // Sub-agent failed
                    let error_msg = format!("Sub-agent failed: {}", e);
                    if let Some(ref tx) = self.tui_tx {
                        let _ = tx.try_send(TuiEvent::TabFailed {
                            agent_id: agent_id.clone(),
                            error: error_msg.clone(),
                        });
                    }
                    Err(anyhow!(error_msg))
                }
                Err(_) => {
                    // Timeout
                    let error_msg = format!(
                        "Sub-agent timed out after {} seconds",
                        timeout_duration.as_secs()
                    );
                    if let Some(ref tx) = self.tui_tx {
                        let _ = tx.try_send(TuiEvent::TabFailed {
                            agent_id: agent_id.clone(),
                            error: error_msg.clone(),
                        });
                    }
                    Err(anyhow!(error_msg))
                }
            }
        })
    }

    /// Record a file change
    fn record_file_change(&mut self, path: &str, operation: FileOperation) {
        // Normalize path for consistent tracking
        let normalized_path = if std::path::Path::new(path).is_absolute() {
            path.to_string()
        } else {
            std::path::Path::new(&self.working_directory)
                .join(path)
                .to_string_lossy()
                .to_string()
        };

        self.file_changes.insert(
            normalized_path.clone(),
            FileChange {
                path: normalized_path,
                operation,
                timestamp: Instant::now(),
            },
        );
    }

    /// Get a summary of file changes
    pub fn get_file_changes_summary(&self) -> Vec<&FileChange> {
        let mut changes: Vec<_> = self.file_changes.values().collect();
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        changes
    }

    /// Get count of file changes
    pub fn file_changes_count(&self) -> usize {
        self.file_changes.len()
    }

    /// Clear file change history
    pub fn clear_file_changes(&mut self) {
        self.file_changes.clear();
    }

    /// Get all tool definitions for the agent
    async fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        let cwd_note = format!("Relative paths are resolved from: {}", self.working_directory);
        let mut tools = vec![
            ToolDefinition {
                name: "read_file".to_string(),
                description: format!("Read the contents of a file. Returns the file content with line numbers. {}", cwd_note),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Path to the file (absolute or relative to working directory)"
                        },
                        "offset": {
                            "type": "integer",
                            "description": "Optional starting line number (1-indexed)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Optional number of lines to read"
                        }
                    },
                    "required": ["file_path"]
                }),
            },
            ToolDefinition {
                name: "write_file".to_string(),
                description: format!("Write content to a file. Creates the file if it doesn't exist, or overwrites if it does. {}", cwd_note),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Path to the file (absolute or relative to working directory)"
                        },
                        "content": {
                            "type": "string",
                            "description": "The content to write to the file"
                        }
                    },
                    "required": ["file_path", "content"]
                }),
            },
            ToolDefinition {
                name: "edit_file".to_string(),
                description: format!("Edit a file by replacing exact text matches. {}", cwd_note),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Path to the file (absolute or relative to working directory)"
                        },
                        "old_string": {
                            "type": "string",
                            "description": "The exact text to find and replace"
                        },
                        "new_string": {
                            "type": "string",
                            "description": "The text to replace it with"
                        },
                        "replace_all": {
                            "type": "boolean",
                            "description": "Whether to replace all occurrences (default: false)"
                        }
                    },
                    "required": ["file_path", "old_string", "new_string"]
                }),
            },
            ToolDefinition {
                name: "list_dir".to_string(),
                description: format!("List the contents of a directory. {}", cwd_note),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the directory (absolute or relative to working directory). Use '.' for current directory."
                        }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: "bash".to_string(),
                description: format!("Execute a bash command and return the output. Can run in background for long-running tasks. Commands run in: {}", self.working_directory),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The bash command to execute"
                        },
                        "working_dir": {
                            "type": "string",
                            "description": "Optional working directory (defaults to project root)"
                        },
                        "timeout_secs": {
                            "type": "integer",
                            "description": "Optional timeout in seconds (default: 600, max recommended: 1800). Increase this for long-running operations like builds, tests, or installations. Ignored if background=true."
                        },
                        "background": {
                            "type": "boolean",
                            "description": "Execute in background and return immediately with process ID. Use bash_status/bash_output/bash_kill tools to manage."
                        }
                    },
                    "required": ["command"]
                }),
            },
            ToolDefinition {
                name: "grep".to_string(),
                description: format!("Search for a pattern in files using ripgrep. {}", cwd_note),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "The regex pattern to search for"
                        },
                        "path": {
                            "type": "string",
                            "description": "File or directory to search (defaults to working directory)"
                        },
                        "ignore_case": {
                            "type": "boolean",
                            "description": "Whether to ignore case"
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Maximum number of results"
                        }
                    },
                    "required": ["pattern"]
                }),
            },
            ToolDefinition {
                name: "glob".to_string(),
                description: format!("Find files matching a glob pattern. {}", cwd_note),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "The glob pattern to match files (e.g., '**/*.rs', 'src/*.txt')"
                        },
                        "base_dir": {
                            "type": "string",
                            "description": "Base directory for search (defaults to working directory)"
                        }
                    },
                    "required": ["pattern"]
                }),
            },
            ToolDefinition {
                name: "bash_status".to_string(),
                description: "Check the status of a background bash process. Returns whether the process is running, completed, or failed.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "process_id": {
                            "type": "string",
                            "description": "The process ID returned by bash command with background=true"
                        }
                    },
                    "required": ["process_id"]
                }),
            },
            ToolDefinition {
                name: "bash_output".to_string(),
                description: "Get the accumulated stdout and stderr output from a background bash process.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "process_id": {
                            "type": "string",
                            "description": "The process ID returned by bash command with background=true"
                        }
                    },
                    "required": ["process_id"]
                }),
            },
            ToolDefinition {
                name: "bash_kill".to_string(),
                description: "Terminate a background bash process.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "process_id": {
                            "type": "string",
                            "description": "The process ID returned by bash command with background=true"
                        }
                    },
                    "required": ["process_id"]
                }),
            },
            ToolDefinition {
                name: "bash_list".to_string(),
                description: "List all background bash processes with their IDs, commands, and status.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
            ToolDefinition {
                name: "web_fetch".to_string(),
                description: "Fetch content from a URL. Automatically converts HTML to readable text. Returns content with metadata (status, content type, final URL).".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL to fetch (must start with http:// or https://)"
                        },
                        "size_limit_kb": {
                            "type": "integer",
                            "description": "Optional size limit in KB (default: 100KB, max: 500KB)"
                        }
                    },
                    "required": ["url"]
                }),
            },
            ToolDefinition {
                name: "web_search".to_string(),
                description: "Search the web using DuckDuckGo. Returns a list of search results with title, URL, and snippet for each result.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query"
                        },
                        "num_results": {
                            "type": "integer",
                            "description": "Number of results to return (default: 5, max: 10)"
                        }
                    },
                    "required": ["query"]
                }),
            },
            ToolDefinition {
                name: "math_calc".to_string(),
                description: "Evaluate mathematical expressions. Supports basic arithmetic (+, -, *, /), exponentiation (^), parentheses, and common mathematical functions (sin, cos, tan, sqrt, log, ln, abs, etc.). Use this tool for any mathematical calculations.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "expression": {
                            "type": "string",
                            "description": "The mathematical expression to evaluate (e.g., '2 + 2', 'sqrt(16)', 'sin(3.14159/2)', '2^10')"
                        }
                    },
                    "required": ["expression"]
                }),
            },
        ];

        // Add memory tools if memory is enabled
        if self.memory_manager.is_some() {
            tools.push(ToolDefinition {
                name: "store_key_memory".to_string(),
                description: "Store an important piece of information in long-term memory. Use this when you learn something important that should be remembered across sessions, such as user preferences, project facts, code patterns, problem solutions, or personal information. For session continuity, store a session summary before the user ends the conversation.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "The information to remember. Be concise but complete."
                        },
                        "category": {
                            "type": "string",
                            "enum": ["user_preference", "project_fact", "code_pattern", "problem_solution", "user_instruction", "personal_info", "session_summary"],
                            "description": "Category: user_preference (user likes/dislikes), project_fact (tech stack, architecture), code_pattern (common patterns used), problem_solution (how bugs were fixed), user_instruction (explicit instructions for future), personal_info (about the user), session_summary (current work state and next steps for continuity)"
                        },
                        "importance": {
                            "type": "string",
                            "enum": ["low", "medium", "high", "critical"],
                            "description": "Importance level. Critical=must never forget (user preferences), High=very useful (frequent patterns), Medium=useful context, Low=nice to have"
                        },
                        "tags": {
                            "type": "array",
                            "items": {
                                "type": "string"
                            },
                            "description": "Optional tags for easier retrieval"
                        },
                        "related_files": {
                            "type": "array",
                            "items": {
                                "type": "string"
                            },
                            "description": "Optional file paths related to this memory"
                        }
                    },
                    "required": ["content", "category", "importance"]
                }),
            });

            tools.push(ToolDefinition {
                name: "search_routine_memory".to_string(),
                description: "Search through past conversation history using semantic search. Use this when the user asks 'What did we discuss about...', 'Remember when we...', or you need context from previous conversations.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query describing what you're looking for"
                        },
                        "top_k": {
                            "type": "integer",
                            "description": "Number of results to return (default: 5, max: 20)",
                            "minimum": 1,
                            "maximum": 20
                        }
                    },
                    "required": ["query"]
                }),
            });

            tools.push(ToolDefinition {
                name: "search_key_memory".to_string(),
                description: "Search through curated important memories (user preferences, project facts, code patterns, etc.). Use this to recall what you've learned about the user, project, or important decisions. Use at the start of conversations to check for relevant context or session summaries.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query (e.g., 'user coding preferences', 'authentication approach', 'last session')"
                        },
                        "top_k": {
                            "type": "integer",
                            "description": "Number of results to return (default: 5, max: 20)",
                            "minimum": 1,
                            "maximum": 20
                        },
                        "categories": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": ["user_preference", "project_fact", "code_pattern", "problem_solution", "user_instruction", "personal_info", "session_summary"]
                            },
                            "description": "Optional: Filter by specific categories"
                        },
                        "min_importance": {
                            "type": "string",
                            "enum": ["low", "medium", "high", "critical"],
                            "description": "Optional: Only return memories at or above this importance level"
                        }
                    },
                    "required": ["query"]
                }),
            });
        }

        tools.push(ToolDefinition {
                name: "spawn_agent".to_string(),
                description: format!(
                    "Spawn an independent sub-agent to work on a separate task. \
                     The sub-agent has its own context and can use all tools. \
                     Use this to delegate focused tasks that can be completed independently. \
                     Current depth: {}/{}",
                    self.depth, MAX_DEPTH
                ),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "instructions": {
                            "type": "string",
                            "description": "Clear, specific instructions for what the sub-agent should accomplish"
                        },
                        "max_iterations": {
                            "type": "integer",
                            "description": "Optional maximum iterations for the sub-agent (default: 100)"
                        },
                        "timeout_secs": {
                            "type": "integer",
                            "description": "Optional timeout in seconds (default: 300)"
                        },
                        "preamble_append": {
                            "type": "string",
                            "description": "Optional additional content to append to the sub-agent's system prompt. Use this to give the sub-agent specialized context, role, or constraints."
                        }
                    },
                    "required": ["instructions"]
                }),
            });

        // Only add rust-analyzer tools if rust-analyzer is available
        if ra_common::is_available().await {
            tools.extend(vec![
            ToolDefinition {
                name: "ra_diagnostics".to_string(),
                description: "Get diagnostics (errors and warnings) from rust-analyzer for the current Rust project.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Optional file path to get diagnostics for"
                        }
                    },
                    "required": []
                }),
            },
            ToolDefinition {
                name: "ra_goto_definition".to_string(),
                description: "Find the definition of a symbol at a specific position in a Rust file.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string", "description": "Path to the file"},
                        "line": {"type": "integer", "description": "Line number (1-indexed)"},
                        "column": {"type": "integer", "description": "Column number (1-indexed)"}
                    },
                    "required": ["file_path", "line", "column"]
                }),
            },
            ToolDefinition {
                name: "ra_find_references".to_string(),
                description: "Find all references to a symbol at a specific position in a Rust file.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string", "description": "Path to the file"},
                        "line": {"type": "integer", "description": "Line number (1-indexed)"},
                        "column": {"type": "integer", "description": "Column number (1-indexed)"},
                        "include_declaration": {"type": "boolean", "description": "Include declaration (default: true)"}
                    },
                    "required": ["file_path", "line", "column"]
                }),
            },
            ToolDefinition {
                name: "ra_hover".to_string(),
                description: "Get hover information (type, docs) for a symbol at a specific position in a Rust file.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string", "description": "Path to the file"},
                        "line": {"type": "integer", "description": "Line number (1-indexed)"},
                        "column": {"type": "integer", "description": "Column number (1-indexed)"}
                    },
                    "required": ["file_path", "line", "column"]
                }),
            },
            ToolDefinition {
                name: "ra_symbols".to_string(),
                description: "Get symbols from a Rust file or search workspace symbols.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string", "description": "Optional file path for document symbols"},
                        "query": {"type": "string", "description": "Search query for workspace symbols"}
                    },
                    "required": []
                }),
            },
            ToolDefinition {
                name: "ra_completion".to_string(),
                description: "Get code completion suggestions at a specific position in a Rust file.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string", "description": "Path to the file"},
                        "line": {"type": "integer", "description": "Line number (1-indexed)"},
                        "column": {"type": "integer", "description": "Column number (1-indexed)"}
                    },
                    "required": ["file_path", "line", "column"]
                }),
            },
            ToolDefinition {
                name: "ra_code_actions".to_string(),
                description: "Get available code actions (refactorings, quick fixes) for a range in a Rust file.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string", "description": "Path to the file"},
                        "start_line": {"type": "integer", "description": "Start line (1-indexed)"},
                        "start_column": {"type": "integer", "description": "Start column (1-indexed)"},
                        "end_line": {"type": "integer", "description": "End line (1-indexed)"},
                        "end_column": {"type": "integer", "description": "End column (1-indexed)"}
                    },
                    "required": ["file_path", "start_line", "start_column"]
                }),
            },
            ToolDefinition {
                name: "ra_rename".to_string(),
                description: "Rename a symbol at a specific position in a Rust file across the workspace.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string", "description": "Path to the file"},
                        "line": {"type": "integer", "description": "Line number (1-indexed)"},
                        "column": {"type": "integer", "description": "Column number (1-indexed)"},
                        "new_name": {"type": "string", "description": "New name for the symbol"}
                    },
                    "required": ["file_path", "line", "column", "new_name"]
                }),
            },
            ToolDefinition {
                name: "ra_format".to_string(),
                description: "Format a Rust file using rust-analyzer's formatter.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string", "description": "Path to the file to format"}
                    },
                    "required": ["file_path"]
                }),
            },
            ]);
        }

        tools
    }

    /// Generate a diff for file operation tools (write_file, edit_file)
    async fn generate_diff_for_tool(&self, tool_name: &str, args: &Value) -> Option<crate::diff::UnifiedDiff> {
        use tokio::fs;

        match tool_name {
            "write_file" => {
                // Get file path and new content
                let file_path = args.get("file_path")?.as_str()?;
                let new_content = args.get("content")?.as_str()?;

                // Resolve path relative to working directory if needed
                let path = if std::path::Path::new(file_path).is_absolute() {
                    std::path::PathBuf::from(file_path)
                } else {
                    std::path::Path::new(&self.working_directory).join(file_path)
                };

                // Read existing file if it exists
                let old_content = if path.exists() {
                    fs::read_to_string(&path).await.unwrap_or_default()
                } else {
                    String::new()
                };

                // Generate diff
                Some(crate::diff::UnifiedDiff::from_texts(
                    file_path.to_string(),
                    &old_content,
                    new_content,
                ))
            }
            "edit_file" => {
                // Get file path and strings to replace
                let file_path = args.get("file_path")?.as_str()?;
                let old_string = args.get("old_string")?.as_str()?;
                let new_string = args.get("new_string")?.as_str()?;

                // Resolve path relative to working directory if needed
                let path = if std::path::Path::new(file_path).is_absolute() {
                    std::path::PathBuf::from(file_path)
                } else {
                    std::path::Path::new(&self.working_directory).join(file_path)
                };

                // Read existing file
                if !path.exists() {
                    return None;
                }

                let old_content = fs::read_to_string(&path).await.ok()?;

                // Check if old_string exists
                if !old_content.contains(old_string) {
                    return None;
                }

                // Simulate the replacement
                let replace_all = args.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
                let new_content = if replace_all {
                    old_content.replace(old_string, new_string)
                } else {
                    old_content.replacen(old_string, new_string, 1)
                };

                // Generate diff
                Some(crate::diff::UnifiedDiff::from_texts(
                    file_path.to_string(),
                    &old_content,
                    &new_content,
                ))
            }
            _ => None,
        }
    }

    /// Request permission to execute a tool
    async fn request_permission(&mut self, tool_name: &str, args: &HashMap<String, String>, diff: Option<crate::diff::UnifiedDiff>) -> bool {
        // Check if tool is already approved for all
        if self.approved_tools.contains(tool_name) {
            return true;
        }

        // If no TUI sender, auto-approve (fallback for non-TUI mode)
        let Some(ref tx) = self.tui_tx else {
            return true;
        };

        // Create response channel
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        // Send permission request
        let event = crate::tui::TuiEvent::PermissionRequest {
            tool_name: tool_name.to_string(),
            args: args.clone(),
            diff,
            response_tx,
        };

        if tx.send(event).await.is_err() {
            // Failed to send request, default to reject
            return false;
        }

        // Wait for response
        match response_rx.await {
            Ok(crate::tui::PermissionDecision::ApproveOnce) => true,
            Ok(crate::tui::PermissionDecision::ApproveAll) => {
                self.approved_tools.insert(tool_name.to_string());
                true
            }
            Ok(crate::tui::PermissionDecision::Reject) => false,
            Err(_) => false, // Channel closed, default to reject
        }
    }

    /// Execute a tool by name with the given arguments
    async fn execute_tool(&self, name: &str, args: Value) -> Result<String, ToolError>
    where
        M: Clone,
    {
        match name {
            "read_file" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                ReadFile.call(tool_args).await
            }
            "write_file" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                WriteFile.call(tool_args).await
            }
            "edit_file" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                EditFile.call(tool_args).await
            }
            "list_dir" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                ListDir.call(tool_args).await
            }
            "bash" => {
                // Inject default working directory if not specified
                let mut args_with_cwd = args;
                if let Some(obj) = args_with_cwd.as_object_mut()
                    && !obj.contains_key("working_dir") {
                        obj.insert("working_dir".to_string(), serde_json::Value::String(self.working_directory.clone()));
                    }
                let tool_args = serde_json::from_value(args_with_cwd)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                BashCommand.call(tool_args).await
            }
            "grep" => {
                // Inject default path if not specified
                let mut args_with_path = args;
                if let Some(obj) = args_with_path.as_object_mut()
                    && !obj.contains_key("path") {
                        obj.insert("path".to_string(), serde_json::Value::String(self.working_directory.clone()));
                    }
                let tool_args = serde_json::from_value(args_with_path)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                GrepSearch.call(tool_args).await
            }
            "glob" => {
                // Inject default base_dir if not specified
                let mut args_with_base = args;
                if let Some(obj) = args_with_base.as_object_mut()
                    && !obj.contains_key("base_dir") {
                        obj.insert("base_dir".to_string(), serde_json::Value::String(self.working_directory.clone()));
                    }
                let tool_args = serde_json::from_value(args_with_base)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                GlobFiles.call(tool_args).await
            }
            "bash_status" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                BashStatus.call(tool_args).await
            }
            "bash_output" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                BashOutput.call(tool_args).await
            }
            "bash_kill" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                BashKill.call(tool_args).await
            }
            "bash_list" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                BashList.call(tool_args).await
            }
            "web_fetch" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                WebFetch.call(tool_args).await
            }
            "web_search" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                WebSearch.call(tool_args).await
            }
            "math_calc" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                MathCalc.call(tool_args).await
            }
            "store_key_memory" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                StoreKeyMemory {
                    memory_manager: self.memory_manager.clone(),
                    session_id: self.session_id.clone(),
                }
                .call(tool_args)
                .await
            }
            "search_routine_memory" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                SearchRoutineMemory {
                    memory_manager: self.memory_manager.clone(),
                }
                .call(tool_args)
                .await
            }
            "search_key_memory" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                SearchKeyMemory {
                    memory_manager: self.memory_manager.clone(),
                }
                .call(tool_args)
                .await
            }
            "spawn_agent" => {
                // Check depth limit
                if self.depth >= MAX_DEPTH {
                    return Err(ToolError::Other(format!(
                        "Maximum agent depth ({}) reached. Cannot spawn more sub-agents.",
                        MAX_DEPTH
                    )));
                }

                // Parse args
                let tool_args: SpawnAgentArgs = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;

                // Build preamble for sub-agent (append additional content if provided)
                let sub_agent_preamble = if let Some(ref append) = tool_args.preamble_append {
                    format!("{}\n\n{}", self.preamble, append)
                } else {
                    self.preamble.clone()
                };

                // Create sub-agent with depth + 1
                let mut sub_agent = AgentLoop::new(
                    self.model.clone(),  // Share model
                    sub_agent_preamble,  // System prompt with optional append
                    self.traffic.clone(),  // Share inspector
                    self.confirm_dangerous,
                    false,  // Disable streaming for sub-agents
                    self.working_directory.clone(),
                    self.context_size,
                    self.vecdb.clone(),
                    self.memory_manager.clone(),  // Share memory manager
                    self.session_id.clone(),  // Share session ID
                    self.depth + 1,  // Increment depth
                    self.cancel_token.clone(),  // Share cancellation token
                    self.permissions.clone(),  // Share permissions
                    self.model_name.clone(),  // Share model name
                );

                // Set custom max_iterations if provided, otherwise use default (100 for sub-agents)
                if let Some(max_iter) = tool_args.max_iterations {
                    sub_agent.set_max_iterations(max_iter);
                }

                // Pass TUI sender to sub-agent so it can send events
                if let Some(ref tx) = self.tui_tx {
                    sub_agent.set_tui_sender(tx.clone());
                }

                // Execute sub-agent with timeout
                self.run_subagent(sub_agent, tool_args)
                    .await
                    .map_err(|e| ToolError::Other(e.to_string()))
            }
            // Rust Analyzer tools
            "ra_diagnostics" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                RaDiagnostics.call(tool_args).await
            }
            "ra_goto_definition" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                RaGotoDefinition.call(tool_args).await
            }
            "ra_find_references" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                RaFindReferences.call(tool_args).await
            }
            "ra_hover" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                RaHover.call(tool_args).await
            }
            "ra_symbols" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                RaSymbols.call(tool_args).await
            }
            "ra_completion" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                RaCompletion.call(tool_args).await
            }
            "ra_code_actions" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                RaCodeActions.call(tool_args).await
            }
            "ra_rename" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                RaRename.call(tool_args).await
            }
            "ra_format" => {
                let tool_args = serde_json::from_value(args)
                    .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
                RaFormat.call(tool_args).await
            }
            _ => Err(ToolError::invalid_arguments(format!(
                "Unknown tool: {}",
                name
            ))),
        }
    }

    /// Serialize messages for logging
    fn serialize_messages(&self) -> Value {
        serde_json::json!(self.chat_history.iter().map(|m| {
            match m {
                Message::User { content } => {
                    serde_json::json!({
                        "role": "user",
                        "content": format!("{:?}", content)
                    })
                }
                Message::Assistant { id, content } => {
                    serde_json::json!({
                        "role": "assistant",
                        "id": id,
                        "content": format!("{:?}", content)
                    })
                }
            }
        }).collect::<Vec<_>>())
    }

    /// Process a user message and run the agentic loop until completion
    pub async fn chat(&mut self, user_input: &str) -> Result<String> {
        //eprintln!("DEBUG chat(): agent_id={}, depth={}, chat_history_len={}, iteration_count={}, max_iterations={}",
        //         self.agent_id, self.depth, self.chat_history.len(), self.iteration_count, self.max_iterations);

        // Search vector database for relevant code context if available
        let mut enriched_input = user_input.to_string();
        if let Some(ref vecdb) = self.vecdb {
            let db = vecdb.lock().await;
            match db.search(user_input, 3).await {
                Ok(results) => {
                    if !results.is_empty() {
                        let mut context = String::from("\n\n[Relevant code context from vector database]:\n");
                        for (idx, (chunk, score)) in results.iter().enumerate() {
                            context.push_str(&format!(
                                "\n--- Context {} (similarity: {:.2}) ---\nFile: {}:{}-{}\nLanguage: {}\n```\n{}\n```\n",
                                idx + 1,
                                score,
                                chunk.file_path,
                                chunk.start_line,
                                chunk.end_line,
                                chunk.language,
                                chunk.content
                            ));
                        }
                        enriched_input.push_str(&context);

                        // Log vector search results with full context details
                        let context_chunks: Vec<_> = results.iter().map(|(chunk, score)| {
                            serde_json::json!({
                                "file_path": &chunk.file_path,
                                "start_line": chunk.start_line,
                                "end_line": chunk.end_line,
                                "language": &chunk.language,
                                "content": &chunk.content,
                                "similarity_score": score,
                            })
                        }).collect();

                        self.traffic
                            .log_system(
                                "vecdb_search",
                                &format!("Found {} relevant code chunks", results.len()),
                                serde_json::json!({
                                    "num_results": results.len(),
                                    "chunks": context_chunks,
                                }),
                            )
                            .await;
                    }
                }
                Err(e) => {
                    // Log error but continue without context
                    self.traffic
                        .log_system(
                            "vecdb_error",
                            &format!("Vector search failed: {}", e),
                            serde_json::json!({}),
                        )
                        .await;
                }
            }
        }

        // Add user message to history (with enriched context if available)
        self.chat_history.push(Message::User {
            content: OneOrMany::one(UserContent::text(&enriched_input)),
        });

        // Store user message in routine memory
        self.store_in_routine_memory("user", user_input, None).await;

        // Log user input
        self.traffic
            .log_request(
                format!("User: {}", truncate_string(user_input, 50)),
                serde_json::json!({
                    "user_input": user_input,
                    "history_length": self.chat_history.len(),
                    "has_vecdb_context": self.vecdb.is_some()
                }),
            )
            .await;

        let mut iterations = 0;


        loop {
            // Check for cancellation before each iteration
            if self.cancel_token.is_cancelled() {
                return Err(anyhow!("Agent execution cancelled by user interrupt"));
            }

            iterations += 1;
            if iterations > self.max_iterations {
                return Err(anyhow!(
                    "Maximum iterations ({}) exceeded. The agent may be stuck in a loop.",
                    self.max_iterations
                ));
            }

            // Log the request to LLM
            let tool_defs = self.get_tool_definitions().await;
            self.traffic
                .log_request(
                    format!("Completion request (iteration {})", iterations),
                    serde_json::json!({
                        "iteration": iterations,
                        "message_count": self.chat_history.len(),
                        "messages": self.serialize_messages(),
                        "tools": tool_defs.iter().map(|t| &t.name).collect::<Vec<_>>()
                    }),
                )
                .await;

            let request_start = Instant::now();

            // Process the response - collect tool calls and text
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut text_response: Option<String> = None;
            let response_choice: OneOrMany<AssistantContent>;
		

            if self.streaming {
                // Streaming mode - print tokens as they arrive
                let mut stream = self
                    .model
                    .completion_request(&self.preamble)
                    .messages(self.chat_history.clone())
                    .tools(tool_defs.clone())
                    .max_tokens(32768)
                    .additional_params(serde_json::json!({
                        "num_ctx": self.context_size
                    }))
                    .stream()
                    .await
                    .map_err(|e| anyhow!("Streaming request failed: {}", e))?;

                let mut streamed_text = String::new();

                // Process stream items
                while let Some(result) = stream.next().await {
                    match result {
                        Ok(content) => match content {
                            StreamedAssistantContent::Text(text) => {
                                // Debug logging to track what text we're receiving
                                self.traffic.log_system(
                                    "stream_text",
                                    &format!("Text chunk: {} chars", text.text.len()),
                                    serde_json::json!({
                                        "text": &text.text,
                                        "contains_xml": text.text.contains('<'),
                                    })
                                ).await;

                                // Emit to TUI or print to terminal
                                if let Some(ref tx) = self.tui_tx {
                                    terminal::emit_assistant_chunk(tx, &self.agent_id, &text.text);
                                } else {
                                    terminal::print_streaming_token(&text.text);
                                }
                                streamed_text.push_str(&text.text);
                            }
                            StreamedAssistantContent::ToolCall(tool_call) => {
                                self.traffic.log_system(
                                    "stream_tool_call",
                                    &format!("Complete tool call: {}", tool_call.function.name),
                                    serde_json::json!({
                                        "id": &tool_call.id,
                                        "name": &tool_call.function.name,
                                    })
                                ).await;
                                tool_calls.push(tool_call);
                            }
                            StreamedAssistantContent::ToolCallDelta { id, delta } => {
                                // Log deltas to understand the streaming pattern
                                self.traffic.log_system(
                                    "stream_tool_delta",
                                    &format!("Tool call delta: {}", id),
                                    serde_json::json!({
                                        "id": id,
                                        "delta": format!("{:?}", delta),
                                    })
                                ).await;
                                // Tool call deltas are accumulated automatically by rig
                            }
                            StreamedAssistantContent::Reasoning(_) => {
                                // Ignore reasoning content
                            }
                            StreamedAssistantContent::Final(_) => {
                                // Final item contains usage info
                            }
                            StreamedAssistantContent::ReasoningDelta { id, reasoning } => {
                                // Ignore reasoning Delta
                            },
                        },
                        Err(e) => {
                            let error_msg = format!("Stream error: {}", e);
                            if let Some(ref tx) = self.tui_tx {
                                terminal::emit_error(tx, &self.agent_id, &error_msg);
                            } else {
                                terminal::print_error(&error_msg);
                            }
                        }
                    }
                }

                // End streaming output
                if !streamed_text.is_empty() {
                    if self.tui_tx.is_none() {
                        terminal::end_streaming();
                    }
                    text_response = Some(streamed_text);
                }

                // Get the final aggregated choice from the stream
                response_choice = stream.choice.clone();

                // Also collect any tool calls from the aggregated response
                for content in response_choice.iter() {
                    if let AssistantContent::ToolCall(tc) = content
                        && !tool_calls.iter().any(|t| t.id == tc.id) {
                            tool_calls.push(tc.clone());
                        }
                }
            } else {
                // Non-streaming mode - show thinking spinner
                let spinner = terminal::create_thinking_spinner();

                let response = self
                    .model
                    .completion_request(&self.preamble)
                    .messages(self.chat_history.clone())
                    .tools(tool_defs.clone())
                    .max_tokens(32768)
                    .additional_params(serde_json::json!({
                        "num_ctx": self.context_size
                    }))
                    .send()
                    .await;

                // Clear spinner before handling result
                terminal::clear_spinner(&spinner);

                let response = response.map_err(|e| anyhow!("Completion request failed: {}", e))?;

                response_choice = response.choice.clone();

                // Iterate over the response choice
                for content in response.choice.iter() {
                    match content {
                        AssistantContent::Text(text) => {
                            text_response = Some(text.text.clone());
                        }
                        AssistantContent::ToolCall(tool_call) => {
                            tool_calls.push(tool_call.clone());
                        }
                        AssistantContent::Reasoning(_) => {
                            // Reasoning content is internal model reasoning, we can ignore it
                        }
                        AssistantContent::Image(image) => {
                            // Ignoring Images
                        },
                    }
                }
            }

            let request_duration = request_start.elapsed().as_millis() as u64;

            // Log the response
            let response_summary = if !tool_calls.is_empty() {
                format!(
                    "Tool calls: {}",
                    tool_calls
                        .iter()
                        .map(|t| t.function.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else if let Some(ref text) = text_response {
                format!("Text: {}", truncate_string(text, 50))
            } else {
                "Empty response".to_string()
            };

            self.traffic
                .log_response(
                    response_summary,
                    serde_json::json!({
                        "tool_calls": tool_calls.iter().map(|t| {
                            serde_json::json!({
                                "id": t.id,
                                "name": t.function.name,
                                "arguments": t.function.arguments
                            })
                        }).collect::<Vec<_>>(),
                        "text_response": text_response,
                        "raw_choice": format!("{:?}", response_choice)
                    }),
                    Some(request_duration),
                )
                .await;

            // If there are tool calls, execute them
            if !tool_calls.is_empty() {
                // Finalize any streaming message before tool execution
                // This ensures the chat log doesn't get fragmented
                if self.streaming && text_response.is_some()
                    && let Some(ref tx) = self.tui_tx
                        && let Some(ref text) = text_response {
                            terminal::emit_assistant_message(tx, &self.agent_id, text);
                        }
                // Add assistant message with tool calls to history
                self.chat_history.push(Message::Assistant {
                    id: None,
                    content: response_choice.clone(),
                });

                // Execute each tool and collect results
                let mut tool_results: Vec<UserContent> = Vec::new();

                for tool_call in &tool_calls {
                    let tool_name = &tool_call.function.name;
                    // Arguments is already a serde_json::Value
                    let tool_args: Value = tool_call.function.arguments.clone();

                    // Emit/print tool execution info
                    let mut args_map = HashMap::new();
                    if let Some(obj) = tool_args.as_object() {
                        for (key, value) in obj {
                            let display_value = if let Some(s) = value.as_str() {
                                if s.len() > 100 {
                                    format!("{}...", &s[..100])
                                } else {
                                    s.to_string()
                                }
                            } else {
                                value.to_string()
                            };
                            args_map.insert(key.clone(), display_value.clone());

                            // For non-TUI mode, still print
                            if self.tui_tx.is_none() {
                                terminal::print_tool_arg(key, &display_value);
                            }
                        }
                    }

                    // Generate diff for file operations
                    let diff = if tool_name == "write_file" || tool_name == "edit_file" {
                        self.generate_diff_for_tool(tool_name, &tool_args).await
                    } else {
                        None
                    };

                    // Check permissions first (for batch mode)
                    if !self.permissions.is_granted(tool_name) {
                        // Tool not granted in batch mode - fail immediately
                        let error_msg = format!(
                            "Permission denied: tool '{}' not granted. Use --grant {} or --grant-all",
                            tool_name, tool_name
                        );
                        if let Some(ref tx) = self.tui_tx {
                            terminal::emit_error(tx, &self.agent_id, &error_msg);
                        } else {
                            terminal::print_error(&error_msg);
                        }
                        return Err(anyhow!(error_msg));
                    }

                    // Request permission to execute the tool (for TUI mode)
                    let has_permission = if self.tui_tx.is_some() && !self.permissions.should_skip_confirmations() {
                        self.request_permission(tool_name, &args_map, diff).await
                    } else {
                        true  // Permission already granted via CLI
                    };

                    // Emit tool start event or print header only if permission granted
                    if has_permission {
                        if let Some(ref tx) = self.tui_tx {
                            terminal::emit_tool_start(tx, &self.agent_id, tool_name, args_map.clone());
                        } else {
                            terminal::print_tool_header(tool_name);
                        }
                    }

                    // If permission was explicitly rejected, stop the completion loop
                    if !has_permission {
                        // User rejected the permission - stop the agent loop and wait for new input
                        return Err(anyhow!("Operation cancelled by user. Please provide new instructions."));
                    }

                    // Check for dangerous commands if this is a bash tool
                    if tool_name == "bash" && self.confirm_dangerous {
                        if let Some(command) = tool_args.get("command").and_then(|c| c.as_str())
                            && let Some(pattern) = terminal::is_dangerous_command(command) {
                                let msg = format!(
                                    "Dangerous command detected ({}): {}",
                                    pattern,
                                    truncate_string(command, 50)
                                );
                                // TODO: Implement modal confirmation for TUI mode
                                if self.tui_tx.is_some() {
                                    // For now, auto-skip in TUI mode
                                    if let Some(ref tx) = self.tui_tx {
                                        terminal::emit_warning(tx, &self.agent_id, &format!("Dangerous command auto-skipped in TUI mode: {}", pattern));
                                    }
                                    return Err(anyhow!("Dangerous command rejected by user. Please provide new instructions."));
                                } else {
                                    match terminal::confirm(&msg) {
                                        Ok(true) => {
                                            // User confirmed, continue
                                        }
                                        Ok(false) => {
                                            terminal::print_warning("Command skipped by user");
                                            return Err(anyhow!("Operation cancelled by user. Please provide new instructions."));
                                        }
                                        Err(_) => {
                                            terminal::print_error("Failed to read confirmation");
                                            return Err(anyhow!("Operation cancelled by user. Please provide new instructions."));
                                        }
                                    }
                                }
                            }
                    } else if tool_name == "write_file" && self.confirm_dangerous {
                        // Check for dangerous paths
                        if let Some(path) = tool_args.get("file_path").and_then(|p| p.as_str())
                            && let Some(pattern) = terminal::is_dangerous_path(path) {
                                let msg = format!("Writing to sensitive path ({}): {}", pattern, path);
                                // TODO: Implement modal confirmation for TUI mode
                                if self.tui_tx.is_some() {
                                    // For now, auto-skip in TUI mode
                                    if let Some(ref tx) = self.tui_tx {
                                        terminal::emit_warning(tx, &self.agent_id, &format!("Dangerous write auto-skipped in TUI mode: {}", pattern));
                                    }
                                    return Err(anyhow!("Dangerous write operation rejected by user. Please provide new instructions."));
                                } else {
                                    match terminal::confirm(&msg) {
                                        Ok(true) => {
                                            // User confirmed, continue
                                        }
                                        Ok(false) => {
                                            terminal::print_warning("Write skipped by user");
                                            return Err(anyhow!("Operation cancelled by user. Please provide new instructions."));
                                        }
                                        Err(_) => {
                                            terminal::print_error("Failed to read confirmation");
                                            return Err(anyhow!("Operation cancelled by user. Please provide new instructions."));
                                        }
                                    }
                                }
                            }
                    }

                    // Execute the tool with timing and spinner
                    let tool_start = Instant::now();

                    // Show spinner for potentially long-running tools (only in non-TUI mode)
                    let spinner = if self.tui_tx.is_none() {
                        Some(terminal::create_tool_spinner(tool_name))
                    } else {
                        None
                    };

                    // Check if we're in dry-run mode
                    let exec_result = if self.permissions.is_dry_run() {
                        // Dry-run: don't actually execute, just return what would happen
                        Ok(format!(
                            "[DRY RUN] Would execute tool '{}' with arguments:\n{}",
                            tool_name,
                            serde_json::to_string_pretty(&tool_args).unwrap_or_else(|_| "{}".to_string())
                        ))
                    } else {
                        self.execute_tool(tool_name, tool_args.clone()).await
                    };
                    let duration_ms = tool_start.elapsed().as_millis();

                    let result = match exec_result {
                        Ok(output) => {
                            let success_msg = format!("{} completed ({}ms, {} chars)", tool_name, duration_ms, output.len());

                            // Emit/print success
                            if let Some(ref tx) = self.tui_tx {
                                terminal::emit_tool_success(tx, &self.agent_id, tool_name, &success_msg);
                            } else if let Some(spinner) = spinner {
                                terminal::finish_spinner_success(&spinner, &success_msg);
                            }

                            // Track file changes for write and edit operations
                            if tool_name == "write_file" {
                                if let Some(path) = tool_args.get("file_path").and_then(|p| p.as_str()) {
                                    // Determine if file was created or modified based on output
                                    let op = if output.contains("Created") {
                                        FileOperation::Created
                                    } else {
                                        FileOperation::Modified
                                    };
                                    self.record_file_change(path, op);
                                }
                            } else if tool_name == "edit_file"
                                && let Some(path) = tool_args.get("file_path").and_then(|p| p.as_str()) {
                                    self.record_file_change(path, FileOperation::Modified);
                                }

                            output
                        }
                        Err(e) => {
                            let error_msg = format!("{} failed: {}", tool_name, e);

                            // Emit/print error
                            if let Some(ref tx) = self.tui_tx {
                                terminal::emit_tool_error(tx, &self.agent_id, tool_name, &error_msg);
                            } else if let Some(spinner) = spinner {
                                terminal::finish_spinner_error(&spinner, &error_msg);
                            }

                            format!("Error: {}", e)
                        }
                    };
                    let tool_duration = tool_start.elapsed().as_millis() as u64;

                    // Log tool execution
                    self.traffic
                        .log_tool(tool_name, &tool_args, &result, tool_duration)
                        .await;

                    // Create tool result
                    let tool_result = ToolResult {
                        id: tool_call.id.clone(),
                        call_id: Some(tool_call.id.clone()),
                        content: OneOrMany::one(ToolResultContent::text(result)),
                    };
                    tool_results.push(UserContent::ToolResult(tool_result));
                }

                // Add tool results to history as user message
                let content = if tool_results.len() == 1 {
                    OneOrMany::one(tool_results.remove(0))
                } else {
                    OneOrMany::many(tool_results).unwrap_or_else(|_| {
                        OneOrMany::one(UserContent::text("No tool results"))
                    })
                };

                self.chat_history.push(Message::User {
                    content,
                });

                // Continue the loop to get the next response
                continue;
            }

            // No tool calls - we have a final text response
            if let Some(text) = text_response {
                // Track token usage (estimated)
                // Estimate prompt from preamble + history
                let prompt_text = format!("{}\n{:?}", self.preamble, self.chat_history);
                //eprintln!("DEBUG agent_loop.rs: Before add_estimated - prompt_tokens: {}, completion_tokens: {}",
                //         self.token_usage.prompt_tokens, self.token_usage.completion_tokens);
                //eprintln!("DEBUG agent_loop.rs: Prompt text length: {}, response text length: {}",
                //         prompt_text.len(), text.len());
                self.token_usage.add_estimated(&prompt_text, &text);
                //eprintln!("DEBUG agent_loop.rs: After add_estimated - prompt_tokens: {}, completion_tokens: {}",
                         //self.token_usage.prompt_tokens, self.token_usage.completion_tokens);

                // Track iteration count
                self.iteration_count = iterations;

                // Add assistant's final response to history
                self.chat_history.push(Message::Assistant {
                    id: None,
                    content: response_choice.clone(),
                });

                // Store assistant message in routine memory
                self.store_in_routine_memory("assistant", &text, None).await;

                //eprintln!("DEBUG chat(): Returning final text response (length: {}, iterations: {})",
                //         text.len(), iterations);
                return Ok(text);
            }

            // Unexpected: no tool calls and no text response
            return Err(anyhow!("Unexpected response: no text or tool calls"));
        }
    }

    /// Clear the conversation history
    pub fn clear_history(&mut self) {
        self.chat_history.clear();
    }

    /// Get the current conversation history length
    pub fn history_len(&self) -> usize {
        self.chat_history.len()
    }

    /// Store a message in routine memory (automatic conversation history)
    async fn store_in_routine_memory(&self, role: &str, content: &str, tool_name: Option<&str>) {
        // Only store if memory manager is available
        if let Some(ref memory_manager) = self.memory_manager {
            // Create routine memory chunk
            let chunk = RoutineMemoryChunk {
                session_id: self.session_id.clone().unwrap_or_else(|| "unknown".to_string()),
                message_id: uuid::Uuid::new_v4().to_string(),
                timestamp: chrono::Utc::now(),
                role: role.to_string(),
                content: content.to_string(),
                working_directory: self.working_directory.clone(),
                model: self.model_name.clone(),
                context_tags: RoutineMemoryChunk::extract_tags(content, tool_name),
            };

            // Store in memory manager (async requires lock)
            let mut mm = memory_manager.lock().await;
            if let Err(e) = mm.store_routine_memory(chunk) {
                // Log error but don't fail the conversation
                eprintln!("Warning: Failed to store routine memory: {}", e);
            }
        }
    }
}

/// Truncate a string for display
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
