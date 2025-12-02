use crate::error::ToolError;
use crate::memory::{ImportanceLevel, KeyMemoryChunk, MemoryCategory, MemoryManager};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Arguments for the StoreKeyMemory tool
#[derive(Debug, Deserialize)]
pub struct StoreKeyMemoryArgs {
    /// The content to remember
    pub content: String,
    /// Category of this memory
    pub category: String,
    /// Importance level: "low", "medium", "high", or "critical"
    pub importance: String,
    /// Optional tags for this memory
    pub tags: Option<Vec<String>>,
    /// Optional related file paths
    pub related_files: Option<Vec<String>>,
}

/// Tool to store important key memories
#[derive(Debug, Clone)]
pub struct StoreKeyMemory {
    pub memory_manager: Option<Arc<Mutex<MemoryManager>>>,
    pub session_id: Option<String>,
}

impl Tool for StoreKeyMemory {
    const NAME: &'static str = "store_key_memory";
    type Error = ToolError;
    type Args = StoreKeyMemoryArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Store an important piece of information in long-term memory. Use this when you learn something important that should be remembered across sessions, such as user preferences, project facts, code patterns, problem solutions, or personal information.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "The information to remember. Be concise but complete."
                    },
                    "category": {
                        "type": "string",
                        "enum": ["user_preference", "project_fact", "code_pattern", "problem_solution", "user_instruction", "personal_info", "session_summary"],
                        "description": "Category: user_preference (user likes/dislikes), project_fact (tech stack, architecture), code_pattern (common patterns used), problem_solution (how bugs were fixed), user_instruction (explicit instructions for future), personal_info (about the user), session_summary (current work state for continuity)"
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
                        "description": "Optional tags for easier retrieval (e.g., ['rust', 'testing'], ['authentication'])"
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
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let memory_manager = self.memory_manager.as_ref()
            .ok_or_else(|| ToolError::Other("Memory not enabled".to_string()))?;

        // Parse category
        let category = match args.category.as_str() {
            "user_preference" => MemoryCategory::UserPreference,
            "project_fact" => MemoryCategory::ProjectFact,
            "code_pattern" => MemoryCategory::CodePattern,
            "problem_solution" => MemoryCategory::ProblemSolution,
            "user_instruction" => MemoryCategory::UserInstruction,
            "personal_info" => MemoryCategory::PersonalInfo,
            "session_summary" => MemoryCategory::SessionSummary,
            _ => return Err(ToolError::invalid_arguments(format!("Invalid category: {}", args.category))),
        };

        // Parse importance
        let importance = match args.importance.as_str() {
            "low" => ImportanceLevel::Low,
            "medium" => ImportanceLevel::Medium,
            "high" => ImportanceLevel::High,
            "critical" => ImportanceLevel::Critical,
            _ => return Err(ToolError::invalid_arguments(format!("Invalid importance: {}", args.importance))),
        };

        // Create memory chunk
        let chunk = KeyMemoryChunk::new(
            args.content.clone(),
            category.clone(),
            importance.clone(),
            args.tags.unwrap_or_default(),
            args.related_files.unwrap_or_default(),
            self.session_id.clone(),
        );

        // Store the memory
        let mut manager = memory_manager.lock().await;
        manager.store_key_memory(chunk)
            .map_err(|e| ToolError::Other(format!("Failed to store memory: {}", e)))?;

        Ok(format!(
            "✓ Stored {} memory ({}): {}",
            category,
            importance,
            if args.content.len() > 60 {
                format!("{}...", &args.content[..60])
            } else {
                args.content
            }
        ))
    }
}
