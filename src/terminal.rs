use colored::Colorize;
use std::io::{self, Write};
use tokio::sync::mpsc::Sender;
use crate::tui::TuiEvent;
use crate::colors;
use std::collections::HashMap;

/// Dangerous command patterns that should trigger confirmation
const DANGEROUS_PATTERNS: &[&str] = &[
    "rm -rf",
    "rm -r",
    "rmdir",
    "sudo rm",
    "sudo dd",
    "mkfs",
    "fdisk",
    "> /dev/",
    "chmod 777",
    "chmod -R 777",
    ":(){:|:&};:",  // fork bomb
    "dd if=",
    "mv /* ",
    "mv / ",
    "wget | sh",
    "curl | sh",
    "wget | bash",
    "curl | bash",
    "sudo su",
    "sudo -i",
    "shutdown",
    "reboot",
    "init 0",
    "init 6",
    "kill -9 -1",
    "pkill -9",
    "DROP TABLE",
    "DROP DATABASE",
    "TRUNCATE TABLE",
    "DELETE FROM",
    "format c:",
    "del /f /s /q",
];

/// Dangerous file paths
const DANGEROUS_PATHS: &[&str] = &[
    "/etc/passwd",
    "/etc/shadow",
    "/etc/sudoers",
    "~/.ssh/",
    "/.ssh/",
    "/root/",
    "/boot/",
    "/dev/",
    "/proc/",
    "/sys/",
];

/// Check if a command is potentially dangerous
pub fn is_dangerous_command(command: &str) -> Option<&'static str> {
    let cmd_lower = command.to_lowercase();

    DANGEROUS_PATTERNS.iter().find(|&pattern| cmd_lower.contains(&pattern.to_lowercase())).map(|v| v as _)
}

/// Check if a file path is potentially dangerous
pub fn is_dangerous_path(path: &str) -> Option<&'static str> {
    DANGEROUS_PATHS.iter().find(|&dangerous_path| path.contains(dangerous_path)).map(|v| v as _)
}

/// Prompt the user for confirmation
pub fn confirm(message: &str) -> io::Result<bool> {
    print!("{} {} ", "⚠️".truecolor(colors::YELLOW.0, colors::YELLOW.1, colors::YELLOW.2),
           message.truecolor(colors::YELLOW.0, colors::YELLOW.1, colors::YELLOW.2));
    print!("{}", "[y/N] ".truecolor(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2));
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(input.trim().to_lowercase() == "y" || input.trim().to_lowercase() == "yes")
}

/// Print a user prompt
pub fn print_user_prompt() {
    print!("{} ", "You:".truecolor(colors::GREEN.0, colors::GREEN.1, colors::GREEN.2).bold());
    io::stdout().flush().ok();
}

/// Print the assistant name
pub fn print_assistant_prompt() {
    print!("\n{} ", "agent-t:".truecolor(colors::BLUE.0, colors::BLUE.1, colors::BLUE.2).bold());
    io::stdout().flush().ok();
}

/// Print assistant response
pub fn print_assistant_response(response: &str) {
    println!("{}\n", response);
}

/// Print a tool execution header
pub fn print_tool_header(tool_name: &str) {
    println!("\n{} {}",
             "[Tool:".truecolor(colors::MAUVE.0, colors::MAUVE.1, colors::MAUVE.2),
             tool_name.truecolor(colors::MAUVE.0, colors::MAUVE.1, colors::MAUVE.2).bold());
}

/// Print tool arguments
pub fn print_tool_arg(key: &str, value: &str) {
    let display_value = if value.len() > 100 {
        format!("{}...", &value[..100])
    } else {
        value.to_string()
    };
    println!("  {}: {}",
             key.truecolor(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2),
             display_value);
}


/// Print an info message
pub fn print_info(message: &str) {
    println!("{} {}",
             "ℹ️".truecolor(colors::SAPPHIRE.0, colors::SAPPHIRE.1, colors::SAPPHIRE.2),
             message.truecolor(colors::SAPPHIRE.0, colors::SAPPHIRE.1, colors::SAPPHIRE.2));
}

/// Print a success message
pub fn print_success(message: &str) {
    println!("{} {}",
             "✓".truecolor(colors::GREEN.0, colors::GREEN.1, colors::GREEN.2),
             message.truecolor(colors::GREEN.0, colors::GREEN.1, colors::GREEN.2));
}

/// Print a warning message
pub fn print_warning(message: &str) {
    println!("{} {}",
             "⚠️".truecolor(colors::YELLOW.0, colors::YELLOW.1, colors::YELLOW.2),
             message.truecolor(colors::YELLOW.0, colors::YELLOW.1, colors::YELLOW.2));
}

/// Print an error message
pub fn print_error(message: &str) {
    eprintln!("{} {}",
              "✗".truecolor(colors::RED.0, colors::RED.1, colors::RED.2),
              message.truecolor(colors::RED.0, colors::RED.1, colors::RED.2));
}

