use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

/// A saved message in the session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedMessage {
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

/// Session metadata and history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: String,
    pub working_directory: String,
    pub messages: Vec<SavedMessage>,
}

impl Session {
    /// Create a new session
    pub fn new(model: &str, working_directory: &str) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            name: None,
            created_at: now,
            updated_at: now,
            model: model.to_string(),
            working_directory: working_directory.to_string(),
            messages: Vec::new(),
        }
    }

    /// Add a user message
    pub fn add_user_message(&mut self, content: &str) {
        self.messages.push(SavedMessage {
            role: "user".to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
        });
        self.updated_at = Utc::now();
    }

    /// Add an assistant message
    pub fn add_assistant_message(&mut self, content: &str) {
        self.messages.push(SavedMessage {
            role: "assistant".to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
        });
        self.updated_at = Utc::now();
    }

    /// Add a tool message
    pub fn add_tool_message(&mut self, tool_name: &str, result: &str) {
        self.messages.push(SavedMessage {
            role: "tool".to_string(),
            content: format!("[{}]: {}", tool_name, result),
            timestamp: Utc::now(),
        });
        self.updated_at = Utc::now();
    }

    /// Clear the messages
    pub fn clear_messages(&mut self) {
        self.messages.clear();
        self.updated_at = Utc::now();
    }

    /// Get message count
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}

/// Manager for session persistence
pub struct SessionManager {
    sessions_dir: PathBuf,
    current_session: Option<Session>,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new() -> Result<Self> {
        let sessions_dir = Self::get_sessions_dir()?;
        fs::create_dir_all(&sessions_dir)?;

        Ok(Self {
            sessions_dir,
            current_session: None,
        })
    }

    /// Get the sessions directory
    fn get_sessions_dir() -> Result<PathBuf> {
        let data_dir = dirs::data_dir()
            .or_else(dirs::home_dir)
            .ok_or_else(|| anyhow!("Could not determine data directory"))?;

        Ok(data_dir.join("agent-t").join("sessions"))
    }

    /// Start a new session
    pub fn start_new_session(&mut self, model: &str, working_directory: &str) -> &Session {
        self.current_session = Some(Session::new(model, working_directory));
        self.current_session.as_ref().unwrap()
    }

    /// Load an existing session by ID
    pub fn load_session(&mut self, session_id: &str) -> Result<&Session> {
        let session_path = self.sessions_dir.join(format!("{}.json", session_id));

        if !session_path.exists() {
            return Err(anyhow!("Session not found: {}", session_id));
        }

        let content = fs::read_to_string(&session_path)?;
        let session: Session = serde_json::from_str(&content)?;

        self.current_session = Some(session);
        Ok(self.current_session.as_ref().unwrap())
    }

    /// Save the current session
    pub fn save_current_session(&self) -> Result<()> {
        let session = self
            .current_session
            .as_ref()
            .ok_or_else(|| anyhow!("No active session"))?;

        let session_path = self.sessions_dir.join(format!("{}.json", session.id));
        let content = serde_json::to_string_pretty(session)?;
        fs::write(&session_path, content)?;

        Ok(())
    }

    /// Get the current session
    pub fn current_session(&self) -> Option<&Session> {
        self.current_session.as_ref()
    }

    /// Get mutable reference to current session
    pub fn current_session_mut(&mut self) -> Option<&mut Session> {
        self.current_session.as_mut()
    }

    /// List all saved sessions
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let mut sessions = Vec::new();

        for entry in fs::read_dir(&self.sessions_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "json")
                && let Ok(content) = fs::read_to_string(&path)
                    && let Ok(session) = serde_json::from_str::<Session>(&content) {
                        sessions.push(SessionSummary {
                            id: session.id,
                            name: session.name,
                            created_at: session.created_at,
                            updated_at: session.updated_at,
                            message_count: session.messages.len(),
                            model: session.model,
                        });
                    }
        }

        // Sort by updated_at descending (most recent first)
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        Ok(sessions)
    }

    /// Delete a session by ID
    pub fn delete_session(&mut self, session_id: &str) -> Result<()> {
        let session_path = self.sessions_dir.join(format!("{}.json", session_id));

        if !session_path.exists() {
            return Err(anyhow!("Session not found: {}", session_id));
        }

        fs::remove_file(&session_path)?;

        // Clear current session if it was deleted
        if let Some(ref session) = self.current_session
            && session.id == session_id {
                self.current_session = None;
            }

        Ok(())
    }

    /// Get the most recent session
    pub fn get_most_recent_session(&self) -> Result<Option<Session>> {
        let sessions = self.list_sessions()?;

        if let Some(summary) = sessions.first() {
            let session_path = self.sessions_dir.join(format!("{}.json", summary.id));
            let content = fs::read_to_string(&session_path)?;
            let session: Session = serde_json::from_str(&content)?;
            return Ok(Some(session));
        }

        Ok(None)
    }
}

/// Summary of a session for listing
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: usize,
    pub model: String,
}

impl std::fmt::Display for SessionSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = self.name.as_deref().unwrap_or("(unnamed)");
        let date = self.updated_at.format("%Y-%m-%d %H:%M");
        write!(
            f,
            "{} - {} ({} messages) [{}]",
            &self.id[..8],
            name,
            self.message_count,
            date
        )
    }
}
