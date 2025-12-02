use anyhow::Result;
use crossterm::{
    event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use std::{
    collections::HashMap,
    io,
    time::Duration,
};
use tokio::sync::{mpsc::{Receiver, Sender}, oneshot};
use tui_textarea::{Input, TextArea};
use crate::colors;
use crate::commands::CommandRegistry;

/// Permission decision made by the user
#[derive(Debug, Clone)]
pub enum PermissionDecision {
    ApproveOnce,
    ApproveAll,
    Reject,
}

/// Events that can be sent from the agent to the TUI
#[derive(Debug)]
pub enum TuiEvent {
    // Output events from agent (with agent_id)
    UserMessage { agent_id: String, text: String },
    AssistantMessage { agent_id: String, text: String },
    AssistantChunk { agent_id: String, chunk: String },  // For streaming
    ToolStart { agent_id: String, name: String, args: HashMap<String, String> },
    ToolSuccess { agent_id: String, name: String, result: String },
    ToolError { agent_id: String, name: String, error: String },
    Info { agent_id: String, text: String },
    Warning { agent_id: String, text: String },
    Error { agent_id: String, text: String },

    // Status updates
    TokenUsage { agent_id: String, prompt: usize, completion: usize },
    SessionUpdate { id: String, model: String },
    SessionListUpdate(Vec<String>),  // List of session IDs for autocomplete

    // Tab lifecycle events
    TabCreate { agent_id: String, name: String },
    TabComplete { agent_id: String },
    TabFailed { agent_id: String, error: String },
    TabKill { agent_id: String },

    // Permission request
    PermissionRequest {
        tool_name: String,
        args: HashMap<String, String>,
        diff: Option<crate::diff::UnifiedDiff>,
        response_tx: oneshot::Sender<PermissionDecision>,
    },

    // System events
    Clear,
    Quit,
    Interrupt,  // Escape key pressed - cancel all agent activity
}

/// A single message in the chat history
#[derive(Debug, Clone)]
pub enum ChatMessage {
    User(String),
    Assistant(String),
    AssistantStreaming(String),  // Being actively streamed
    ToolHeader { name: String, args: HashMap<String, String> },
    ToolResult { name: String, success: bool, message: String },
    Info(String),
    Warning(String),
    Error(String),
}

impl ChatMessage {
    /// Wrap long text with backslash continuation (like shell commands)
    /// Returns lines that fit within max_width, with '\' at the end of continued lines
    fn wrap_with_continuation(text: &str, max_width: usize, indent: usize) -> Vec<String> {
        if text.len() + indent <= max_width {
            return vec![text.to_string()];
        }

        let mut lines = Vec::new();
        let mut remaining = text;
        let indent_str = " ".repeat(indent);

        while !remaining.is_empty() {
            // Available width: max_width - indent - 2 (for " \" at end)
            let available = if lines.is_empty() {
                max_width.saturating_sub(2)  // First line: no indent, but needs " \"
            } else {
                max_width.saturating_sub(indent + 2)  // Subsequent lines: indent + " \"
            };

            if remaining.len() <= available {
                // Last chunk fits
                if lines.is_empty() {
                    lines.push(remaining.to_string());
                } else {
                    lines.push(format!("{}{}", indent_str, remaining));
                }
                break;
            }

            // Find a good break point (prefer breaking at spaces)
            let mut break_at = available;
            if let Some(last_space) = remaining[..available].rfind(char::is_whitespace)
                && last_space > available / 2 {  // Only use space if it's not too early
                    break_at = last_space;
                }

            let (chunk, rest) = remaining.split_at(break_at);
            if lines.is_empty() {
                lines.push(format!("{} \\", chunk.trim_end()));
            } else {
                lines.push(format!("{}{} \\", indent_str, chunk.trim_end()));
            }
            remaining = rest.trim_start();
        }

        lines
    }

    /// Convert message to styled list items
    fn to_list_items(&self, agent_name: &str) -> Vec<ListItem<'static>> {
        match self {
            ChatMessage::User(text) => {
                const MAX_WIDTH: usize = 120;
                let prefix = "You: ";
                let prefix_len = prefix.len();

                // Wrap the user's text
                let wrapped_lines = Self::wrap_with_continuation(text, MAX_WIDTH - prefix_len, prefix_len);

                let mut items = Vec::new();
                for (i, line) in wrapped_lines.iter().enumerate() {
                    if i == 0 {
                        items.push(ListItem::new(Line::from(vec![
                            Span::styled(prefix, Style::default()
                                .fg(Color::Rgb(colors::GREEN.0, colors::GREEN.1, colors::GREEN.2))
                                .add_modifier(Modifier::BOLD)),
                            Span::styled(line.clone(), Style::default()
                                .fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2))),
                        ])));
                    } else {
                        items.push(ListItem::new(Line::from(Span::styled(
                            format!("{}{}", " ".repeat(prefix_len), line),
                            Style::default().fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2)),
                        ))));
                    }
                }

                items
            }
            ChatMessage::Assistant(text) | ChatMessage::AssistantStreaming(text) => {
                const MAX_WIDTH: usize = 120;
                let mut items = vec![
                    ListItem::new(Line::from(Span::styled(
                        format!("{}:", agent_name),
                        Style::default()
                            .fg(Color::Rgb(colors::BLUE.0, colors::BLUE.1, colors::BLUE.2))
                            .add_modifier(Modifier::BOLD),
                    )))
                ];

                // Split text into lines and wrap each line if needed
                for line in text.lines() {
                    let wrapped = Self::wrap_with_continuation(line, MAX_WIDTH - 2, 2);
                    for wrapped_line in wrapped {
                        items.push(ListItem::new(Line::from(Span::styled(
                            format!("  {}", wrapped_line),
                            Style::default().fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2)),
                        ))));
                    }
                }

                items
            }
            ChatMessage::ToolHeader { name, args } => {
                let mut items = vec![
                    ListItem::new(Line::from(vec![
                        Span::styled("⚡ ", Style::default()
                            .fg(Color::Rgb(colors::PEACH.0, colors::PEACH.1, colors::PEACH.2))),
                        Span::styled(name.clone(), Style::default()
                            .fg(Color::Rgb(colors::MAUVE.0, colors::MAUVE.1, colors::MAUVE.2))
                            .add_modifier(Modifier::BOLD)),
                    ]))
                ];

                // Add arguments with text wrapping for long values
                const MAX_WIDTH: usize = 120;  // Reasonable terminal width
                const ARG_INDENT: usize = 4;   // "    " before arg name

                for (key, value) in args {
                    let prefix = format!("    {}: ", key);
                    let prefix_len = prefix.len();

                    // Wrap the value if it's long
                    let wrapped_lines = Self::wrap_with_continuation(value, MAX_WIDTH - prefix_len, prefix_len);

                    for (i, line) in wrapped_lines.iter().enumerate() {
                        let display_text = if i == 0 {
                            format!("{}{}", prefix, line)
                        } else {
                            format!("{}{}", " ".repeat(prefix_len), line)
                        };

                        items.push(ListItem::new(Line::from(Span::styled(
                            display_text,
                            Style::default().fg(Color::Rgb(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2)),
                        ))));
                    }
                }

                items
            }
            ChatMessage::ToolResult { name: _, success, message } => {
                let (icon, color) = if *success {
                    ("✓", Color::Rgb(colors::GREEN.0, colors::GREEN.1, colors::GREEN.2))
                } else {
                    ("✗", Color::Rgb(colors::RED.0, colors::RED.1, colors::RED.2))
                };

                const MAX_WIDTH: usize = 120;
                let prefix = format!("  {} ", icon);
                let prefix_len = prefix.len();

                // Wrap long tool result messages
                let wrapped_lines = Self::wrap_with_continuation(message, MAX_WIDTH - prefix_len, prefix_len);

                let mut items = Vec::new();
                for (i, line) in wrapped_lines.iter().enumerate() {
                    if i == 0 {
                        items.push(ListItem::new(Line::from(vec![
                            Span::styled(format!("  {} ", icon), Style::default().fg(color)),
                            Span::styled(line.clone(), Style::default().fg(color)),
                        ])));
                    } else {
                        items.push(ListItem::new(Line::from(Span::styled(
                            format!("{}{}", " ".repeat(prefix_len), line),
                            Style::default().fg(color),
                        ))));
                    }
                }

                items
            }
            ChatMessage::Info(text) => {
                const MAX_WIDTH: usize = 120;
                let mut items = vec![
                    ListItem::new(Line::from(Span::styled(
                        "ℹ Info:",
                        Style::default()
                            .fg(Color::Rgb(colors::SAPPHIRE.0, colors::SAPPHIRE.1, colors::SAPPHIRE.2))
                            .add_modifier(Modifier::BOLD),
                    )))
                ];

                // Split text into lines and wrap each line if needed
                for line in text.lines() {
                    let wrapped = Self::wrap_with_continuation(line, MAX_WIDTH - 2, 2);
                    for wrapped_line in wrapped {
                        items.push(ListItem::new(Line::from(Span::styled(
                            format!("  {}", wrapped_line),
                            Style::default().fg(Color::Rgb(colors::SAPPHIRE.0, colors::SAPPHIRE.1, colors::SAPPHIRE.2)),
                        ))));
                    }
                }

                items
            }
            ChatMessage::Warning(text) => {
                const MAX_WIDTH: usize = 120;
                let mut items = vec![
                    ListItem::new(Line::from(Span::styled(
                        "⚠ Warning:",
                        Style::default()
                            .fg(Color::Rgb(colors::YELLOW.0, colors::YELLOW.1, colors::YELLOW.2))
                            .add_modifier(Modifier::BOLD),
                    )))
                ];

                // Split text into lines and wrap each line if needed
                for line in text.lines() {
                    let wrapped = Self::wrap_with_continuation(line, MAX_WIDTH - 2, 2);
                    for wrapped_line in wrapped {
                        items.push(ListItem::new(Line::from(Span::styled(
                            format!("  {}", wrapped_line),
                            Style::default().fg(Color::Rgb(colors::YELLOW.0, colors::YELLOW.1, colors::YELLOW.2)),
                        ))));
                    }
                }

                items
            }
            ChatMessage::Error(text) => {
                const MAX_WIDTH: usize = 120;
                let mut items = vec![
                    ListItem::new(Line::from(Span::styled(
                        "✗ Error:",
                        Style::default()
                            .fg(Color::Rgb(colors::RED.0, colors::RED.1, colors::RED.2))
                            .add_modifier(Modifier::BOLD),
                    )))
                ];

                // Split text into lines and wrap each line if needed
                for line in text.lines() {
                    let wrapped = Self::wrap_with_continuation(line, MAX_WIDTH - 2, 2);
                    for wrapped_line in wrapped {
                        items.push(ListItem::new(Line::from(Span::styled(
                            format!("  {}", wrapped_line),
                            Style::default().fg(Color::Rgb(colors::RED.0, colors::RED.1, colors::RED.2)),
                        ))));
                    }
                }

                items
            }
        }
    }
}

