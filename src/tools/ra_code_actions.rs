//! Rust Analyzer code actions tool

use crate::error::ToolError;
use crate::tools::ra_common;
use lsp_types::{Position, Range, Url};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

/// Arguments for the RaCodeActions tool
#[derive(Debug, Deserialize)]
pub struct RaCodeActionsArgs {
    /// File path
    pub file_path: String,
    /// Start line number (1-indexed)
    pub start_line: u32,
    /// Start column number (1-indexed)
    pub start_column: u32,
    /// End line number (1-indexed, defaults to start_line)
    pub end_line: Option<u32>,
    /// End column number (1-indexed, defaults to start_column)
    pub end_column: Option<u32>,
}

/// Tool to get available code actions (refactorings, fixes)
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RaCodeActions;

impl Tool for RaCodeActions {
    const NAME: &'static str = "ra_code_actions";
    type Error = ToolError;
    type Args = RaCodeActionsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Get available code actions (refactorings, quick fixes) for a range in a Rust file. Returns actions like 'extract function', 'inline variable', 'fix import', etc.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the file"
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "Start line number (1-indexed)"
                    },
                    "start_column": {
                        "type": "integer",
                        "description": "Start column number (1-indexed)"
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "End line number (1-indexed, defaults to start_line)"
                    },
                    "end_column": {
                        "type": "integer",
                        "description": "End column number (1-indexed, defaults to start_column)"
                    }
                },
                "required": ["file_path", "start_line", "start_column"]
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

        // Create range (0-indexed for LSP)
        let range = Range {
            start: Position {
                line: args.start_line.saturating_sub(1),
                character: args.start_column.saturating_sub(1),
            },
            end: Position {
                line: args.end_line.unwrap_or(args.start_line).saturating_sub(1),
                character: args.end_column.unwrap_or(args.start_column).saturating_sub(1),
            },
        };

        // Get code actions (pass empty diagnostics for now)
        let result = client.code_actions(uri.clone(), range, vec![]).await
            .map_err(|e| ToolError::Other(format!("Failed to get code actions: {}", e)))?;

        // Close the document
        let _ = client.did_close(uri).await;

        match result {
            Some(actions) if !actions.is_empty() => {
                let mut output = format!("Found {} code action(s):\n", actions.len());
                for (i, action) in actions.iter().enumerate() {
                    match action {
                        lsp_types::CodeActionOrCommand::CodeAction(ca) => {
                            output.push_str(&format!(
                                "{}. {}\n",
                                i + 1,
                                ca.title
                            ));
                            if let Some(kind) = &ca.kind {
                                output.push_str(&format!("   Kind: {:?}\n", kind));
                            }
                        }
                        lsp_types::CodeActionOrCommand::Command(cmd) => {
                            output.push_str(&format!(
                                "{}. {} (command: {})\n",
                                i + 1,
                                cmd.title,
                                cmd.command
                            ));
                        }
                    }
                }
                Ok(output)
            }
            _ => Ok("No code actions available.".to_string()),
        }
    }
}