/// Print the inspector URL
pub fn print_inspector_url(port: u16) {
    println!(
        "\n{} Traffic Inspector: {}",
        "🔍".truecolor(colors::MAUVE.0, colors::MAUVE.1, colors::MAUVE.2),
        format!("http://localhost:{}", port)
            .truecolor(colors::MAUVE.0, colors::MAUVE.1, colors::MAUVE.2)
            .underline()
    );
}

/// Print the working directory
pub fn print_working_dir(path: &str) {
    println!(
        "{} {}",
        "Working directory:".truecolor(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2),
        path.truecolor(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2)
    );
}

/// Print git repository info
pub fn print_git_info(git_info: &crate::git::GitInfo) {
    if git_info.is_repo {
        let branch = git_info.branch.as_deref().unwrap_or("unknown");
        let status_color = if git_info.is_dirty {
            branch.truecolor(colors::YELLOW.0, colors::YELLOW.1, colors::YELLOW.2)
        } else {
            branch.truecolor(colors::GREEN.0, colors::GREEN.1, colors::GREEN.2)
        };

        let status_indicator = if git_info.is_dirty {
            format!(
                " ({} staged, {} modified, {} untracked)",
                git_info.staged_count, git_info.unstaged_count, git_info.untracked_count
            )
            .truecolor(colors::YELLOW.0, colors::YELLOW.1, colors::YELLOW.2)
        } else {
            " (clean)".truecolor(colors::GREEN.0, colors::GREEN.1, colors::GREEN.2)
        };

        println!(
            "{} {}{}",
            "Git:".truecolor(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2),
            status_color,
            status_indicator
        );
    }
    println!();
}

/// Print history count
pub fn print_history_count(count: usize) {
    println!(
        "{}\n",
        format!("[History: {} messages]", count)
            .truecolor(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2)
    );
}

/// Print session info
pub fn print_session_info(session_id: &str, message_count: usize) {
    println!(
        "{} Session: {} ({} messages)",
        "📁".truecolor(colors::SAPPHIRE.0, colors::SAPPHIRE.1, colors::SAPPHIRE.2),
        &session_id[..8].truecolor(colors::SAPPHIRE.0, colors::SAPPHIRE.1, colors::SAPPHIRE.2),
        message_count
    );
}

/// Format a streaming token for display
pub fn print_streaming_token(token: &str) {
    print!("{}", token);
    io::stdout().flush().ok();
}

/// End streaming output
pub fn end_streaming() {
    println!("\n");
}

/// Create a spinner for LLM thinking
pub fn create_thinking_spinner() -> indicatif::ProgressBar {
    let spinner = indicatif::ProgressBar::new_spinner();
    spinner.set_style(
        indicatif::ProgressStyle::default_spinner()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
            .template("{spinner:.blue} {msg}")
            .unwrap(),
    );
    spinner.set_message("Thinking...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));
    spinner
}

/// Create a spinner for tool execution
pub fn create_tool_spinner(tool_name: &str) -> indicatif::ProgressBar {
    let spinner = indicatif::ProgressBar::new_spinner();
    spinner.set_style(
        indicatif::ProgressStyle::default_spinner()
            .tick_chars("⣾⣽⣻⢿⡿⣟⣯⣷")
            .template("{spinner:.magenta} {msg}")
            .unwrap(),
    );
    spinner.set_message(format!("Running {}...", tool_name));
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));
    spinner
}

/// Finish a spinner with success
pub fn finish_spinner_success(spinner: &indicatif::ProgressBar, message: &str) {
    let formatted = format!("{} {}",
                            "✓".truecolor(colors::GREEN.0, colors::GREEN.1, colors::GREEN.2),
                            message.truecolor(colors::GREEN.0, colors::GREEN.1, colors::GREEN.2));
    spinner.finish_with_message(formatted);
}

/// Finish a spinner with error
pub fn finish_spinner_error(spinner: &indicatif::ProgressBar, message: &str) {
    let formatted = format!("{} {}",
                            "✗".truecolor(colors::RED.0, colors::RED.1, colors::RED.2),
                            message.truecolor(colors::RED.0, colors::RED.1, colors::RED.2));
    spinner.finish_with_message(formatted);
}

/// Clear/abandon a spinner (for streaming mode where we don't want final message)
pub fn clear_spinner(spinner: &indicatif::ProgressBar) {
    spinner.finish_and_clear();
}

