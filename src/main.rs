use anyhow::Result;
use clap::Parser;
use rig::client::{CompletionClient, Nothing};
use rig::completion::CompletionModel;
use rig::providers::anthropic::Client;
use rig::providers::ollama;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

mod agent;
mod agent_loop;
mod colors;
mod commands;
mod diff;
mod error;
mod git;
mod inspector;
mod memory;
mod permissions;
mod process_manager;
mod rust_analyzer;
mod session;
mod template;
mod terminal;
mod tools;
mod tree_sitter_chunker;
mod tui;
mod vecdb;

use agent_loop::AgentLoop;
use commands::{CommandRegistry, CommandContext};
use inspector::{InspectorState, TrafficHandle};
use session::SessionManager;
use template::TemplateContext;

/// agent-t - A terminal-based coding agent
#[derive(Parser, Debug)]
#[command(name = "agent-t")]
#[command(about = "A terminal-based coding agent powered by local LLMs")]
struct Args {
    /// Agent name to use (creates if doesn't exist)
    #[arg(long, short = 'a')]
    agent: Option<String>,

    /// List all available agents
    #[arg(long)]
    list_agents: bool,

    /// Enable the traffic inspector web interface
    #[arg(long, short = 'i')]
    inspector: bool,

    /// Port for the traffic inspector (default: 8080)
    #[arg(long, default_value = "8080")]
    inspector_port: u16,

    /// Ollama model to use
    #[arg(long, short = 'm', default_value = "qwen3-coder")]
    model: String,

    /// Resume the most recent session
    #[arg(long, short = 'r')]
    resume: bool,

    /// Load a specific session by ID
    #[arg(long)]
    session: Option<String>,

    /// Disable dangerous command confirmations
    #[arg(long)]
    no_confirm: bool,

    /// Ollama server URL (default: http://localhost:11434)
    #[arg(long, short = 'u')]
    ollama_url: Option<String>,

    /// Enable streaming output
    #[arg(long, short = 's')]
    streaming: bool,

    /// Context window size (num_ctx) for the LLM (default: 8192)
    #[arg(long, short = 'c', default_value = "8192")]
    context_size: usize,

    /// Special instructions to append to system prompt (inline text or path to file starting with @)
    #[arg(long, short = 'I')]
    instructions: Option<String>,

    /// Override the default system prompt (inline text or path to file starting with @)
    #[arg(long, short = 'S')]
    system_prompt: Option<String>,

    /// Enable vector database for code context (indexes code files)
    #[arg(long)]
    vecdb: bool,

    /// Embedding model for code vector database (default: nomic-embed-text)
    #[arg(long, default_value = "nomic-embed-text")]
    vecdb_embedding_model: String,

    /// Force reindex of code files (rebuilds vector database)
    #[arg(long)]
    reindex: bool,

    /// Enable long-term memory for this agent
    #[arg(long)]
    memory: bool,

    /// Disable memory for this session
    #[arg(long)]
    no_memory: bool,

    /// Embedding model for memory (default: BAAI/bge-small-en-v1.5)
    #[arg(long, default_value = "BAAI/bge-small-en-v1.5")]
    memory_embedding_model: String,

    // Batch mode arguments
    /// Batch mode: provide initial prompt via CLI (non-interactive)
    #[arg(short = 'p', long)]
    prompt: Option<String>,

    /// Read prompt from file
    #[arg(long)]
    prompt_file: Option<String>,

    /// Grant tool permissions (comma-separated: read_file,bash,write_file)
    #[arg(short = 'g', long, value_delimiter = ',')]
    grant: Vec<String>,

    /// Grant all tool permissions (use with caution)
    #[arg(long)]
    grant_all: bool,

    /// Disable all confirmation prompts (implies --grant-all)
    #[arg(short = 'y', long)]
    yes: bool,

    /// Maximum iterations for the agent loop (default: 100)
    #[arg(long)]
    max_iterations: Option<usize>,

    /// Batch mode timeout in seconds (default: 300)
    #[arg(long, default_value = "300")]
    batch_timeout: u64,

