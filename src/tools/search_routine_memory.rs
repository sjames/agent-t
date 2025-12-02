use crate::error::ToolError;
use crate::memory::MemoryManager;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Arguments for the SearchRoutineMemory tool
#[derive(Debug, Deserialize)]
pub struct SearchRoutineMemoryArgs {
    /// The search query
    pub query: String,
    /// Number of results to return (default: 5)
    pub top_k: Option<usize>,
}

/// Tool to search routine conversation memory
#[derive(Debug, Clone)]
pub struct SearchRoutineMemory {
    pub memory_manager: Option<Arc<Mutex<MemoryManager>>>,
}

impl Tool for SearchRoutineMemory {
    const NAME: &'static str = "search_routine_memory";
    type Error = ToolError;
    type Args = SearchRoutineMemoryArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Search through past conversation history using semantic search. Use this when the user asks 'What did we discuss about...', 'Remember when we...', or you need context from previous conversations.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query describing what you're looking for (e.g., 'authentication implementation', 'bug fix for database connection')"
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
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let memory_manager = self.memory_manager.as_ref()
            .ok_or_else(|| ToolError::Other("Memory not enabled".to_string()))?;

        let top_k = args.top_k.unwrap_or(5).min(20);

        let mut manager = memory_manager.lock().await;
        let results = manager.search_routine(&args.query, top_k)
            .map_err(|e| ToolError::Other(format!("Memory search failed: {}", e)))?;

        if results.is_empty() {
            return Ok(format!("No relevant memories found for query: '{}'", args.query));
        }

        let mut output = format!("Found {} relevant conversation memories:\n\n", results.len());

        for (idx, (chunk, score)) in results.iter().enumerate() {
            let timestamp = chunk.timestamp.format("%Y-%m-%d %H:%M");
            let content_preview = if chunk.content.len() > 200 {
                format!("{}...", &chunk.content[..200])
            } else {
                chunk.content.clone()
            };

            output.push_str(&format!(
                "{}. [{}] ({:.2} relevance) {}: {}\n   Session: {}, Role: {}\n   Tags: {}\n\n",
                idx + 1,
                timestamp,
                score,
                chunk.model,
                content_preview,
                &chunk.session_id[..8],
                chunk.role,
                chunk.context_tags.join(", ")
            ));
        }

        Ok(output)
    }
}
