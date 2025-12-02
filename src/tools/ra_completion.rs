//! Rust Analyzer completion tool

use crate::error::ToolError;
use crate::tools::ra_common;
use lsp_types::{Position, Url};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

/// Arguments for the RaCompletion tool
#[derive(Debug, Deserialize)]
pub struct RaCompletionArgs {
    /// File path
    pub file_path: String,
    /// Line number (1-indexed)
    pub line: u32,
    /// Column number (1-indexed)
    pub column: u32,
}

/// Tool to get code completion suggestions
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RaCompletion;

impl Tool for RaCompletion {
    const NAME: &'static str = "ra_completion";
    type Error = ToolError;
    type Args = RaCompletionArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Get code completion suggestions at a specific position in a Rust file. Returns available completions like functions, variables, types, etc.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the file"
                    },
                    "line": {
                        "type": "integer",
                        "description": "Line number (1-indexed)"
                    },
                    "column": {
                        "type": "integer",
                        "description": "Column number (1-indexed)"
                    }
                },
                "required": ["file_path", "line", "column"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let client = ra_common::get_client().await?;

        // Convert file path to URI
        let path = PathBuf::from(&args.file_path);
        let absolute_path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .map_err(|e| ToolError::Other(format!("Failed to get current directory: {}", e)))?
                .join(path)
        };

        let uri = Url::from_file_path(&absolute_path)
            .map_err(|_| ToolError::invalid_arguments("Invalid file path"))?;

        // Read file content and open it with rust-analyzer
        let content = tokio::fs::read_to_string(&absolute_path).await
            .map_err(ToolError::from)?;

        client.did_open(uri.clone(), "rust".to_string(), 1, content).await
            .map_err(|e| ToolError::Other(format!("Failed to open document: {}", e)))?;

        // Create position (0-indexed for LSP)
        let position = Position {
            line: args.line.saturating_sub(1),
            character: args.column.saturating_sub(1),
        };

        // Get completions
        let result = client.completion(uri.clone(), position).await
            .map_err(|e| ToolError::Other(format!("Failed to get completions: {}", e)))?;

        // Close the document
        let _ = client.did_close(uri).await;

        match result {
            Some(items) if !items.is_empty() => {
                let mut output = format!("Found {} completion(s):\n", items.len());
                let max_items = 20; // Limit to top 20 completions
                for (i, item) in items.iter().take(max_items).enumerate() {
                    let kind_str = item.kind.map(|k| format!("{:?}", k)).unwrap_or_else(|| "Unknown".to_string());
                    let detail = item.detail.as_deref().unwrap_or("");

                    output.push_str(&format!(
                        "{}. {} ({})",
                        i + 1,
                        item.label,
                        kind_str
                    ));

                    if !detail.is_empty() {
                        output.push_str(&format!(" - {}", detail));
                    }

                    output.push('\n');
                }

                if items.len() > max_items {
                    output.push_str(&format!("... and {} more", items.len() - max_items));
                }

                Ok(output)
            }
            _ => Ok("No completions available.".to_string()),
        }
    }
}
