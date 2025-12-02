use crate::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::process::Stdio;
use tokio::process::Command;

/// Arguments for the GrepSearch tool
#[derive(Debug, Deserialize)]
pub struct GrepArgs {
    /// The pattern to search for (supports regex)
    pub pattern: String,
    /// The path to search in (file or directory)
    pub path: Option<String>,
    /// Case insensitive search
    pub ignore_case: Option<bool>,
    /// Maximum number of results to return
    pub max_results: Option<usize>,
}

/// Tool to search for patterns in files
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct GrepSearch;

impl Tool for GrepSearch {
    const NAME: &'static str = "grep";
    type Error = ToolError;
    type Args = GrepArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Search for a pattern in files using ripgrep (rg). Returns matching lines with file paths and line numbers.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "The regex pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "The file or directory to search in (defaults to current directory)"
                    },
                    "ignore_case": {
                        "type": "boolean",
                        "description": "Whether to ignore case (default: false)"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 50)"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // Try ripgrep first, fall back to grep
        let (cmd_name, use_rg) = if Command::new("rg")
            .arg("--version")
            .output()
            .await
            .is_ok()
        {
            ("rg", true)
        } else {
            ("grep", false)
        };

        let mut cmd = Command::new(cmd_name);

        if use_rg {
            cmd.arg("--line-number");
            cmd.arg("--color=never");

            if args.ignore_case.unwrap_or(false) {
                cmd.arg("--ignore-case");
            }

            if let Some(max) = args.max_results {
                cmd.arg("--max-count").arg(max.to_string());
            }

            cmd.arg(&args.pattern);

            if let Some(ref path) = args.path {
                cmd.arg(path);
            } else {
                cmd.arg(".");
            }
        } else {
            // Fallback to grep
            cmd.arg("-rn");

            if args.ignore_case.unwrap_or(false) {
                cmd.arg("-i");
            }

            cmd.arg(&args.pattern);

            if let Some(ref path) = args.path {
                cmd.arg(path);
            } else {
                cmd.arg(".");
            }
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let output = cmd.output().await.map_err(ToolError::Io)?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !stderr.is_empty() && !output.status.success() && output.status.code() != Some(1) {
            return Err(ToolError::command_failed(stderr.to_string()));
        }

        if stdout.is_empty() {
            Ok("No matches found.".to_string())
        } else {
            // Limit results if needed
            let max = args.max_results.unwrap_or(50);
            let lines: Vec<&str> = stdout.lines().take(max).collect();
            let total_matches = stdout.lines().count();

            let mut result = lines.join("\n");
            if total_matches > max {
                result.push_str(&format!(
                    "\n\n... and {} more matches (showing first {})",
                    total_matches - max,
                    max
                ));
            }

            Ok(result)
        }
    }
}