/// Tab status for tracking agent state
#[derive(Debug, Clone, PartialEq)]
pub enum TabStatus {
    Running,
    Completed,
    Failed,
    Killed,
}

/// A tab representing an agent (main or sub-agent)
pub struct AgentTab {
    pub id: String,
    pub name: String,
    pub messages: Vec<ChatMessage>,
    pub list_state: ListState,
    pub status: TabStatus,
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub auto_scroll: bool,
    pub start_time: std::time::Instant,
}

impl AgentTab {
    pub fn new(id: String, name: String) -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));

        Self {
            id,
            name,
            messages: Vec::new(),
            list_state,
            status: TabStatus::Running,
            prompt_tokens: 0,
            completion_tokens: 0,
            auto_scroll: true,
            start_time: std::time::Instant::now(),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self.status, TabStatus::Running)
    }

    #[allow(dead_code)]
    pub fn duration(&self) -> std::time::Duration {
        self.start_time.elapsed()
    }
}

/// Main TUI application state
pub struct App {
    /// Agent tabs (main + sub-agents)
    tabs: Vec<AgentTab>,

    /// Currently active tab index
    active_tab_index: usize,

    /// Input text area (shared across tabs)
    textarea: TextArea<'static>,

    /// Session information
    session_id: String,
    model_name: String,
    agent_name: String,

    /// Whether the app should quit
    should_quit: bool,

    /// Prompt history for up/down arrow navigation
    prompt_history: Vec<String>,

    /// Current index in prompt history (None = viewing current draft, Some(i) = viewing history[i])
    history_index: Option<usize>,

    /// Current draft saved when navigating history
    current_draft: String,

    /// Permission modal state
    permission_modal: Option<PermissionModal>,

    /// Autocomplete suggestions for current input
    autocomplete_suggestions: Vec<String>,

    /// Currently selected autocomplete index
    autocomplete_index: usize,

    /// Cached session IDs for autocomplete
    session_ids: Vec<String>,

    /// Current working directory
    cwd: String,

    /// Whether mouse capture is enabled (for scrolling vs text selection)
    mouse_capture_enabled: bool,
}

/// State for the permission modal
struct PermissionModal {
    tool_name: String,
    args: HashMap<String, String>,
    diff: Option<crate::diff::UnifiedDiff>,
    response_tx: oneshot::Sender<PermissionDecision>,
    scroll_offset: usize,
}

