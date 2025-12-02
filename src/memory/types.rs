use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A chunk of routine memory (conversation history)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutineMemoryChunk {
    pub session_id: String,
    pub message_id: String,
    pub timestamp: DateTime<Utc>,
    pub role: String, // user/assistant/tool
    pub content: String,
    pub working_directory: String,
    pub model: String,
    pub context_tags: Vec<String>, // auto-extracted: file paths, tool names, etc.
}

/// A chunk of key memory (curated by LLM)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyMemoryChunk {
    pub memory_id: String,
    pub timestamp: DateTime<Utc>,
    pub session_id: Option<String>,
    pub category: MemoryCategory,
    pub content: String,
    pub importance: ImportanceLevel,
    pub tags: Vec<String>,
    pub related_files: Vec<String>,
}

/// Categories for key memories
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryCategory {
    UserPreference,   // "User prefers tabs over spaces"
    ProjectFact,      // "This project uses Actix-web for HTTP"
    CodePattern,      // "Error handling uses anyhow crate"
    ProblemSolution,  // "Fixed bug X by doing Y"
    UserInstruction,  // "Always add docs to public functions"
    PersonalInfo,     // "User's name is John, works on ML projects"
    SessionSummary,   // "Currently working on X, next steps are Y" - for session continuity
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryCategory::UserPreference => write!(f, "User Preference"),
            MemoryCategory::ProjectFact => write!(f, "Project Fact"),
            MemoryCategory::CodePattern => write!(f, "Code Pattern"),
            MemoryCategory::ProblemSolution => write!(f, "Problem Solution"),
            MemoryCategory::UserInstruction => write!(f, "User Instruction"),
            MemoryCategory::PersonalInfo => write!(f, "Personal Info"),
            MemoryCategory::SessionSummary => write!(f, "Session Summary"),
        }
    }
}

/// Importance levels for key memories
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImportanceLevel {
    Low,      // Nice to have
    Medium,   // Useful context
    High,     // Very useful (frequent patterns, solutions)
    Critical, // Must remember (user preferences, critical facts)
}

impl std::fmt::Display for ImportanceLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportanceLevel::Low => write!(f, "Low"),
            ImportanceLevel::Medium => write!(f, "Medium"),
            ImportanceLevel::High => write!(f, "High"),
            ImportanceLevel::Critical => write!(f, "Critical"),
        }
    }
}

impl RoutineMemoryChunk {
    /// Extract context tags from content
    pub fn extract_tags(content: &str, tool_name: Option<&str>) -> Vec<String> {
        let mut tags = Vec::new();

        // Add tool name if present
        if let Some(tool) = tool_name {
            tags.push(format!("tool:{}", tool));
        }

        // Extract file paths (simple heuristic: contains / or .rs, .py, etc.)
        for word in content.split_whitespace() {
            if word.contains('/') || word.ends_with(".rs") || word.ends_with(".py")
                || word.ends_with(".js") || word.ends_with(".ts") {
                tags.push(format!("file:{}", word));
            }
        }

        // Extract common keywords
        let keywords = ["error", "fix", "bug", "feature", "refactor", "test"];
        for keyword in keywords {
            if content.to_lowercase().contains(keyword) {
                tags.push(keyword.to_string());
            }
        }

        tags
    }
}

impl KeyMemoryChunk {
    /// Create a new key memory
    pub fn new(
        content: String,
        category: MemoryCategory,
        importance: ImportanceLevel,
        tags: Vec<String>,
        related_files: Vec<String>,
        session_id: Option<String>,
    ) -> Self {
        Self {
            memory_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            session_id,
            category,
            content,
            importance,
            tags,
            related_files,
        }
    }
}
