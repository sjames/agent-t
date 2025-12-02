//! Rust Analyzer format document tool

use crate::error::ToolError;
use crate::tools::ra_common;
use lsp_types::Url;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

/// Arguments for the RaFormat tool
#[derive(Debug, Deserialize)]
pub struct RaFormatArgs {
    /// File path
    pub file_path: String,
}

/// Tool to format a Rust document
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RaFormat;

impl Tool for RaFormat {
    const NAME: &'static str = "ra_format";
    type Error = ToolError;
    type Args = RaFormatArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Format a Rust file using rust-analyzer's formatter. Returns the formatting changes that would be applied.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the file to format"
                    }
                },
                "required": ["file_path"]
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

        // Get formatting edits
        let result = client.format(uri.clone()).await
            .map_err(|e| ToolError::Other(format!("Failed to format document: {}", e)))?;

        // Close the document
        let _ = client.did_close(uri).await;

        match result {
            Some(edits) if !edits.is_empty() => {
                let mut output = format!("Formatting will apply {} edit(s):\n", edits.len());
                for (i, edit) in edits.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. Line {}-{}, Column {}-{}\n",
                        i + 1,
                        edit.range.start.line + 1,
                        edit.range.end.line + 1,
                        edit.range.start.character + 1,
                        edit.range.end.character + 1
                    ));

                    // Show a preview of the change (truncated)
                    let preview = if edit.new_text.len() > 100 {
                        format!("{}...", &edit.new_text[..100])
                    } else {
                        edit.new_text.clone()
                    };
                    output.push_str(&format!("   New text: {}\n", preview));
                }
                Ok(output)
            }
            Some(_) => Ok("No formatting changes needed (file is already formatted).".to_string()),
            None => Ok("Formatting not available.".to_string()),
        }
    }
}