impl App {
    pub fn new(session_id: String, model_name: String, agent_name: String, cwd: String) -> Self {
        let mut textarea = TextArea::default();
        textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(""),
        );
        textarea.set_placeholder_text("Message...");

        // Create the main agent tab
        let mut main_tab = AgentTab::new("main".to_string(), "Main Agent".to_string());

        // Add startup banner to initial messages
        let version = env!("CARGO_PKG_VERSION");
        let banner = format!("Agent-t v{}\n  History is moving pretty quickly these days, and the heroes and villains keep on changing parts", version);
        main_tab.messages.push(ChatMessage::Info(banner));

        Self {
            tabs: vec![main_tab],
            active_tab_index: 0,
            textarea,
            session_id,
            model_name,
            agent_name,
            should_quit: false,
            prompt_history: Vec::new(),
            history_index: None,
            current_draft: String::new(),
            permission_modal: None,
            autocomplete_suggestions: Vec::new(),
            autocomplete_index: 0,
            session_ids: Vec::new(),
            cwd,
            mouse_capture_enabled: true,
        }
    }

    // Tab management helper methods

    fn get_active_tab(&self) -> &AgentTab {
        &self.tabs[self.active_tab_index]
    }

    fn get_active_tab_mut(&mut self) -> &mut AgentTab {
        &mut self.tabs[self.active_tab_index]
    }

    fn find_tab_by_id(&self, agent_id: &str) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == agent_id)
    }

    fn switch_to_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab_index = index;
        }
    }

    fn create_tab(&mut self, agent_id: String, name: String) {
        let tab = AgentTab::new(agent_id, name);
        self.tabs.push(tab);
        // Auto-switch to new tab
        self.active_tab_index = self.tabs.len() - 1;
    }

    fn next_tab(&mut self) {
        self.active_tab_index = (self.active_tab_index + 1) % self.tabs.len();
    }

    fn prev_tab(&mut self) {
        if self.active_tab_index == 0 {
            self.active_tab_index = self.tabs.len() - 1;
        } else {
            self.active_tab_index -= 1;
        }
    }

    fn scroll_tab_to_bottom(&mut self, tab_index: usize) {
        let agent_name = self.agent_name.clone();
        let tab = &mut self.tabs[tab_index];
        if tab.auto_scroll && !tab.messages.is_empty() {
            let total_items = tab.messages.iter()
                .map(|m| m.to_list_items(&agent_name).len())
                .sum::<usize>();
            if total_items > 0 {
                tab.list_state.select(Some(total_items.saturating_sub(1)));
            }
        }
    }

    /// Handle incoming TUI event from agent
    pub fn handle_tui_event(&mut self, event: TuiEvent) {
        match event {
            TuiEvent::UserMessage { agent_id, text } => {
                if let Some(index) = self.find_tab_by_id(&agent_id) {
                    self.tabs[index].messages.push(ChatMessage::User(text));
                    self.scroll_tab_to_bottom(index);
                }
            }
            TuiEvent::AssistantMessage { agent_id, text } => {
                if let Some(index) = self.find_tab_by_id(&agent_id) {
                    // Replace streaming message if exists, or add new
                    if let Some(ChatMessage::AssistantStreaming(_)) = self.tabs[index].messages.last() {
                        self.tabs[index].messages.pop();
                    }
                    self.tabs[index].messages.push(ChatMessage::Assistant(text));
                    self.scroll_tab_to_bottom(index);
                    // Auto-switch to this tab
                    self.switch_to_tab(index);
                }
            }
            TuiEvent::AssistantChunk { agent_id, chunk } => {
                if let Some(index) = self.find_tab_by_id(&agent_id) {
                    // Append to existing streaming message or create new
                    if let Some(ChatMessage::AssistantStreaming(text)) = self.tabs[index].messages.last_mut() {
                        text.push_str(&chunk);
                    } else {
                        self.tabs[index].messages.push(ChatMessage::AssistantStreaming(chunk));
                    }
                    self.scroll_tab_to_bottom(index);
                    // Auto-switch to this tab
                    self.switch_to_tab(index);
                }
            }
            TuiEvent::ToolStart { agent_id, name, args } => {
                if let Some(index) = self.find_tab_by_id(&agent_id) {
                    self.tabs[index].messages.push(ChatMessage::ToolHeader { name, args });
                    self.scroll_tab_to_bottom(index);
                }
            }
            TuiEvent::ToolSuccess { agent_id, name, result } => {
                if let Some(index) = self.find_tab_by_id(&agent_id) {
                    self.tabs[index].messages.push(ChatMessage::ToolResult {
                        name,
                        success: true,
                        message: result,
                    });
                    self.scroll_tab_to_bottom(index);
                }
            }
            TuiEvent::ToolError { agent_id, name, error } => {
                if let Some(index) = self.find_tab_by_id(&agent_id) {
                    self.tabs[index].messages.push(ChatMessage::ToolResult {
                        name,
                        success: false,
                        message: error,
                    });
                    self.scroll_tab_to_bottom(index);
                }
            }
            TuiEvent::Info { agent_id, text } => {
                if let Some(index) = self.find_tab_by_id(&agent_id) {
                    self.tabs[index].messages.push(ChatMessage::Info(text));
                    self.scroll_tab_to_bottom(index);
                }
            }
            TuiEvent::Warning { agent_id, text } => {
                if let Some(index) = self.find_tab_by_id(&agent_id) {
                    self.tabs[index].messages.push(ChatMessage::Warning(text));
                    self.scroll_tab_to_bottom(index);
                }
            }
            TuiEvent::Error { agent_id, text } => {
                if let Some(index) = self.find_tab_by_id(&agent_id) {
                    self.tabs[index].messages.push(ChatMessage::Error(text));
                    self.scroll_tab_to_bottom(index);
                }
            }
            TuiEvent::TokenUsage { agent_id, prompt, completion } => {
                //eprintln!("DEBUG: Received TokenUsage event - agent_id: {}, prompt: {}, completion: {}", agent_id, prompt, completion);
                if let Some(index) = self.find_tab_by_id(&agent_id) {
                    //eprintln!("DEBUG: Found tab at index {}, updating tokens", index);
                    self.tabs[index].prompt_tokens = prompt;
                    self.tabs[index].completion_tokens = completion;
                    //eprintln!("DEBUG: Tab tokens updated - prompt: {}, completion: {}",
                             //self.tabs[index].prompt_tokens, self.tabs[index].completion_tokens);
                } else {
                    //eprintln!("DEBUG: No tab found for agent_id: {}", agent_id);
                }
            }
            TuiEvent::TabCreate { agent_id, name } => {
                self.create_tab(agent_id, name);
            }
            TuiEvent::TabComplete { agent_id } => {
                if let Some(index) = self.find_tab_by_id(&agent_id) {
                    self.tabs[index].status = TabStatus::Completed;
                    // Switch back to main tab
                    self.switch_to_tab(0);
                }
            }
            TuiEvent::TabFailed { agent_id, error } => {
                if let Some(index) = self.find_tab_by_id(&agent_id) {
                    self.tabs[index].status = TabStatus::Failed;
                    self.tabs[index].messages.push(ChatMessage::Error(error));
                    self.scroll_tab_to_bottom(index);
                }
            }
            TuiEvent::TabKill { agent_id } => {
                if let Some(index) = self.find_tab_by_id(&agent_id) {
                    self.tabs[index].status = TabStatus::Killed;
                }
            }
            TuiEvent::SessionUpdate { id, model } => {
                self.session_id = id;
                self.model_name = model;
            }
            TuiEvent::SessionListUpdate(session_ids) => {
                self.session_ids = session_ids;
            }
            TuiEvent::PermissionRequest { tool_name, args, diff, response_tx } => {
                self.permission_modal = Some(PermissionModal {
                    tool_name,
                    args,
                    diff,
                    response_tx,
                    scroll_offset: 0,
                });
            }
            TuiEvent::Clear => {
                // Only clear active tab
                self.get_active_tab_mut().messages.clear();
            }
            TuiEvent::Quit => {
                self.should_quit = true;
            }
            TuiEvent::Interrupt => {
                // Show interrupt notification
                self.get_active_tab_mut().messages.push(ChatMessage::Warning(
                    "⚠ Interrupt requested - cancelling agent activity...".to_string()
                ));
                self.scroll_to_bottom();
            }
        }
    }

    /// Handle keyboard input
    pub fn handle_input(&mut self, event: Event, input_tx: &Sender<String>) -> Result<()> {
        // If permission modal is active, handle modal-specific input
        if let Some(mut modal) = self.permission_modal.take() {
            match event {
                Event::Key(key) => {
                    match key.code {
                        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                            // Approve once
                            let _ = modal.response_tx.send(PermissionDecision::ApproveOnce);
                            return Ok(());
                        }
                        KeyCode::Char('a') | KeyCode::Char('A') => {
                            // Approve all
                            let _ = modal.response_tx.send(PermissionDecision::ApproveAll);
                            return Ok(());
                        }
                        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                            // Reject
                            let _ = modal.response_tx.send(PermissionDecision::Reject);
                            return Ok(());
                        }
                        KeyCode::Up => {
                            // Scroll up in diff view
                            modal.scroll_offset = modal.scroll_offset.saturating_sub(1);
                            self.permission_modal = Some(modal);
                            return Ok(());
                        }
                        KeyCode::Down => {
                            // Scroll down in diff view
                            if let Some(ref diff) = modal.diff
                                && modal.scroll_offset < diff.lines.len().saturating_sub(1) {
                                    modal.scroll_offset += 1;
                                }
                            self.permission_modal = Some(modal);
                            return Ok(());
                        }
                        KeyCode::PageUp => {
                            // Scroll up by page
                            modal.scroll_offset = modal.scroll_offset.saturating_sub(10);
                            self.permission_modal = Some(modal);
                            return Ok(());
                        }
                        KeyCode::PageDown => {
                            // Scroll down by page
                            if let Some(ref diff) = modal.diff {
                                modal.scroll_offset = (modal.scroll_offset + 10).min(diff.lines.len().saturating_sub(1));
                            }
                            self.permission_modal = Some(modal);
                            return Ok(());
                        }
                        _ => {
                            // Unknown key, restore modal and ignore
                            self.permission_modal = Some(modal);
                            return Ok(());
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    // Handle mouse wheel scrolling in modal
                    match mouse.kind {
                        event::MouseEventKind::ScrollUp => {
                            modal.scroll_offset = modal.scroll_offset.saturating_sub(3);
                            self.permission_modal = Some(modal);
                            return Ok(());
                        }
                        event::MouseEventKind::ScrollDown => {
                            if let Some(ref diff) = modal.diff {
                                modal.scroll_offset = (modal.scroll_offset + 3).min(diff.lines.len().saturating_sub(1));
                            }
                            self.permission_modal = Some(modal);
                            return Ok(());
                        }
                        _ => {
                            // Restore modal for other mouse events
                            self.permission_modal = Some(modal);
                            return Ok(());
                        }
                    }
                }
                _ => {
                    // Restore modal for non-key events
                    self.permission_modal = Some(modal);
                    return Ok(());
                }
            }
        }

        match event {
            Event::Paste(text) => {
                // Handle pasted content - insert into textarea preserving newlines
                for line in text.lines() {
                    self.textarea.insert_str(line);
                    self.textarea.insert_newline();
                }
                // Remove the last extra newline if the paste didn't end with one
                if !text.ends_with('\n') {
                    self.textarea.delete_line_by_head();
                    self.textarea.move_cursor(tui_textarea::CursorMove::End);
                }
                return Ok(());
            }
            Event::Key(key) => {
                // Check for special key combinations first
                match (key.code, key.modifiers) {
                    // Escape - Interrupt agent activity
                    (KeyCode::Esc, KeyModifiers::NONE) => {
                        // Send interrupt signal
                        let _ = input_tx.try_send("\x1b[INTERRUPT]".to_string());
                        return Ok(());
                    }
                    // Ctrl+C or Ctrl+D - Quit
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) |
                    (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                        self.should_quit = true;
                        return Ok(());
                    }
                    // Ctrl+L - Clear history
                    (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                        self.get_active_tab_mut().messages.clear();
                        return Ok(());
                    }
                    // Ctrl+M - Toggle mouse capture (for text selection)
                    (KeyCode::Char('m'), KeyModifiers::CONTROL) => {
                        self.mouse_capture_enabled = !self.mouse_capture_enabled;
                        let mode_msg = if self.mouse_capture_enabled {
                            "Mouse mode: Scroll (mouse wheel scrolls chat)"
                        } else {
                            "Mouse mode: Select (hold Shift and drag to select text, then Ctrl+Shift+C to copy)"
                        };
                        self.get_active_tab_mut().messages.push(ChatMessage::Info(mode_msg.to_string()));
                        self.scroll_to_bottom();
                        return Ok(());
                    }
                    // Ctrl+T - Next tab
                    (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
                        self.next_tab();
                        return Ok(());
                    }
                    // Ctrl+Shift+T - Previous tab
                    (KeyCode::Char('T'), KeyModifiers::CONTROL | KeyModifiers::SHIFT) => {
                        self.prev_tab();
                        return Ok(());
                    }
                    // Ctrl+1 through Ctrl+9 - Direct tab selection
                    (KeyCode::Char(c @ '1'..='9'), KeyModifiers::CONTROL) => {
                        let index = c.to_digit(10).unwrap() as usize - 1;
                        self.switch_to_tab(index);
                        return Ok(());
                    }
                    // Tab - Autocomplete (next suggestion)
                    (KeyCode::Tab, KeyModifiers::NONE) => {
                        self.update_autocomplete();
                        self.next_autocomplete();
                        return Ok(());
                    }
                    // Shift+Tab - Autocomplete (previous suggestion)
                    (KeyCode::BackTab, _) => {
                        self.update_autocomplete();
                        self.prev_autocomplete();
                        return Ok(());
                    }
                    // Enter without Alt - Submit
                    (KeyCode::Enter, mods) if !mods.contains(KeyModifiers::ALT) => {
                        let input = self.textarea.lines().join("\n");
                        if !input.trim().is_empty() {
                            // Add to prompt history
                            self.prompt_history.push(input.clone());
                            // Reset history navigation
                            self.history_index = None;
                            self.current_draft.clear();

                            // Send to agent
                            let _ = input_tx.try_send(input.clone());
                            // Clear input
                            self.textarea = TextArea::default();
                            self.textarea.set_block(
                                Block::default()
                                    .borders(Borders::ALL)
                                    .title(""),
                            );
                            self.textarea.set_placeholder_text("Message...");
                        }
                        return Ok(());
                    }
                    // Alt+Enter - New line (handled by textarea)
                    (KeyCode::Enter, mods) if mods.contains(KeyModifiers::ALT) => {
                        self.textarea.insert_newline();
                        return Ok(());
                    }
                    // Up arrow - Navigate to previous prompt in history
                    (KeyCode::Up, KeyModifiers::NONE) => {
                        self.navigate_history_prev();
                        return Ok(());
                    }
                    // Down arrow - Navigate to next prompt in history
                    (KeyCode::Down, KeyModifiers::NONE) => {
                        self.navigate_history_next();
                        return Ok(());
                    }
                    // PageUp - Scroll up in history
                    (KeyCode::PageUp, _) => {
                        self.scroll_up(10);
                        return Ok(());
                    }
                    // PageDown - Scroll down in history
                    (KeyCode::PageDown, _) => {
                        self.scroll_down(10);
                        return Ok(());
                    }
                    // Ctrl+Up - Scroll up (alternative to PageUp)
                    (KeyCode::Up, KeyModifiers::CONTROL) => {
                        self.scroll_up(3);
                        return Ok(());
                    }
                    // Ctrl+Down - Scroll down (alternative to PageDown)
                    (KeyCode::Down, KeyModifiers::CONTROL) => {
                        self.scroll_down(3);
                        return Ok(());
                    }
                    _ => {}
                }

                // Pass to textarea for normal editing
                self.textarea.input(Input::from(event));
            }
            Event::Mouse(mouse) => {
                // Handle mouse wheel scrolling only if mouse capture is enabled
                if self.mouse_capture_enabled {
                    match mouse.kind {
                        event::MouseEventKind::ScrollUp => {
                            self.scroll_up(3);
                            return Ok(());
                        }
                        event::MouseEventKind::ScrollDown => {
                            self.scroll_down(3);
                            return Ok(());
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Navigate to previous prompt in history (Up arrow)
    fn navigate_history_prev(&mut self) {
        if self.prompt_history.is_empty() {
            return;
        }

        // If we're currently viewing the draft, save it and start from the most recent history
        if self.history_index.is_none() {
            self.current_draft = self.textarea.lines().join("\n");
            self.history_index = Some(self.prompt_history.len() - 1);
        } else if let Some(current_idx) = self.history_index {
            // Move backwards in history if possible
            if current_idx > 0 {
                self.history_index = Some(current_idx - 1);
            }
        }

        // Update textarea with history content
        if let Some(idx) = self.history_index
            && let Some(historical_prompt) = self.prompt_history.get(idx).cloned() {
                self.set_textarea_content(&historical_prompt);
            }
    }

    /// Navigate to next prompt in history (Down arrow)
    fn navigate_history_next(&mut self) {
        if let Some(current_idx) = self.history_index {
            if current_idx + 1 < self.prompt_history.len() {
                // Move forward in history
                self.history_index = Some(current_idx + 1);
                if let Some(historical_prompt) = self.prompt_history.get(current_idx + 1).cloned() {
                    self.set_textarea_content(&historical_prompt);
                }
            } else {
                // Reached the end of history, restore the draft
                self.history_index = None;
                self.set_textarea_content(&self.current_draft.clone());
            }
        }
    }

    /// Helper to set textarea content
    fn set_textarea_content(&mut self, content: &str) {
        self.textarea = TextArea::from(content.lines().map(|s| s.to_string()));
        self.textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(""),
        );
        self.textarea.set_placeholder_text("Message...");
        // Move cursor to end
        self.textarea.move_cursor(tui_textarea::CursorMove::End);
    }

    /// Scroll to bottom of message list
    fn scroll_to_bottom(&mut self) {
        let agent_name = self.agent_name.clone();
        let tab = self.get_active_tab_mut();
        if tab.auto_scroll && !tab.messages.is_empty() {
            let total_items = tab.messages.iter()
                .map(|m| m.to_list_items(&agent_name).len())
                .sum::<usize>();
            if total_items > 0 {
                tab.list_state.select(Some(total_items.saturating_sub(1)));
            }
        }
    }

    /// Scroll up in message list
    fn scroll_up(&mut self, lines: usize) {
        let tab = self.get_active_tab_mut();
        tab.auto_scroll = false;
        let current = tab.list_state.selected().unwrap_or(0);
        tab.list_state.select(Some(current.saturating_sub(lines)));
    }

    /// Scroll down in message list
    fn scroll_down(&mut self, lines: usize) {
        let agent_name = self.agent_name.clone();
        let tab = self.get_active_tab_mut();
        let total_items = tab.messages.iter()
            .map(|m| m.to_list_items(&agent_name).len())
            .sum::<usize>();

        let current = tab.list_state.selected().unwrap_or(0);
        let new_pos = (current + lines).min(total_items.saturating_sub(1));
        tab.list_state.select(Some(new_pos));

        // Re-enable auto-scroll if at bottom
        if new_pos >= total_items.saturating_sub(1) {
            let tab = self.get_active_tab_mut();
            tab.auto_scroll = true;
        }
    }

    /// Calculate the height needed for the input textarea
    /// Starts at 1 line high and grows as content increases
    fn calculate_input_height(&self) -> u16 {
        const MIN_HEIGHT: u16 = 3; // 1 line + 2 for borders
        const MAX_HEIGHT: u16 = 12; // Maximum lines to show before scrolling

        let line_count = self.textarea.lines().len() as u16;
        // Add 2 for borders
        let needed_height = line_count + 2;

        // Clamp between MIN_HEIGHT and MAX_HEIGHT
        needed_height.max(MIN_HEIGHT).min(MAX_HEIGHT)
    }

    /// Update autocomplete suggestions based on current input
    fn update_autocomplete(&mut self) {
        let input = self.textarea.lines().join("\n");

        // Only autocomplete commands (starting with /)
        if !input.starts_with('/') {
            self.autocomplete_suggestions.clear();
            self.autocomplete_index = 0;
            return;
        }

        // Create command registry and compute suggestions
        let registry = CommandRegistry::new();

        // Create a mock context with cached session IDs
        // Note: This is a simplified version. In a full implementation,
        // we'd need to create a proper CommandContext, but for autocomplete
        // we can work around it by using the simpler get_autocomplete_suggestions method
        let suggestions = self.get_autocomplete_suggestions_simple(&input, &registry);

        self.autocomplete_suggestions = suggestions;
        self.autocomplete_index = 0;
    }

    /// Get autocomplete suggestions (simplified version without full CommandContext)
    fn get_autocomplete_suggestions_simple(&self, input: &str, registry: &CommandRegistry) -> Vec<String> {
        let input = input.trim();

        if !input.starts_with('/') {
            return vec![];
        }

        let input = &input[1..];
        let parts: Vec<&str> = input.split_whitespace().collect();

        if parts.is_empty() || (parts.len() == 1 && !input.ends_with(' ')) {
            // Autocomplete command name
            let prefix = if parts.is_empty() { "" } else { parts[0] };
            let mut suggestions: Vec<String> = registry.all_commands()
                .iter()
                .filter(|cmd| cmd.name().starts_with(prefix))
                .map(|cmd| format!("/{}", cmd.name()))
                .collect();

            suggestions.sort();
            suggestions.dedup();
            suggestions
        } else {
            // Autocomplete command arguments
            let command_name = parts[0];

            // Special handling for /load command - suggest session IDs
            if command_name == "load" {
                let prefix = if parts.len() > 1 { parts[1] } else { "" };
                self.session_ids.iter()
                    .filter(|id| id.starts_with(prefix))
                    .map(|id| format!("/load {}", id))
                    .collect()
            } else if command_name == "help" {
                // Suggest command names for /help
                let prefix = if parts.len() > 1 { parts[1] } else { "" };
                registry.all_commands()
                    .iter()
                    .filter(|cmd| cmd.name().starts_with(prefix))
                    .map(|cmd| format!("/help {}", cmd.name()))
                    .collect()
            } else {
                vec![]
            }
        }
    }

    /// Cycle to next autocomplete suggestion
    fn next_autocomplete(&mut self) {
        if !self.autocomplete_suggestions.is_empty() {
            self.autocomplete_index = (self.autocomplete_index + 1) % self.autocomplete_suggestions.len();
            self.apply_autocomplete();
        }
    }

    /// Cycle to previous autocomplete suggestion
    fn prev_autocomplete(&mut self) {
        if !self.autocomplete_suggestions.is_empty() {
            if self.autocomplete_index == 0 {
                self.autocomplete_index = self.autocomplete_suggestions.len() - 1;
            } else {
                self.autocomplete_index -= 1;
            }
            self.apply_autocomplete();
        }
    }

    /// Apply the currently selected autocomplete suggestion
    fn apply_autocomplete(&mut self) {
        if let Some(suggestion) = self.autocomplete_suggestions.get(self.autocomplete_index).cloned() {
            self.set_textarea_content(&suggestion);
        }
    }

    /// Render the UI
    pub fn render(&mut self, frame: &mut Frame, terminal_area: Rect) {
        // Calculate dynamic input height based on content
        let input_height = self.calculate_input_height();

        // Create layout with three sections: history, status bar, input
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),         // Tab bar
                Constraint::Min(10),           // Chat history (takes remaining space)
                Constraint::Length(1),         // Status bar
                Constraint::Length(input_height), // Input area (dynamic)
            ])
            .split(terminal_area);

        // Render tab bar
        self.render_tab_bar(frame, chunks[0]);

        // Render chat history
        self.render_history(frame, chunks[1]);

        // Render status bar
        self.render_status_bar(frame, chunks[2]);

        // Render input area
        frame.render_widget(&self.textarea, chunks[3]);

        // Render permission modal on top if active
        if self.permission_modal.is_some() {
            self.render_permission_modal(frame, terminal_area);
        }

        // Render autocomplete suggestions if available
        if !self.autocomplete_suggestions.is_empty() {
            self.render_autocomplete(frame, chunks[3]);
        }
    }

    /// Render tab bar with status indicators
    fn render_tab_bar(&self, frame: &mut Frame, area: Rect) {
        use ratatui::widgets::Tabs;

        // Create tab titles with status indicators
        let tabs: Vec<Line> = self.tabs.iter().enumerate().map(|(i, tab)| {
            let status_icon = match tab.status {
                TabStatus::Running => "▶",
                TabStatus::Completed => "✓",
                TabStatus::Failed => "✗",
                TabStatus::Killed => "⊗",
            };

            let status_color = match tab.status {
                TabStatus::Running => Color::Rgb(colors::GREEN.0, colors::GREEN.1, colors::GREEN.2),
                TabStatus::Completed => Color::Rgb(colors::BLUE.0, colors::BLUE.1, colors::BLUE.2),
                TabStatus::Failed => Color::Rgb(colors::RED.0, colors::RED.1, colors::RED.2),
                TabStatus::Killed => Color::Rgb(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2),
            };

            let tab_name = if tab.name.len() > 15 {
                format!("{}...", &tab.name[..12])
            } else {
                tab.name.clone()
            };

            let style = if i == self.active_tab_index {
                Style::default()
                    .bg(Color::Rgb(colors::SURFACE0.0, colors::SURFACE0.1, colors::SURFACE0.2))
                    .fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Rgb(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2))
            };

            Line::from(vec![
                Span::styled(format!(" {} ", status_icon), Style::default().fg(status_color)),
                Span::styled(format!("{} ", tab_name), style),
            ])
        }).collect();

        let tabs_widget = Tabs::new(tabs)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Agents (Ctrl+T: next, Ctrl+1-9: direct) ")
            )
            .highlight_style(
                Style::default()
                    .bg(Color::Rgb(colors::SURFACE0.0, colors::SURFACE0.1, colors::SURFACE0.2))
                    .add_modifier(Modifier::BOLD)
            )
            .select(self.active_tab_index);

        frame.render_widget(tabs_widget, area);
    }

    /// Render chat history
    fn render_history(&mut self, frame: &mut Frame, area: Rect) {
        let agent_name = self.agent_name.clone();
        let tab = self.get_active_tab_mut();
        // Convert messages to list items
        let items: Vec<ListItem> = tab.messages.iter()
            .flat_map(|msg| msg.to_list_items(&agent_name))
            .collect();

        let list = List::new(items)
            .highlight_style(Style::default().add_modifier(Modifier::BOLD));

        frame.render_stateful_widget(list, area, &mut tab.list_state);
    }

    /// Render status bar
    fn render_status_bar(&self, frame: &mut Frame, area: Rect) {
        let tab = self.get_active_tab();
        let session_short = if self.session_id.len() > 8 {
            &self.session_id[..8]
        } else {
            &self.session_id
        };

        let total_tokens = tab.prompt_tokens + tab.completion_tokens;

        let mode_indicator = if self.mouse_capture_enabled {
            "Scroll"
        } else {
            "Select (Shift+drag)"
        };

        let status_text = format!(
            " Session: {} | Model: {} | Tab: {} | Tokens: {}/{}/{} | Mode: {} (Ctrl+M to toggle) ",
            session_short,
            self.model_name,
            tab.name,
            tab.prompt_tokens,
            tab.completion_tokens,
            total_tokens,
            mode_indicator
        );

        let status = Paragraph::new(status_text)
            .style(Style::default()
                .bg(Color::Rgb(colors::SURFACE0.0, colors::SURFACE0.1, colors::SURFACE0.2))
                .fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2)));

        frame.render_widget(status, area);
    }

    /// Render permission modal
    fn render_permission_modal(&self, frame: &mut Frame, area: Rect) {
        if let Some(modal) = &self.permission_modal {
            // Clear the entire background first
            frame.render_widget(Clear, area);

            // Then render a solid black overlay to block background text
            let overlay = Block::default()
                .style(Style::default()
                    .bg(Color::Rgb(0, 0, 0))); // Solid black background
            frame.render_widget(overlay, area);

            // Create larger modal area for diff display
            let modal_width = if modal.diff.is_some() {
                area.width.saturating_sub(4).min(120)
            } else {
                area.width.min(80)
            };

            let modal_height = if modal.diff.is_some() {
                area.height.saturating_sub(4).max(20)
            } else {
                // Calculate needed height for non-diff modal
                let needed_height = if modal.args.is_empty() {
                    6
                } else {
                    8 + modal.args.len() as u16
                };
                needed_height.min(area.height - 4)
            };

            let modal_x = (area.width.saturating_sub(modal_width)) / 2;
            let modal_y = (area.height.saturating_sub(modal_height)) / 2;

            let modal_area = Rect {
                x: area.x + modal_x,
                y: area.y + modal_y,
                width: modal_width,
                height: modal_height,
            };

            // If there's a diff, use a different layout
            if let Some(ref diff) = modal.diff {
                // Split modal into header, diff view, and footer
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),        // Header (tool name + summary)
                        Constraint::Min(10),          // Diff view (scrollable)
                        Constraint::Length(3),        // Footer (instructions)
                    ])
                    .split(modal_area);

                // Render header
                let header_lines = vec![
                    Line::from(vec![
                        Span::styled("Tool: ", Style::default()
                            .fg(Color::Rgb(colors::YELLOW.0, colors::YELLOW.1, colors::YELLOW.2))
                            .add_modifier(Modifier::BOLD)),
                        Span::styled(&modal.tool_name, Style::default()
                            .fg(Color::Rgb(colors::MAUVE.0, colors::MAUVE.1, colors::MAUVE.2))
                            .add_modifier(Modifier::BOLD)),
                        Span::styled(" - ", Style::default()
                            .fg(Color::Rgb(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2))),
                        Span::styled(diff.summary(), Style::default()
                            .fg(Color::Rgb(colors::SAPPHIRE.0, colors::SAPPHIRE.1, colors::SAPPHIRE.2))),
                    ]),
                ];

                let header = Paragraph::new(header_lines)
                    .block(Block::default()
                        .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                        .title(" Permission Required ")
                        .style(Style::default()
                            .bg(Color::Rgb(colors::BASE.0, colors::BASE.1, colors::BASE.2))))
                    .style(Style::default()
                        .bg(Color::Rgb(colors::BASE.0, colors::BASE.1, colors::BASE.2)));

                frame.render_widget(header, chunks[0]);

                // Render scrollable diff view
                let available_height = chunks[1].height.saturating_sub(2) as usize; // Account for borders
                let visible_lines: Vec<Line> = diff.lines
                    .iter()
                    .skip(modal.scroll_offset)
                    .take(available_height)
                    .map(|diff_line| {
                        // Format line numbers
                        let line_num_str = match (&diff_line.old_line_num, &diff_line.new_line_num) {
                            (Some(old), Some(new)) => format!("{:>4} {:>4} ", old, new),
                            (Some(old), None) => format!("{:>4} {:>4} ", old, ""),
                            (None, Some(new)) => format!("{:>4} {:>4} ", "", new),
                            (None, None) => "         ".to_string(),
                        };

                        // Choose color and prefix based on change type
                        let (prefix, color) = match diff_line.change_type {
                            crate::diff::DiffChangeType::Addition => ("+", Color::Rgb(colors::GREEN.0, colors::GREEN.1, colors::GREEN.2)),
                            crate::diff::DiffChangeType::Deletion => ("-", Color::Rgb(colors::RED.0, colors::RED.1, colors::RED.2)),
                            crate::diff::DiffChangeType::Context => (" ", Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2)),
                        };

                        Line::from(vec![
                            Span::styled(line_num_str, Style::default()
                                .fg(Color::Rgb(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2))),
                            Span::styled(format!("{} ", prefix), Style::default().fg(color)),
                            Span::styled(&diff_line.content, Style::default().fg(color)),
                        ])
                    })
                    .collect();

                let scroll_indicator = if diff.lines.len() > available_height {
                    format!(" ({}/{}) ↕ Scroll ", modal.scroll_offset + 1, diff.lines.len())
                } else {
                    String::new()
                };

                let diff_view = Paragraph::new(visible_lines)
                    .block(Block::default()
                        .borders(Borders::LEFT | Borders::RIGHT)
                        .title(scroll_indicator)
                        .style(Style::default()
                            .bg(Color::Rgb(colors::BASE.0, colors::BASE.1, colors::BASE.2))))
                    .style(Style::default()
                        .bg(Color::Rgb(colors::BASE.0, colors::BASE.1, colors::BASE.2)));

                frame.render_widget(diff_view, chunks[1]);

                // Render footer with instructions
                let footer_lines = vec![
                    Line::from(vec![
                        Span::styled("[Enter/Y]", Style::default()
                            .fg(Color::Rgb(colors::GREEN.0, colors::GREEN.1, colors::GREEN.2))
                            .add_modifier(Modifier::BOLD)),
                        Span::styled(" Approve Once  ", Style::default()
                            .fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2))),
                        Span::styled("[A]", Style::default()
                            .fg(Color::Rgb(colors::BLUE.0, colors::BLUE.1, colors::BLUE.2))
                            .add_modifier(Modifier::BOLD)),
                        Span::styled(" Approve All  ", Style::default()
                            .fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2))),
                        Span::styled("[Esc/N]", Style::default()
                            .fg(Color::Rgb(colors::RED.0, colors::RED.1, colors::RED.2))
                            .add_modifier(Modifier::BOLD)),
                        Span::styled(" Reject", Style::default()
                            .fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2))),
                    ]),
                ];

                let footer = Paragraph::new(footer_lines)
                    .block(Block::default()
                        .borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT)
                        .style(Style::default()
                            .bg(Color::Rgb(colors::BASE.0, colors::BASE.1, colors::BASE.2))))
                    .style(Style::default()
                        .bg(Color::Rgb(colors::BASE.0, colors::BASE.1, colors::BASE.2)));

                frame.render_widget(footer, chunks[2]);
            } else {
                // No diff - render traditional permission modal
                let mut lines = vec![
                    Line::from(vec![
                        Span::styled("Tool: ", Style::default()
                            .fg(Color::Rgb(colors::YELLOW.0, colors::YELLOW.1, colors::YELLOW.2))
                            .add_modifier(Modifier::BOLD)),
                        Span::styled(&modal.tool_name, Style::default()
                            .fg(Color::Rgb(colors::MAUVE.0, colors::MAUVE.1, colors::MAUVE.2))
                            .add_modifier(Modifier::BOLD)),
                    ]),
                    Line::from(""),
                ];

                // Add arguments
                if !modal.args.is_empty() {
                    lines.push(Line::from(Span::styled("Arguments:", Style::default().add_modifier(Modifier::BOLD))));
                    for (key, value) in &modal.args {
                        let display_value = if value.len() > 60 {
                            format!("{}...", &value[..60])
                        } else {
                            value.clone()
                        };
                        lines.push(Line::from(vec![
                            Span::styled("  ", Style::default()),
                            Span::styled(key, Style::default()
                                .fg(Color::Rgb(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2))),
                            Span::styled(": ", Style::default()
                                .fg(Color::Rgb(colors::OVERLAY0.0, colors::OVERLAY0.1, colors::OVERLAY0.2))),
                            Span::styled(display_value, Style::default()
                                .fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2))),
                        ]));
                    }
                    lines.push(Line::from(""));
                }

                // Add instructions
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled("[Enter/Y]", Style::default()
                        .fg(Color::Rgb(colors::GREEN.0, colors::GREEN.1, colors::GREEN.2))
                        .add_modifier(Modifier::BOLD)),
                    Span::styled(" Approve Once  ", Style::default()
                        .fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2))),
                    Span::styled("[A]", Style::default()
                        .fg(Color::Rgb(colors::BLUE.0, colors::BLUE.1, colors::BLUE.2))
                        .add_modifier(Modifier::BOLD)),
                    Span::styled(" Approve All  ", Style::default()
                        .fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2))),
                    Span::styled("[Esc/N]", Style::default()
                        .fg(Color::Rgb(colors::RED.0, colors::RED.1, colors::RED.2))
                        .add_modifier(Modifier::BOLD)),
                    Span::styled(" Reject", Style::default()
                        .fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2))),
                ]));

                let paragraph = Paragraph::new(lines)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" Permission Required ")
                            .style(Style::default()
                                .bg(Color::Rgb(colors::BASE.0, colors::BASE.1, colors::BASE.2))
                                .fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2)))
                    )
                    .style(Style::default()
                        .bg(Color::Rgb(colors::BASE.0, colors::BASE.1, colors::BASE.2)));

                frame.render_widget(paragraph, modal_area);
            }
        }
    }

    /// Render autocomplete suggestions popup
    fn render_autocomplete(&self, frame: &mut Frame, input_area: Rect) {
        if self.autocomplete_suggestions.is_empty() {
            return;
        }

        // Calculate popup dimensions
        let max_width = 60;
        let popup_height = (self.autocomplete_suggestions.len() as u16).min(8) + 2; // +2 for borders
        let popup_width = self.autocomplete_suggestions.iter()
            .map(|s| s.len())
            .max()
            .unwrap_or(20)
            .min(max_width as usize) as u16 + 4; // +4 for padding and borders

        // Position popup just above the input area
        let popup_x = input_area.x + 2;
        let popup_y = input_area.y.saturating_sub(popup_height);

        let popup_area = Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        };

        // Create list items for suggestions
        let items: Vec<ListItem> = self.autocomplete_suggestions.iter()
            .enumerate()
            .map(|(i, suggestion)| {
                let style = if i == self.autocomplete_index {
                    // Highlight selected item
                    Style::default()
                        .bg(Color::Rgb(colors::SURFACE0.0, colors::SURFACE0.1, colors::SURFACE0.2))
                        .fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::Rgb(colors::TEXT.0, colors::TEXT.1, colors::TEXT.2))
                };

                ListItem::new(Line::from(Span::styled(
                    format!(" {} ", suggestion),
                    style
                )))
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Suggestions (Tab/Shift+Tab to cycle) ")
                    .style(Style::default()
                        .bg(Color::Rgb(colors::BASE.0, colors::BASE.1, colors::BASE.2))
                        .fg(Color::Rgb(colors::SAPPHIRE.0, colors::SAPPHIRE.1, colors::SAPPHIRE.2)))
            );

        frame.render_widget(list, popup_area);
    }
}

