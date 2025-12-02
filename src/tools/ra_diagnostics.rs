//! Rust Analyzer diagnostics tool

use crate::error::ToolError;
use crate::tools::ra_common;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Arguments for the RaDiagnostics tool
#[derive(Debug, Deserialize)]
pub struct RaDiagnosticsArgs {
    /// Optional file path to get diagnostics for (if not specified, returns all diagnostics)
    pub file_path: Option<String>,
}

/// Tool to get rust-analyzer diagnostics (errors and warnings)
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RaDiagnostics;

impl Tool for RaDiagnostics {
    const NAME: &'static str = "ra_diagnostics";
    type Error = ToolError;
    type Args = RaDiagnosticsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Get diagnostics (errors and warnings) from rust-analyzer for the current Rust project. Returns compiler errors, warnings, and other diagnostic messages.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Optional file path to get diagnostics for. If not specified, returns diagnostics for all files."
                    }
                },
                "required": []
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let client = ra_common::get_client().await?;
        let diagnostics = client.get_diagnostics().await;

        if diagnostics.is_empty() {
            return Ok("No diagnostics found. The project has no errors or warnings.".to_string());
        }

        let mut output = String::new();
        let mut total_errors = 0;
        let mut total_warnings = 0;

        for (uri, diags) in &diagnostics {
            // Filter by file path if specified
            if let Some(ref path_filter) = args.file_path
                && !uri.path().contains(path_filter) {
                    continue;
                }

            if diags.is_empty() {
                continue;
            }

            output.push_str(&format!("\n{}:\n", uri.path()));

            for diag in diags {
                let severity = match diag.severity {
                    Some(lsp_types::DiagnosticSeverity::ERROR) => {
                        total_errors += 1;
                        "ERROR"
                    }
                    Some(lsp_types::DiagnosticSeverity::WARNING) => {
                        total_warnings += 1;
                        "WARNING"
                    }
                    Some(lsp_types::DiagnosticSeverity::INFORMATION) => "INFO",
                    Some(lsp_types::DiagnosticSeverity::HINT) => "HINT",
                    _ => "UNKNOWN",
                };

                let start = diag.range.start;
                output.push_str(&format!(
                    "  [{}] Line {}, Column {}: {}\n",
                    severity,
                    start.line + 1,
                    start.character + 1,
                    diag.message
                ));

                // Include related information if available
                if let Some(ref related) = diag.related_information {
                    for info in related {
                        output.push_str(&format!(
                            "    Related: {} (Line {}): {}\n",
                            info.location.uri.path(),
                            info.location.range.start.line + 1,
                            info.message
                        ));
                    }
                }
            }
        }

        if output.is_empty() {
            Ok("No diagnostics found for the specified file path.".to_string())
        } else {
            output.insert_str(
                0,
                &format!(
                    "Diagnostics Summary: {} error(s), {} warning(s)\n",
                    total_errors, total_warnings
                ),
            );
            Ok(output)
        }
    }
}