    /// Dry-run mode: show what would be executed without actually doing it
    #[arg(long)]
    dry_run: bool,

    /// Quiet mode: only output final response (for batch mode)
    #[arg(short = 'q', long)]
    quiet: bool,
}

// System prompt loaded from external file at compile time
const SYSTEM_PROMPT: &str = include_str!("../prompts/system.txt");


/// Load special instructions from either inline text or a file path
/// If the input starts with '@', treat it as a file path, otherwise treat as inline text
fn load_instructions(instructions: &str) -> Result<String> {
    if let Some(path) = instructions.strip_prefix('@') {
        // Load from file
        std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read instructions file '{}': {}", path, e))
    } else {
        // Use inline text
        Ok(instructions.to_string())
    }
}

/// Build GrantedPermissions from CLI arguments
fn build_permissions(args: &Args) -> permissions::GrantedPermissions {
    let grant_all = args.grant_all || args.yes;
    let mut granted_tools = args.grant.clone();

    // Expand tool categories (e.g., "read-only" -> ["read_file", "grep", ...])
    granted_tools = permissions::expand_tool_categories(granted_tools);

    permissions::GrantedPermissions::new(
        granted_tools,
        grant_all,
        args.yes,
        args.dry_run,
    )
}

/// Get the initial prompt for batch mode (from --prompt or --prompt-file)
fn get_initial_prompt(args: &Args) -> Result<Option<String>> {
    if let Some(ref prompt) = args.prompt {
        Ok(Some(prompt.clone()))
    } else if let Some(ref prompt_file) = args.prompt_file {
        let content = std::fs::read_to_string(prompt_file)
            .map_err(|e| anyhow::anyhow!("Failed to read prompt file '{}': {}", prompt_file, e))?;
        Ok(Some(content))
    } else {
        Ok(None)
    }
}