/// Create a progress bar for indexing files
pub fn create_indexing_progress(total: u64) -> indicatif::ProgressBar {
    let pb = indicatif::ProgressBar::new(total);
    pb.set_style(
        indicatif::ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("█▓▒░  "),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

/// Create a progress bar for embedding generation
pub fn create_embedding_progress(total: u64) -> indicatif::ProgressBar {
    let pb = indicatif::ProgressBar::new(total);
    pb.set_style(
        indicatif::ProgressStyle::default_bar()
            .template("{spinner:.yellow} [{elapsed_precise}] [{bar:40.yellow/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("█▓▒░  "),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

/// Print token usage summary
pub fn print_token_usage(usage: &crate::agent_loop::TokenUsage) {
    println!(
        "\n{} Token Usage (estimated):",
        "📊".truecolor(colors::SAPPHIRE.0, colors::SAPPHIRE.1, colors::SAPPHIRE.2)
    );
    println!(
        "  {} {} prompt tokens",
        "→".truecolor(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2),
        usage.prompt_tokens.to_string().truecolor(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2)
    );
    println!(
        "  {} {} completion tokens",
        "←".truecolor(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2),
        usage.completion_tokens.to_string().truecolor(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2)
    );
    println!(
        "  {} {} total tokens",
        "∑".truecolor(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2),
        usage.total_tokens.to_string().truecolor(colors::YELLOW.0, colors::YELLOW.1, colors::YELLOW.2)
    );
    println!(
        "  {} {} requests",
        "#".truecolor(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2),
        usage.request_count.to_string().truecolor(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2)
    );
    println!();
}

/// Print file changes summary
pub fn print_file_changes_summary(changes: &[&crate::agent_loop::FileChange]) {
    if changes.is_empty() {
        println!("{} No files modified this session.",
                 "📄".truecolor(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2));
        return;
    }

    println!(
        "\n{} {} file(s) modified this session:",
        "📄".truecolor(colors::SAPPHIRE.0, colors::SAPPHIRE.1, colors::SAPPHIRE.2),
        changes.len()
    );

    for change in changes {
        let symbol = match change.operation {
            crate::agent_loop::FileOperation::Created =>
                "+".truecolor(colors::GREEN.0, colors::GREEN.1, colors::GREEN.2),
            crate::agent_loop::FileOperation::Modified =>
                "~".truecolor(colors::YELLOW.0, colors::YELLOW.1, colors::YELLOW.2),
            crate::agent_loop::FileOperation::Deleted =>
                "-".truecolor(colors::RED.0, colors::RED.1, colors::RED.2),
        };
        println!(
            "  {} {} ({})",
            symbol,
            change.path.truecolor(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2),
            change.operation.to_string().truecolor(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2)
        );
    }
    println!();
}

// ============================================================================
// TUI Event Emitting Functions
// ============================================================================

/// Emit an assistant message chunk (for streaming)
pub fn emit_assistant_chunk(tx: &Sender<TuiEvent>, agent_id: &str, chunk: &str) {
    let _ = tx.try_send(TuiEvent::AssistantChunk {
        agent_id: agent_id.to_string(),
        chunk: chunk.to_string(),
    });
}

/// Emit a complete assistant message
pub fn emit_assistant_message(tx: &Sender<TuiEvent>, agent_id: &str, message: &str) {
    let _ = tx.try_send(TuiEvent::AssistantMessage {
        agent_id: agent_id.to_string(),
        text: message.to_string(),
    });
}

/// Emit a tool start event
pub fn emit_tool_start(tx: &Sender<TuiEvent>, agent_id: &str, tool_name: &str, args: HashMap<String, String>) {
    let _ = tx.try_send(TuiEvent::ToolStart {
        agent_id: agent_id.to_string(),
        name: tool_name.to_string(),
        args,
    });
}

/// Emit a tool success event
pub fn emit_tool_success(tx: &Sender<TuiEvent>, agent_id: &str, tool_name: &str, result: &str) {
    let _ = tx.try_send(TuiEvent::ToolSuccess {
        agent_id: agent_id.to_string(),
        name: tool_name.to_string(),
        result: result.to_string(),
    });
}

/// Emit a tool error event
pub fn emit_tool_error(tx: &Sender<TuiEvent>, agent_id: &str, tool_name: &str, error: &str) {
    let _ = tx.try_send(TuiEvent::ToolError {
        agent_id: agent_id.to_string(),
        name: tool_name.to_string(),
        error: error.to_string(),
    });
}

/// Emit an info message
pub fn emit_info(tx: &Sender<TuiEvent>, agent_id: &str, message: &str) {
    let _ = tx.try_send(TuiEvent::Info {
        agent_id: agent_id.to_string(),
        text: message.to_string(),
    });
}

/// Emit a warning message
pub fn emit_warning(tx: &Sender<TuiEvent>, agent_id: &str, message: &str) {
    let _ = tx.try_send(TuiEvent::Warning {
        agent_id: agent_id.to_string(),
        text: message.to_string(),
    });
}

/// Emit an error message
pub fn emit_error(tx: &Sender<TuiEvent>, agent_id: &str, message: &str) {
    let _ = tx.try_send(TuiEvent::Error {
        agent_id: agent_id.to_string(),
        text: message.to_string(),
    });
}

/// Emit token usage update
pub fn emit_token_usage(tx: &Sender<TuiEvent>, agent_id: &str, prompt: usize, completion: usize) {
    let _ = tx.try_send(TuiEvent::TokenUsage {
        agent_id: agent_id.to_string(),
        prompt,
        completion,
    });
}

/// Emit session update
pub fn emit_session_update(tx: &Sender<TuiEvent>, id: &str, model: &str) {
    let _ = tx.try_send(TuiEvent::SessionUpdate {
        id: id.to_string(),
        model: model.to_string(),
    });
}
