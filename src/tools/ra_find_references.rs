//! Rust Analyzer find references tool

use crate::error::ToolError;
use crate::tools::ra_common;
use lsp_types::{Position, Url};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

/// Arguments for the RaFindReferences tool
#[derive(Debug, Deserialize)]
pub struct RaFindReferencesArgs {
    /// File path
    pub file_path: String,
    /// Line number (1-indexed)
    pub line: u32,
    /// Column number (1-indexed)
    pub column: u32,
    /// Include the declaration in results
    #[serde(default = "default_true")]
    pub include_declaration: bool,
}

fn default_true() -> bool {
    true
}

/// Tool to find all references to a symbol
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RaFindReferences;

impl Tool for RaFindReferences {
    const NAME: &'static str = "ra_find_references";
    type Error = ToolError;
    type Args = RaFindReferencesArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Find all references to a symbol at a specific position in a Rust file. Returns all locations where the symbol is used.".to_string(),
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
                    },
                    "include_declaration": {
                        "type": "boolean",
                        "description": "Whether to include the declaration in results (default: true)"
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

        // Find references
        let result = client.find_references(uri.clone(), position, args.include_declaration).await
            .map_err(|e| ToolError::Other(format!("Failed to find references: {}", e)))?;

        // Close the document
        let _ = client.did_close(uri).await;

        match result {
            Some(locations) if !locations.is_empty() => {
                let mut output = format!("Found {} reference(s):\n", locations.len());
                for (i, loc) in locations.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. File: {}\n   Line: {}, Column: {}\n",
                        i + 1,
                        loc.uri.path(),
                        loc.range.start.line + 1,
                        loc.range.start.character + 1
                    ));
                }
                Ok(output)
            }
            _ => Ok("No references found.".to_string()),
        }
    }
}