/// Run agent in batch mode (non-interactive)
async fn run_batch_mode<M: rig::completion::CompletionModel>(
    prompt: String,
    model: M,
    system_prompt: String,
    permissions: permissions::GrantedPermissions,
    args: &Args,
    cwd: String,
    vecdb: Option<Arc<tokio::sync::Mutex<vecdb::VectorDB>>>,
    memory_manager: Option<Arc<tokio::sync::Mutex<memory::MemoryManager>>>,
    traffic: TrafficHandle,
) -> Result<()> {
    use tokio::time::{timeout, Duration};

    if !args.quiet {
        eprintln!("Running in batch mode...");
        eprintln!("Permissions: {}", permissions.summary());
        if permissions.is_dry_run() {
            eprintln!("DRY RUN MODE: No tools will actually execute");
        }
        eprintln!();
    }

    // Create cancellation token (won't be used in batch mode but required)
    let cancel_token = CancellationToken::new();

    // Clone memory_manager before moving it to agent so we can use it later
    let memory_manager_cleanup = memory_manager.clone();

    // Create agent
    let mut agent = AgentLoop::new(
        model,
        system_prompt,
        traffic,
        !args.no_confirm,
        false,  // No streaming in batch mode
        cwd,
        args.context_size,
        vecdb,
        memory_manager,
        None,  // No session ID in batch mode
        0,     // Depth 0 (main agent)
        cancel_token,
        permissions,
        args.model.clone(),  // Model name
    );

    // Set max iterations if specified
    if let Some(max_iter) = args.max_iterations {
        agent.set_max_iterations(max_iter);
    }

    // Run with timeout
    let timeout_duration = Duration::from_secs(args.batch_timeout);
    let result = timeout(timeout_duration, agent.chat(&prompt)).await;

    match result {
        Ok(Ok(response)) => {
            // Success - print the final response
            if args.quiet {
                // Quiet mode: only output the response
                println!("{}", response);
            } else {
                eprintln!("\n=== Agent Response ===");
                println!("{}", response);
                eprintln!("\n=== Summary ===");
                eprintln!("Files changed: {}", agent.file_changes_count());
                eprintln!("Iterations: {}", agent.iteration_count());
                let usage = agent.get_token_usage();
                eprintln!("Token usage: {} prompt, {} completion", usage.prompt_tokens, usage.completion_tokens);
            }
            // Flush memory before exit
            if let Some(ref mm) = memory_manager_cleanup {
                let manager = mm.lock().await;
                let _ = manager.flush();
            }
            Ok(())
        }
        Ok(Err(e)) => {
            if !args.quiet {
                eprintln!("Error: {}", e);
            }
            // Flush memory before exit
            if let Some(ref mm) = memory_manager_cleanup {
                let manager = mm.lock().await;
                let _ = manager.flush();
            }
            std::process::exit(1);
        }
        Err(_) => {
            if !args.quiet {
                eprintln!("Error: Batch mode timed out after {} seconds", args.batch_timeout);
            }
            // Flush memory before exit
            if let Some(ref mm) = memory_manager_cleanup {
                let manager = mm.lock().await;
                let _ = manager.flush();
            }
            std::process::exit(3);
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_max_level(tracing_subscriber::filter::LevelFilter::WARN)
        .init();

    // Handle --list-agents
    if args.list_agents {
        let agent_manager = agent::AgentManager::new()?;
        let agents = agent_manager.list_agents()?;

        if agents.is_empty() {
            terminal::print_info("No agents created yet. Use --agent <name> to create one.");
        } else {
            println!("\nAvailable agents:\n");
            for agent_info in agents {
                println!("  {}", agent_info);
            }
        }
        return Ok(());
    }

    // Determine which agent to use
    let agent_manager = agent::AgentManager::new()?;
    let agent_name = if let Some(ref name) = args.agent {
        name.clone()
    } else {
        // No agent specified - check if there's only one agent
        let agents = agent_manager.list_agents()?;

        if agents.is_empty() {
            terminal::print_error("No agents available. Create one with --agent <name>");
            return Ok(());
        } else if agents.len() == 1 {
            // Auto-select the only agent
            let agent = &agents[0];
            terminal::print_info(&format!("Auto-selecting agent '{}'", agent.name));
            agent.name.clone()
        } else {
            // Multiple agents - ask user to specify
            println!("\nMultiple agents available:");
            for agent_info in &agents {
                println!("  {}", agent_info);
            }
            terminal::print_error("\nPlease specify an agent with --agent <name>");
            return Ok(());
        }
    };

    // Validate and load/create agent
    agent::AgentManager::validate_name(&agent_name)?;

    let agent_config = if agent_manager.exists(&agent_name) {
        // Load existing agent
        let config = agent_manager.load_agent(&agent_name)?;
        terminal::print_success(&format!("Loaded agent '{}'", agent_name));
        if let Some(desc) = &config.description {
            terminal::print_info(&format!("  {}", desc));
        }
        config
    } else {
        // Agent doesn't exist - prompt to create
        terminal::print_warning(&format!("Agent '{}' does not exist.", agent_name));
        print!("Create new agent? (y/n): ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut response = String::new();
        std::io::stdin().read_line(&mut response)?;

        if response.trim().to_lowercase() == "y" {
            agent_manager.create_agent_interactive(&agent_name)?
        } else {
            terminal::print_info("Agent creation cancelled.");
            return Ok(());
        }
    };

    // Update last active
    agent_manager.update_last_active(&agent_name)?;

    // Initialize memory if enabled
    let memory_enabled = agent_config.memory_enabled && !args.no_memory || args.memory;

    let (_memory_manager, last_session_summary) = if memory_enabled {
        terminal::print_info("Initializing long-term memory...");
        let mut manager = memory::MemoryManager::new(
            &agent_name,
            &args.memory_embedding_model
        )?;
        match manager.load_or_initialize().await {
            Ok(_) => {
                let stats = manager.stats();
                terminal::print_success(&format!(
                    "Memory loaded: {} routine, {} key memories",
                    stats.routine_count,
                    stats.key_count
                ));

                // Get the last session summary for continuity
                let last_summary = manager.get_last_session_summary();
                if let Some(ref summary) = last_summary {
                    terminal::print_info(&format!(
                        "Found previous session summary from {}",
                        summary.timestamp.format("%Y-%m-%d %H:%M")
                    ));
                }

                (Some(Arc::new(tokio::sync::Mutex::new(manager))), last_summary)
            }
            Err(e) => {
                terminal::print_error(&format!("Failed to initialize memory: {}", e));
                (None, None)
            }
        }
    } else {
        (None, None)
    };

    // Setup Ctrl-C handler for clean shutdown
    if let Some(ref memory_manager) = _memory_manager {
        let memory_clone = Arc::clone(memory_manager);
        tokio::spawn(async move {
            match tokio::signal::ctrl_c().await {
                Ok(()) => {
                    eprintln!("\n[INFO] Ctrl-C received, flushing memory...");
                    let mm = memory_clone.lock().await;
                    if let Err(e) = mm.flush() {
                        eprintln!("[ERROR] Failed to flush memory on Ctrl-C: {}", e);
                    }
                    std::process::exit(0);
                }
                Err(err) => {
                    eprintln!("[WARN] Unable to listen for Ctrl-C signal: {}", err);
                }
            }
        });
    }

    // Setup traffic inspector if enabled
    let (traffic_handle, inspector_state) = if args.inspector {
        let state = InspectorState::new();
        let handle = TrafficHandle::new(Some(Arc::clone(&state)));
        (handle, Some(state))
    } else {
        (TrafficHandle::disabled(), None)
    };

    // Start inspector web server if enabled
    if let Some(state) = inspector_state {
        let port = args.inspector_port;
        tokio::spawn(async move {
            if let Err(e) = inspector::start_server(state, port).await {
                terminal::print_error(&format!("Inspector server error: {}", e));
            }
        });
    }

    // Setup session manager (wrapped in Arc<Mutex> for sharing with agent task)
    let session_manager = Arc::new(tokio::sync::Mutex::new(SessionManager::new()?));

    // Get current working directory
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    // Detect and initialize rust-analyzer if this is a Rust project
    let is_rust_project = std::path::Path::new(&cwd).join("Cargo.toml").exists();
    if is_rust_project {
        terminal::print_info("Rust project detected. Initializing rust-analyzer...");
        match rust_analyzer::RustAnalyzerClient::new(std::path::PathBuf::from(&cwd)).await {
            Ok(client) => {
                tools::ra_common::set_client(client).await;
                terminal::print_success("rust-analyzer initialized successfully");
            }
            Err(e) => {
                terminal::print_warning(&format!("Failed to initialize rust-analyzer: {}. Rust-specific features will be unavailable.", e));
            }
        }
    }

    // Initialize vector database if enabled
    let vecdb = if args.vecdb {
        terminal::print_info("Initializing vector database...");
        match vecdb::VectorDB::new(args.ollama_url.as_deref(), &args.vecdb_embedding_model) {
            Ok(mut db) => {
                // Check if we need to index or reindex
                if args.reindex || !db.index_exists() {
                    terminal::print_info("Indexing code files... This may take a few minutes.");
                    match db.index_directory(&cwd).await {
                        Ok(num_chunks) => {
                            terminal::print_success(&format!("Indexed {} code chunks", num_chunks));
                            Some(Arc::new(tokio::sync::Mutex::new(db)))
                        }
                        Err(e) => {
                            terminal::print_error(&format!("Failed to index code files: {}", e));
                            None
                        }
                    }
                } else {
                    // Load existing index
                    match db.load_index().await {
                        Ok(_) => {
                            let stats = db.stats();
                            terminal::print_success(&format!("Loaded vector database with {} chunks", stats.get("chunks").unwrap_or(&"0".to_string())));
                            Some(Arc::new(tokio::sync::Mutex::new(db)))
                        }
                        Err(e) => {
                            terminal::print_error(&format!("Failed to load vector database: {}", e));
                            None
                        }
                    }
                }
            }
            Err(e) => {
                terminal::print_error(&format!("Failed to initialize vector database: {}", e));
                None
            }
        }
    } else {
        None
    };

    // Handle session loading/creation
    {
        let mut sm = session_manager.lock().await;
        if let Some(ref session_id) = args.session {
            // Load specific session
            match sm.load_session(session_id) {
                Ok(session) => {
                    terminal::print_success(&format!(
                        "Loaded session {} ({} messages)",
                        &session.id[..8],
                        session.message_count()
                    ));
                }
                Err(e) => {
                    terminal::print_error(&format!("Failed to load session: {}", e));
                    sm.start_new_session(&args.model, &cwd);
                }
            }
        } else if args.resume {
            // Resume most recent session
            match sm.get_most_recent_session()? {
                Some(session) => {
                    terminal::print_success(&format!(
                        "Resumed session {} ({} messages)",
                        &session.id[..8],
                        session.messages.len()
                    ));
                    sm.load_session(&session.id)?;
                }
                None => {
                    terminal::print_info("No previous session found. Starting new session.");
                    sm.start_new_session(&args.model, &cwd);
                }
            }
        } else {
            // Start new session
            sm.start_new_session(&args.model, &cwd);
        }
    }

    // Create Ollama client
    let ollama_client = if let Some(ref url) = args.ollama_url {
        terminal::print_info(&format!("Using Ollama at: {}", url));
       
       ollama::Client::builder()
            .api_key(Nothing)
            .base_url(&url)
            .build()
            .unwrap()

    } else {
        // Use default localhost:11434
        ollama::Client::new(Nothing).unwrap()
    };

    use rig::providers::*;

    // Create a completion model
    //let completion_model_type = open
    let model: ollama::CompletionModel<reqwest::Client> = ollama_client.completion_model(&args.model);

    // Create channels for TUI <-> Agent communication
    let (tui_tx, tui_rx) = tokio::sync::mpsc::channel::<tui::TuiEvent>(100);
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<String>(100);

    // Load system prompt (custom or default)
    let base_prompt = if let Some(ref custom_prompt) = args.system_prompt {
        match load_instructions(custom_prompt) {
            Ok(prompt) => {
                terminal::print_info("Using custom system prompt");
                prompt
            }
            Err(e) => {
                terminal::print_error(&format!("Failed to load custom system prompt: {}", e));
                return Err(e);
            }
        }
    } else {
        SYSTEM_PROMPT.to_string()
    };

    // Try to load agent-specific system prompt file
    let agent_file_prompt = match agent::load_agent_system_prompt(&agent_manager, &agent_name) {
        Ok(Some(content)) => {
            terminal::print_info(&format!(
                "Loaded agent-specific system prompt from {}/.agent-t/agents/{}/system_prompt.md",
                dirs::home_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "~".to_string()),
                agent_name
            ));
            Some(content)
        }
        Ok(None) => None,
        Err(e) => {
            terminal::print_error(&format!("Error loading agent system prompt: {}", e));
            None  // Continue with agent.json fallback
        }
    };

    // Build system prompt with agent personality (file takes precedence over agent.json)
    let prompt_with_agent = agent::build_system_prompt(
        &agent_config,
        &base_prompt,
        agent_file_prompt.as_deref()
    );

    // Render system prompt with template variables
    let template_ctx = TemplateContext::new(&cwd, &args.model, &agent_name);
    let mut rendered_prompt = template_ctx.render(&prompt_with_agent);

    // Append special instructions if provided
    if let Some(ref instructions_input) = args.instructions {
        match load_instructions(instructions_input) {
            Ok(instructions) => {
                rendered_prompt.push_str("\n\n");
                rendered_prompt.push_str(&instructions);
                terminal::print_info("Special instructions added to system prompt");
            }
            Err(e) => {
                terminal::print_error(&format!("Failed to load instructions: {}", e));
                return Err(e);
            }
        }
    }

    // Append last session summary if available (for continuity)
    if let Some(ref summary) = last_session_summary {
        rendered_prompt.push_str("\n\n## Previous Session Context\n\n");
        rendered_prompt.push_str(&format!(
            "Your last session ended on {}. Here's what you were working on:\n\n{}",
            summary.timestamp.format("%Y-%m-%d %H:%M"),
            summary.content
        ));
        if !summary.related_files.is_empty() {
            rendered_prompt.push_str(&format!(
                "\n\nRelated files: {}",
                summary.related_files.join(", ")
            ));
        }
        rendered_prompt.push_str("\n\nUse this context to continue where you left off. You can search for more details using search_routine_memory or search_key_memory tools.");
    }

    // Check for batch mode
    if let Some(prompt) = get_initial_prompt(&args)? {
        // BATCH MODE - run non-interactively and exit
        let permissions = build_permissions(&args);
        return run_batch_mode(
            prompt,
            model,
            rendered_prompt,
            permissions,
            &args,
            cwd,
            vecdb,
            _memory_manager,
            traffic_handle,
        )
        .await;
    }

    // INTERACTIVE MODE (TUI)
    // For interactive mode, allow all tools (permissions handled via TUI prompts)
    let permissions = permissions::GrantedPermissions::allow_all();

    // Create cancellation token for interrupt handling
    let cancel_token = CancellationToken::new();

    // Create the agentic loop with traffic handle and confirmation setting
    // Clone rendered_prompt before moving it so we can recreate the agent later
    let rendered_prompt_agent = rendered_prompt.clone();

    // Get session ID from session manager
    let session_id = {
        let mgr = session_manager.lock().await;
        mgr.current_session().map(|s| s.id.clone())
    };

    let mut agent = AgentLoop::new(
        model,
        rendered_prompt,
        traffic_handle.clone(),
        !args.no_confirm,
        args.streaming,
        cwd.clone(),
        args.context_size,
        vecdb.clone(),
        _memory_manager.clone(),
        session_id.clone(),
        0,  // Initial depth is 0 (main agent)
        cancel_token.clone(),
        permissions.clone(),  // Use permissions from CLI (allow_all for interactive mode)
        args.model.clone(),  // Model name
    );

    // Set TUI event sender on agent
    agent.set_tui_sender(tui_tx.clone());

    // Get session info for TUI
    let session_id = {
        let sm = session_manager.lock().await;
        sm.current_session()
            .map(|s| s.id.clone())
            .unwrap_or_else(|| "unknown".to_string())
    };

    // Log startup
    let git_info = git::GitInfo::detect(&cwd);
    traffic_handle
        .log_system(
            "startup",
            "Agent started",
            serde_json::json!({
                "model": args.model,
                "working_directory": cwd,
                "git_branch": git_info.branch,
                "git_dirty": git_info.is_dirty,
                "session_id": &session_id,
            }),
        )
        .await;

    // Send initial session info to TUI
    let _ = tui_tx.try_send(tui::TuiEvent::SessionUpdate {
        id: session_id.clone(),
        model: args.model.clone(),
    });

    // Send session list to TUI for autocomplete
    {
        let sm = session_manager.lock().await;
        if let Ok(sessions) = sm.list_sessions() {
            let session_ids: Vec<String> = sessions.iter()
                .map(|s| {
                    let short_id = if s.id.len() > 8 { &s.id[..8] } else { &s.id };
                    short_id.to_string()
                })
                .collect();
            let _ = tui_tx.try_send(tui::TuiEvent::SessionListUpdate(session_ids));
        }
    }

    // Create command registry
    let command_registry = CommandRegistry::new();
    let session_manager_clone = Arc::clone(&session_manager);
    let model_clone = args.model.clone();
    let cwd_clone = cwd.clone();

    // Spawn agent task to handle user inputs
    let mut cancel_token_agent = cancel_token.clone();
    let ollama_client_agent = ollama_client.clone();
    let model_name_agent = args.model.clone();
    let traffic_handle_agent = traffic_handle.clone();
    let no_confirm_agent = args.no_confirm;
    let streaming_agent = args.streaming;
    let context_size_agent = args.context_size;
    let vecdb_agent = vecdb.clone();
    let memory_manager_agent = _memory_manager.clone();
    let session_id_agent = session_id.clone();
    let permissions_agent = permissions.clone();

    let agent_task = tokio::spawn(async move {
        while let Some(user_input) = input_rx.recv().await {
            // Check for interrupt signal
            if user_input == "\x1b[INTERRUPT]" {
                // Trigger cancellation
                cancel_token_agent.cancel();
                // Send interrupt event to TUI
                let _ = tui_tx.try_send(tui::TuiEvent::Interrupt);

                // Recreate the agent with a new cancellation token
                let new_cancel_token = CancellationToken::new();
                let new_model = ollama_client_agent.completion_model(&model_name_agent);
                agent = AgentLoop::new(
                    new_model,
                    rendered_prompt_agent.clone(),
                    traffic_handle_agent.clone(),
                    !no_confirm_agent,
                    streaming_agent,
                    cwd_clone.clone(),
                    context_size_agent,
                    vecdb_agent.clone(),
                    memory_manager_agent.clone(),
                    Some(session_id_agent.clone()),
                    0,
                    new_cancel_token.clone(),
                    permissions_agent.clone(),
                    model_name_agent.clone(),  // Model name
                );
                agent.set_tui_sender(tui_tx.clone());
                cancel_token_agent = new_cancel_token;
                continue;
            }

            // Skip empty input
            if user_input.trim().is_empty() {
                continue;
            }

            // Check if it's a shell command (starts with !)
            if user_input.trim().starts_with('!') {
                let shell_command = user_input.trim()[1..].trim();

                if shell_command.is_empty() {
                    let _ = tui_tx.try_send(tui::TuiEvent::Error {
                        agent_id: "main".to_string(),
                        text: "Empty shell command".to_string(),
                    });
                    continue;
                }

                // Execute shell command directly
                use tokio::process::Command;
                use std::process::Stdio;
                use tokio::time::{timeout, Duration};

                let mut cmd = Command::new("bash");
                cmd.arg("-c").arg(shell_command);
                cmd.current_dir(&cwd_clone);
                cmd.stdout(Stdio::piped());
                cmd.stderr(Stdio::piped());

                // Send info about command execution
                let _ = tui_tx.try_send(tui::TuiEvent::Info {
                    agent_id: "main".to_string(),
                    text: format!("$ {}", shell_command),
                });

                match timeout(Duration::from_secs(600), cmd.output()).await {
                    Ok(Ok(output)) => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);

                        let mut result = String::new();

                        if !stdout.is_empty() {
                            result.push_str(&stdout);
                        }

                        if !stderr.is_empty() {
                            if !result.is_empty() {
                                result.push_str("\n--- stderr ---\n");
                            }
                            result.push_str(&stderr);
                        }

                        if result.is_empty() {
                            result = "(no output)".to_string();
                        }

                        // Add exit code info if non-zero
                        if !output.status.success() {
                            let exit_code = output.status.code().unwrap_or(-1);
                            result.push_str(&format!("\n[Exit code: {}]", exit_code));

                            let _ = tui_tx.try_send(tui::TuiEvent::Warning {
                                agent_id: "main".to_string(),
                                text: result,
                            });
                        } else {
                            let _ = tui_tx.try_send(tui::TuiEvent::Info {
                                agent_id: "main".to_string(),
                                text: result,
                            });
                        }
                    }
                    Ok(Err(e)) => {
                        let _ = tui_tx.try_send(tui::TuiEvent::Error {
                            agent_id: "main".to_string(),
                            text: format!("Failed to execute command: {}", e),
                        });
                    }
                    Err(_) => {
                        let _ = tui_tx.try_send(tui::TuiEvent::Error {
                            agent_id: "main".to_string(),
                            text: "Command timed out (600s)".to_string(),
                        });
                    }
                }

                continue;
            }

            // Check if it's a command
            if CommandRegistry::is_command(&user_input) {
                // Execute command
                let mut sm = session_manager_clone.lock().await;
                let mut context = CommandContext {
                    session_manager: &mut sm,
                    tui_tx: &tui_tx,
                    cwd: &cwd_clone,
                    model: &model_clone,
                };

                match command_registry.execute(&user_input, &mut context) {
                    Ok(result) => {
                        use commands::CommandResult;
                        match result {
                            CommandResult::Exit => {
                                let _ = tui_tx.try_send(tui::TuiEvent::Quit);
                                break;
                            }
                            CommandResult::ClearHistory => {
                                agent.clear_history();
                                let _ = tui_tx.try_send(tui::TuiEvent::Clear);
                            }
                            CommandResult::ShowFileChanges => {
                                let changes = agent.get_file_changes_summary();
                                let msg = if changes.is_empty() {
                                    "No files have been modified during this session.".to_string()
                                } else {
                                    let mut output = format!("{} file(s) modified during this session:\n\n", changes.len());
                                    for change in changes {
                                        let symbol = match change.operation {
                                            agent_loop::FileOperation::Created => "+",
                                            agent_loop::FileOperation::Modified => "~",
                                            agent_loop::FileOperation::Deleted => "-",
                                        };
                                        output.push_str(&format!("  {} {}\n", symbol, change.path));
                                    }
                                    output
                                };
                                let _ = tui_tx.try_send(tui::TuiEvent::Info {
                                    agent_id: "main".to_string(),
                                    text: msg,
                                });
                            }
                            CommandResult::Info(msg) => {
                                let _ = tui_tx.try_send(tui::TuiEvent::Info {
                                    agent_id: "main".to_string(),
                                    text: msg,
                                });
                            }
                            CommandResult::Warning(msg) => {
                                let _ = tui_tx.try_send(tui::TuiEvent::Warning {
                                    agent_id: "main".to_string(),
                                    text: msg,
                                });
                            }
                            CommandResult::Error(msg) => {
                                let _ = tui_tx.try_send(tui::TuiEvent::Error {
                                    agent_id: "main".to_string(),
                                    text: msg,
                                });
                            }
                            CommandResult::Continue => {
                                // Do nothing, just continue
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tui_tx.try_send(tui::TuiEvent::Error {
                            agent_id: "main".to_string(),
                            text: format!("Command error: {}", e),
                        });
                    }
                }
                continue;
            }

            // Add user message to TUI (main agent)
            let _ = tui_tx.try_send(tui::TuiEvent::UserMessage {
                agent_id: "main".to_string(),
                text: user_input.clone(),
            });

            // Run the agentic loop
            match agent.chat(&user_input).await {
                Ok(response) => {
                    // Always send the final complete message to finalize streaming
                    // The TUI will replace any streaming message with the final one
                    let _ = tui_tx.try_send(tui::TuiEvent::AssistantMessage {
                        agent_id: "main".to_string(),
                        text: response,
                    });

                    // Update token usage
                    let usage = agent.get_token_usage();
                    //eprintln!("DEBUG main.rs: Agent token usage - prompt: {}, completion: {}",
                    //         usage.prompt_tokens, usage.completion_tokens);
                    let _ = tui_tx.try_send(tui::TuiEvent::TokenUsage {
                        agent_id: "main".to_string(),
                        prompt: usage.prompt_tokens,
                        completion: usage.completion_tokens,
                    });
                    eprintln!("DEBUG main.rs: TokenUsage event sent");
                }
                Err(e) => {
                    let _ = tui_tx.try_send(tui::TuiEvent::Error {
                        agent_id: "main".to_string(),
                        text: e.to_string(),
                    });
                }
            }
        }
    });

    // Run TUI (this blocks until user quits)
    let tui_result = tui::run(
        session_id,
        args.model.clone(),
        agent_name.clone(),
        cwd.clone(),
        tui_rx,
        input_tx,
    ).await;

    // Wait for agent task to complete
    let _ = agent_task.await;

    // Handle any TUI errors
    if let Err(e) = tui_result {
        eprintln!("TUI error: {}", e);
    }

    // Flush memory to disk before exit (if memory is enabled)
    if let Some(ref memory_manager) = _memory_manager {
        let mm = memory_manager.lock().await;
        if let Err(e) = mm.flush() {
            eprintln!("[WARN] Failed to flush memory on exit: {}", e);
        }
    }

    // Save session on exit
    {
        let sm = session_manager.lock().await;
        if let Err(e) = sm.save_current_session() {
            eprintln!("Failed to save session: {}", e);
        }
    }

    // Log shutdown
    traffic_handle
        .log_system("shutdown", "Agent shutting down", serde_json::json!({}))
        .await;

    Ok(())
}
