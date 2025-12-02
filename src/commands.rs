use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use crate::tui::TuiEvent;
use crate::session::SessionManager;
use crate::git::GitInfo;

/// Result of executing a command
#[derive(Debug, Clone)]
pub enum CommandResult {
    /// Continue normal operation
    Continue,
    /// Exit the application
    Exit,
    /// Clear chat history
    ClearHistory,
    /// Show file changes summary
    ShowFileChanges,
    /// Display informational message to user
    Info(String),
    /// Display warning message to user
    Warning(String),
    /// Display error message to user
    Error(String),
}

/// Context provided to commands during execution
pub struct CommandContext<'a> {
    pub session_manager: &'a mut SessionManager,
    pub tui_tx: &'a Sender<TuiEvent>,
    pub cwd: &'a str,
    pub model: &'a str,
}

/// Trait that all commands must implement
pub trait Command: Send + Sync {
    /// The primary name of the command (e.g., "help", "exit")
    fn name(&self) -> &str;

    /// Alternative names for the command (e.g., "quit" for "exit")
    fn aliases(&self) -> Vec<&str> {
        vec![]
    }

    /// Short description of what the command does
    fn description(&self) -> &str;

    /// Detailed help text for the command
    fn help(&self) -> String {
        self.description().to_string()
    }

    /// Execute the command with the given arguments
    /// Returns a CommandResult indicating what should happen next
    fn execute(&self, context: &mut CommandContext, args: Vec<&str>) -> Result<CommandResult>;

    /// Get autocomplete suggestions for this command
    /// Args: current argument values being typed
    /// Returns: list of possible completions for the current argument
    fn autocomplete(&self, _context: &CommandContext, _args: Vec<&str>) -> Vec<String> {
        vec![]
    }
}

/// Registry of all available commands
pub struct CommandRegistry {
    commands: HashMap<String, Arc<dyn Command>>,
}

impl CommandRegistry {
    /// Create a new command registry with all built-in commands
    pub fn new() -> Self {
        let mut registry = Self {
            commands: HashMap::new(),
        };

        // Register built-in commands
        registry.register(Arc::new(HelpCommand));
        registry.register(Arc::new(ExitCommand));
        registry.register(Arc::new(ClearCommand));
        registry.register(Arc::new(SessionsCommand));
        registry.register(Arc::new(SaveCommand));
        registry.register(Arc::new(LoadCommand));
        registry.register(Arc::new(GitCommand));
        registry.register(Arc::new(ChangesCommand));

        registry
    }

    /// Register a command
    pub fn register(&mut self, command: Arc<dyn Command>) {
        let name = command.name().to_string();
        self.commands.insert(name.clone(), command.clone());

        // Also register aliases
        for alias in command.aliases() {
            self.commands.insert(alias.to_string(), command.clone());
        }
    }

    /// Get a command by name or alias
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Command>> {
        self.commands.get(name)
    }

    /// Get all commands (without duplicates from aliases)
    pub fn all_commands(&self) -> Vec<&Arc<dyn Command>> {
        let mut commands: Vec<_> = self.commands.values().collect();
        commands.sort_by_key(|c| c.name());
        commands.dedup_by_key(|c| c.name());
        commands
    }

    /// Check if input is a command (starts with /)
    pub fn is_command(input: &str) -> bool {
        input.trim().starts_with('/')
    }

    /// Parse and execute a command
    pub fn execute(&self, input: &str, context: &mut CommandContext) -> Result<CommandResult> {
        let input = input.trim();

        // Remove leading slash
        let input = if let Some(stripped) = input.strip_prefix('/') {
            stripped
        } else {
            return Ok(CommandResult::Error("Commands must start with '/'".to_string()));
        };

        // Split into command and arguments
        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(CommandResult::Error("Empty command".to_string()));
        }

        let command_name = parts[0];
        let args = parts[1..].to_vec();

