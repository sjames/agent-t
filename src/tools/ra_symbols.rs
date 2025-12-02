//! Rust Analyzer symbols tool

use crate::error::ToolError;
use crate::tools::ra_common;
use lsp_types::Url;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

/// Arguments for the RaSymbols tool
#[derive(Debug, Deserialize)]
pub struct RaSymbolsArgs {
    /// File path for document symbols, or omit for workspace symbols
    pub file_path: Option<String>,
    /// Query string for workspace symbol search (only used if file_path is not provided)
    pub query: Option<String>,
}

/// Tool to get document or workspace symbols
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RaSymbols;

impl Tool for RaSymbols {
    const NAME: &'static str = "ra_symbols";
    type Error = ToolError;
    type Args = RaSymbolsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Get symbols from a Rust file or search workspace symbols. If file_path is provided, returns document outline. Otherwise, searches workspace for symbols matching the query.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Optional file path to get document symbols"
                    },
                    "query": {
                        "type": "string",
                        "description": "Search query for workspace symbols (only used if file_path is not provided)"
                    }
                },
                "required": []
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let client = ra_common::get_client().await?;

        if let Some(file_path) = args.file_path {
            // Document symbols
            let path = PathBuf::from(&file_path);
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

            // Get document symbols
            let result = client.document_symbols(uri.clone()).await
                .map_err(|e| ToolError::Other(format!("Failed to get document symbols: {}", e)))?;

            // Close the document
            let _ = client.did_close(uri).await;

            match result {
                Some(symbols) if !symbols.is_empty() => {
                    let mut output = String::from("Document symbols:\n");
                    format_symbols(&symbols, &mut output, 0);
                    Ok(output)
                }
                _ => Ok("No symbols found in document.".to_string()),
            }
        } else {
            // Workspace symbols
            let query = args.query.unwrap_or_default();
            let result = client.workspace_symbols(query.clone()).await
                .map_err(|e| ToolError::Other(format!("Failed to search workspace symbols: {}", e)))?;

            match result {
                Some(symbols) if !symbols.is_empty() => {
                    let mut output = format!("Found {} workspace symbol(s):\n", symbols.len());
                    for (i, symbol) in symbols.iter().enumerate() {
                        output.push_str(&format!(
                            "{}. {} ({:?})\n   File: {}\n   Line: {}\n",
                            i + 1,
                            symbol.name,
                            symbol.kind,
                            symbol.location.uri.path(),
                            symbol.location.range.start.line + 1
                        ));
                    }
                    Ok(output)
                }
                _ => Ok(format!("No workspace symbols found for query: '{}'", query)),
            }
        }
    }
}

fn format_symbols(symbols: &[lsp_types::DocumentSymbol], output: &mut String, indent: usize) {
    for symbol in symbols {
        let indent_str = "  ".repeat(indent);
        output.push_str(&format!(
            "{}{} ({:?}) - Line {}\n",
            indent_str,
            symbol.name,
            symbol.kind,
            symbol.range.start.line + 1
        ));

        if let Some(ref children) = symbol.children {
            format_symbols(children, output, indent + 1);
        }
    }
}
