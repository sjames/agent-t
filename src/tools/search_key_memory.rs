use crate::error::ToolError;
use crate::memory::{ImportanceLevel, MemoryCategory, MemoryManager};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Arguments for the SearchKeyMemory tool
#[derive(Debug, Deserialize)]
pub struct SearchKeyMemoryArgs {
    /// The search query
    pub query: String,
    /// Number of results to return (default: 5)
    pub top_k: Option<usize>,
    /// Filter by categories (optional)
    pub categories: Option<Vec<String>>,
    /// Minimum importance level (optional)
    pub min_importance: Option<String>,
}

/// Tool to search curated key memories
#[derive(Debug, Clone)]
pub struct SearchKeyMemory {
    pub memory_manager: Option<Arc<Mutex<MemoryManager>>>,
}

impl Tool for SearchKeyMemory {
    const NAME: &'static str = "search_key_memory";
    type Error = ToolError;
    type Args = SearchKeyMemoryArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Search through curated important memories (user preferences, project facts, code patterns, etc.). Use this to recall what you've learned about the user, project, or important decisions. Automatically retrieves the most recent session summary when starting work.".to_string(),
            parameters: json!({
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
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let memory_manager = self.memory_manager.as_ref()
            .ok_or_else(|| ToolError::Other("Memory not enabled".to_string()))?;

        let top_k = args.top_k.unwrap_or(5).min(20);

        // Parse categories
        let categories = if let Some(cats) = args.categories {
            let parsed: Result<Vec<MemoryCategory>, _> = cats.iter().map(|c| {
                match c.as_str() {
                    "user_preference" => Ok(MemoryCategory::UserPreference),
                    "project_fact" => Ok(MemoryCategory::ProjectFact),
                    "code_pattern" => Ok(MemoryCategory::CodePattern),
                    "problem_solution" => Ok(MemoryCategory::ProblemSolution),
                    "user_instruction" => Ok(MemoryCategory::UserInstruction),
                    "personal_info" => Ok(MemoryCategory::PersonalInfo),
                    "session_summary" => Ok(MemoryCategory::SessionSummary),
                    _ => Err(ToolError::invalid_arguments(format!("Invalid category: {}", c))),
                }
            }).collect();
            Some(parsed?)
        } else {
            None
        };

        // Parse importance
        let min_importance = if let Some(imp) = args.min_importance {
            let level = match imp.as_str() {
                "low" => ImportanceLevel::Low,
                "medium" => ImportanceLevel::Medium,
                "high" => ImportanceLevel::High,
                "critical" => ImportanceLevel::Critical,
                _ => return Err(ToolError::invalid_arguments(format!("Invalid importance: {}", imp))),
            };
            Some(level)
        } else {
            None
        };

        let mut manager = memory_manager.lock().await;
        let results = manager.search_key(&args.query, top_k, categories, min_importance)
            .map_err(|e| ToolError::Other(format!("Memory search failed: {}", e)))?;

        if results.is_empty() {
            return Ok(format!("No relevant memories found for query: '{}'", args.query));
        }

        let mut output = format!("Found {} curated memories:\n\n", results.len());

        for (idx, (chunk, score)) in results.iter().enumerate() {
            let timestamp = chunk.timestamp.format("%Y-%m-%d %H:%M");
            let tags_str = if chunk.tags.is_empty() {
                "none".to_string()
            } else {
                chunk.tags.join(", ")
            };
            let files_str = if chunk.related_files.is_empty() {
                "none".to_string()
            } else {
                chunk.related_files.join(", ")
            };

            output.push_str(&format!(
                "{}. [{}] {} ({}) - Relevance: {:.2}\n   {}\n   Tags: {}\n   Files: {}\n\n",
                idx + 1,
                timestamp,
                chunk.category,
                chunk.importance,
                score,
                chunk.content,
                tags_str,
                files_str
            ));
        }

        Ok(output)
    }
}