        // Find and execute command
        match self.get(command_name) {
            Some(command) => command.execute(context, args),
            None => Ok(CommandResult::Error(format!(
                "Unknown command: '{}'. Type /help for available commands.",
                command_name
            ))),
        }
    }

    /// Get autocomplete suggestions for the current input
    /// Returns a list of possible completions
    pub fn get_autocomplete_suggestions(&self, input: &str, context: &CommandContext) -> Vec<String> {
        let input = input.trim();

        // Must start with /
        if !input.starts_with('/') {
            return vec![];
        }

        // Remove leading slash
        let input = &input[1..];

        // Split into parts
        let parts: Vec<&str> = input.split_whitespace().collect();

        if parts.is_empty() || (parts.len() == 1 && !input.ends_with(' ')) {
            // Autocomplete command name
            let prefix = if parts.is_empty() { "" } else { parts[0] };
            let mut suggestions: Vec<String> = self.all_commands()
                .iter()
                .filter(|cmd| cmd.name().starts_with(prefix))
                .map(|cmd| format!("/{}", cmd.name()))
                .collect();

            // Also check aliases
            for (name, cmd) in &self.commands {
                if name.starts_with(prefix) && name != cmd.name() {
                    suggestions.push(format!("/{}", name));
                }
            }

            suggestions.sort();
            suggestions.dedup();
            suggestions
        } else {
            // Autocomplete command arguments
            let command_name = parts[0];
            let args = parts[1..].to_vec();

            match self.get(command_name) {
                Some(command) => {
                    let completions = command.autocomplete(context, args);
                    completions.into_iter()
                        .map(|arg| format!("/{} {}", command_name, arg))
                        .collect()
                }
                None => vec![],
            }
        }
    }
}

// ===== Built-in Commands =====

/// Display help information
struct HelpCommand;

impl Command for HelpCommand {
    fn name(&self) -> &str {
        "help"
    }

    fn description(&self) -> &str {
        "Display this help message"
    }

    fn execute(&self, _context: &mut CommandContext, args: Vec<&str>) -> Result<CommandResult> {
        if args.is_empty() {
            // Show all commands
            let registry = CommandRegistry::new();
            let mut help_text = String::from("Available commands:\n\n");

            for command in registry.all_commands() {
                let aliases = command.aliases();
                let alias_text = if aliases.is_empty() {
                    String::new()
                } else {
                    format!(" (aliases: {})", aliases.join(", "))
                };

                help_text.push_str(&format!(
                    "  /{}{}\n      {}\n\n",
                    command.name(),
                    alias_text,
                    command.description()
                ));
            }

            help_text.push_str("\nTip: Type /help <command> for detailed help on a specific command.");

            Ok(CommandResult::Info(help_text))
        } else {
            // Show help for specific command
            let command_name = args[0];
            let registry = CommandRegistry::new();

            match registry.get(command_name) {
                Some(command) => {
                    let aliases = command.aliases();
                    let alias_text = if aliases.is_empty() {
                        String::new()
                    } else {
                        format!("\nAliases: {}", aliases.join(", "))
                    };

                    let help_text = format!(
                        "/{}{}\n\n{}",
                        command.name(),
                        alias_text,
                        command.help()
                    );

                    Ok(CommandResult::Info(help_text))
                }
                None => Ok(CommandResult::Error(format!("Unknown command: '{}'", command_name))),
            }
        }
    }

    fn autocomplete(&self, _context: &CommandContext, args: Vec<&str>) -> Vec<String> {
        // Suggest command names
        let registry = CommandRegistry::new();

        if args.is_empty() {
            // Return all command names
            registry.all_commands()
                .iter()
                .map(|cmd| cmd.name().to_string())
                .collect()
        } else {
            // Filter by prefix
            let prefix = args[0];
            registry.all_commands()
                .iter()
                .filter(|cmd| cmd.name().starts_with(prefix))
                .map(|cmd| cmd.name().to_string())
                .collect()
        }
    }
}

/// Exit the application
struct ExitCommand;

impl Command for ExitCommand {
    fn name(&self) -> &str {
        "exit"
    }

    fn aliases(&self) -> Vec<&str> {
        vec!["quit", "q"]
    }

    fn description(&self) -> &str {
        "Exit the application"
    }

    fn execute(&self, _context: &mut CommandContext, _args: Vec<&str>) -> Result<CommandResult> {
        Ok(CommandResult::Exit)
    }
}

/// Clear chat history
struct ClearCommand;

impl Command for ClearCommand {
    fn name(&self) -> &str {
        "clear"
    }

    fn aliases(&self) -> Vec<&str> {
        vec!["cls"]
    }

    fn description(&self) -> &str {
        "Clear the chat history"
    }

    fn execute(&self, _context: &mut CommandContext, _args: Vec<&str>) -> Result<CommandResult> {
        Ok(CommandResult::ClearHistory)
    }
}

/// List all sessions
struct SessionsCommand;

impl Command for SessionsCommand {
    fn name(&self) -> &str {
        "sessions"
    }

    fn description(&self) -> &str {
        "List all saved sessions"
    }

    fn help(&self) -> String {
        "List all saved sessions with their IDs, models, and message counts.\n\
         Use /load <session_id> to resume a session.".to_string()
    }

