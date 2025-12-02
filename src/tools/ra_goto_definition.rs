//! Rust Analyzer goto definition tool

use crate::error::ToolError;
use crate::tools::ra_common;
use lsp_types::{Position, Url};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

/// Arguments for the RaGotoDefinition tool
#[derive(Debug, Deserialize)]
pub struct RaGotoDefinitionArgs {
    /// File path
    pub file_path: String,
    /// Line number (1-indexed)
    pub line: u32,
    /// Column number (1-indexed)
    pub column: u32,
}

/// Tool to go to definition of a symbol
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RaGotoDefinition;

impl Tool for RaGotoDefinition {
    const NAME: &'static str = "ra_goto_definition";
    type Error = ToolError;
    type Args = RaGotoDefinitionArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Find the definition of a symbol at a specific position in a Rust file. Returns the file path and location where the symbol is defined.".to_string(),
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

        // Get definition
        let result = client.goto_definition(uri.clone(), position).await
            .map_err(|e| ToolError::Other(format!("Failed to get definition: {}", e)))?;

        // Close the document
        let _ = client.did_close(uri).await;

        match result {
            Some(locations) if !locations.is_empty() => {
                let mut output = String::from("Definition(s) found:\n");
                for loc in locations {
                    output.push_str(&format!(
                        "  File: {}\n  Line: {}, Column: {}\n",
                        loc.uri.path(),
                        loc.range.start.line + 1,
                        loc.range.start.character + 1
                    ));
                }
                Ok(output)
            }
            _ => Ok("No definition found.".to_string()),
        }
    }
}
