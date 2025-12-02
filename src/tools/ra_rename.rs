//! Rust Analyzer rename symbol tool

use crate::error::ToolError;
use crate::tools::ra_common;
use lsp_types::{Position, Url};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

/// Arguments for the RaRename tool
#[derive(Debug, Deserialize)]
pub struct RaRenameArgs {
    /// File path
    pub file_path: String,
    /// Line number (1-indexed)
    pub line: u32,
    /// Column number (1-indexed)
    pub column: u32,
    /// New name for the symbol
    pub new_name: String,
}

/// Tool to rename a symbol across the workspace
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RaRename;

impl Tool for RaRename {
    const NAME: &'static str = "ra_rename";
    type Error = ToolError;
    type Args = RaRenameArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Rename a symbol at a specific position in a Rust file. Returns all locations that will be affected by the rename.".to_string(),
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
                    "new_name": {
                        "type": "string",
                        "description": "New name for the symbol"
                    }
                },
                "required": ["file_path", "line", "column", "new_name"]
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

        // Get rename edits
        let result = client.rename(uri.clone(), position, args.new_name.clone()).await
            .map_err(|e| ToolError::Other(format!("Failed to get rename edits: {}", e)))?;

        // Close the document
        let _ = client.did_close(uri).await;

        match result {
            Some(workspace_edit) => {
                let mut output = format!("Rename to '{}' will affect:\n", args.new_name);
                let mut total_changes = 0;

                if let Some(changes) = workspace_edit.changes {
                    for (uri, edits) in changes {
                        output.push_str(&format!("\n{}:\n", uri.path()));
                        for edit in edits {
                            total_changes += 1;
                            output.push_str(&format!(
                                "  Line {}, Column {}: {}\n",
                                edit.range.start.line + 1,
                                edit.range.start.character + 1,
                                edit.new_text
                            ));
                        }
                    }
                }

                if let Some(document_changes) = workspace_edit.document_changes {
                    match document_changes {
                        lsp_types::DocumentChanges::Edits(edits) => {
                            for edit in edits {
                                output.push_str(&format!("\n{}:\n", edit.text_document.uri.path()));
                                for e in edit.edits {
                                    total_changes += 1;
                                    match e {
                                        lsp_types::OneOf::Left(text_edit) => {
                                            output.push_str(&format!(
                                                "  Line {}, Column {}: {}\n",
                                                text_edit.range.start.line + 1,
                                                text_edit.range.start.character + 1,
                                                text_edit.new_text
                                            ));
                                        }
                                        lsp_types::OneOf::Right(annotated) => {
                                            output.push_str(&format!(
                                                "  Line {}, Column {}: {}\n",
                                                annotated.text_edit.range.start.line + 1,
                                                annotated.text_edit.range.start.character + 1,
                                                annotated.text_edit.new_text
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                        lsp_types::DocumentChanges::Operations(_) => {
                            output.push_str("(Contains complex document operations)\n");
                        }
                    }
                }

                output.insert_str(0, &format!("Total changes: {}\n\n", total_changes));
                Ok(output)
            }
            None => Ok("Rename operation not available (symbol cannot be renamed).".to_string()),
        }
    }
}