    fn execute(&self, context: &mut CommandContext, _args: Vec<&str>) -> Result<CommandResult> {
        let sessions = context.session_manager.list_sessions()?;

        if sessions.is_empty() {
            return Ok(CommandResult::Info("No saved sessions found.".to_string()));
        }

        let mut output = String::from("Saved sessions:\n\n");
        for session in sessions {
            let session_id_short = if session.id.len() > 8 {
                &session.id[..8]
            } else {
                &session.id
            };

            let created = session.created_at.format("%Y-%m-%d %H:%M:%S");

            output.push_str(&format!(
                "  {} | {} | {} messages | Model: {}\n",
                session_id_short,
                created,
                session.message_count,
                session.model
            ));
        }

        output.push_str("\nUse /load <session_id> to resume a session.");

        Ok(CommandResult::Info(output))
    }
}

/// Save current session
struct SaveCommand;

impl Command for SaveCommand {
    fn name(&self) -> &str {
        "save"
    }

    fn description(&self) -> &str {
        "Save the current session"
    }

    fn execute(&self, context: &mut CommandContext, _args: Vec<&str>) -> Result<CommandResult> {
        context.session_manager.save_current_session()?;

        let session_id = context.session_manager
            .current_session()
            .map(|s| &s.id[..8.min(s.id.len())])
            .unwrap_or("unknown");

        Ok(CommandResult::Info(format!("Session {} saved successfully.", session_id)))
    }
}

/// Load a session
struct LoadCommand;

impl Command for LoadCommand {
    fn name(&self) -> &str {
        "load"
    }

    fn description(&self) -> &str {
        "Load a saved session by ID"
    }

    fn help(&self) -> String {
        "Load a saved session by its ID.\n\
         Usage: /load <session_id>\n\
         You can find session IDs using the /sessions command.".to_string()
    }

    fn execute(&self, context: &mut CommandContext, args: Vec<&str>) -> Result<CommandResult> {
        if args.is_empty() {
            return Ok(CommandResult::Error(
                "Usage: /load <session_id>\nUse /sessions to see available sessions.".to_string()
            ));
        }

        let session_id = args[0];

        // Try to find a matching session (allowing partial IDs)
        let sessions = context.session_manager.list_sessions()?;
        let matching_session = sessions
            .iter()
            .find(|s| s.id.starts_with(session_id));

        match matching_session {
            Some(session) => {
                let full_id = session.id.clone();
                context.session_manager.load_session(&full_id)?;

                Ok(CommandResult::Info(format!(
                    "Loaded session {} ({} messages)",
                    &full_id[..8.min(full_id.len())],
                    session.message_count
                )))
            }
            None => Ok(CommandResult::Error(format!(
                "Session '{}' not found. Use /sessions to see available sessions.",
                session_id
            ))),
        }
    }

    fn autocomplete(&self, context: &CommandContext, args: Vec<&str>) -> Vec<String> {
        // Get list of sessions
        let sessions = match context.session_manager.list_sessions() {
            Ok(sessions) => sessions,
            Err(_) => return vec![],
        };

        // If no args or only one arg (still typing first argument)
        if args.is_empty() {
            // Return all session IDs (short form)
            sessions.iter()
                .map(|s| {
                    let short_id = if s.id.len() > 8 { &s.id[..8] } else { &s.id };
                    short_id.to_string()
                })
                .collect()
        } else {
            // Filter by prefix
            let prefix = args[0];
            sessions.iter()
                .filter(|s| s.id.starts_with(prefix))
                .map(|s| {
                    let short_id = if s.id.len() > 8 { &s.id[..8] } else { &s.id };
                    short_id.to_string()
                })
                .collect()
        }
    }
}

/// Show git status
struct GitCommand;

impl Command for GitCommand {
    fn name(&self) -> &str {
        "git"
    }

    fn description(&self) -> &str {
        "Show git repository status"
    }

    fn execute(&self, context: &mut CommandContext, _args: Vec<&str>) -> Result<CommandResult> {
        let git_info = GitInfo::detect(context.cwd);

        let mut output = String::from("Git repository information:\n\n");

        match git_info.branch {
            Some(branch) => {
                output.push_str(&format!("  Branch: {}\n", branch));
                output.push_str(&format!("  Status: {}\n",
                    if git_info.is_dirty { "dirty (uncommitted changes)" } else { "clean" }
                ));
            }
            None => {
                output.push_str("  Not a git repository\n");
            }
        }

        Ok(CommandResult::Info(output))
    }
}

/// Show file changes made during session
struct ChangesCommand;

impl Command for ChangesCommand {
    fn name(&self) -> &str {
        "changes"
    }

    fn description(&self) -> &str {
        "Show files modified during this session"
    }

    fn execute(&self, _context: &mut CommandContext, _args: Vec<&str>) -> Result<CommandResult> {
        // Signal to main loop to display file changes from the agent
        Ok(CommandResult::ShowFileChanges)
    }
}