/// Main TUI event loop
pub async fn run(
    session_id: String,
    model_name: String,
    agent_name: String,
    cwd: String,
    mut event_rx: Receiver<TuiEvent>,
    input_tx: Sender<String>,
) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut app = App::new(session_id, model_name, agent_name, cwd);

    // Track previous mouse capture state to detect changes
    let mut prev_mouse_capture = app.mouse_capture_enabled;

    // Main event loop
    loop {
        // Check if mouse capture state changed and update terminal
        if app.mouse_capture_enabled != prev_mouse_capture {
            if app.mouse_capture_enabled {
                execute!(terminal.backend_mut(), EnableMouseCapture)?;
            } else {
                execute!(terminal.backend_mut(), DisableMouseCapture)?;
            }
            prev_mouse_capture = app.mouse_capture_enabled;
        }

        // Render
        terminal.draw(|f| {
            let area = f.area();
            app.render(f, area);
        })?;

        // Handle events with timeout
        let timeout = Duration::from_millis(100);

        tokio::select! {
            // Handle terminal events (keyboard, mouse, etc.)
            poll_result = tokio::task::spawn_blocking(move || event::poll(timeout)) => {
                if let Ok(Ok(true)) = poll_result
                    && let Ok(event) = event::read() {
                        app.handle_input(event, &input_tx)?;
                    }
            }

            // Handle events from agent
            Some(tui_event) = event_rx.recv() => {
                app.handle_tui_event(tui_event);
            }
        }

        // Check if should quit
        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    Ok(())
}
