use crate::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

/// Arguments for the BashCommand tool
#[derive(Debug, Deserialize)]
pub struct BashArgs {
    /// The command to execute
    pub command: String,
    /// Optional working directory
    pub working_dir: Option<String>,
    /// Optional timeout in seconds (default: 600)
    pub timeout_secs: Option<u64>,
    /// Execute in background (default: false)
    pub background: Option<bool>,
}

/// Tool to execute bash commands
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct BashCommand;

impl Tool for BashCommand {
    const NAME: &'static str = "bash";
    type Error = ToolError;
    type Args = BashArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Execute a bash command and return the output. Use this for running shell commands, git operations, build tools, etc. Can execute in background for long-running commands.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The bash command to execute"
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Optional working directory for the command"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Optional timeout in seconds (default: 600, max recommended: 1800). Increase this for long-running operations like builds, tests, or installations. Ignored if background=true."
                    },
                    "background": {
                        "type": "boolean",
                        "description": "Execute in background and return immediately with process ID. Use bash_status/bash_output tools to check progress."
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // Check if background execution is requested
        if args.background.unwrap_or(false) {
            // Use process manager for background execution
            let process_id = crate::process_manager::PROCESS_MANAGER
                .spawn_background(args.command.clone(), args.working_dir.clone())
                .await
                .map_err(ToolError::Other)?;

            return Ok(format!(
                "Background process started with ID: {}\nCommand: {}\nUse bash_status to check progress, bash_output to get output, or bash_kill to terminate.",
                process_id, args.command
            ));
        }

        // Foreground execution (original behavior)
        let timeout_duration = Duration::from_secs(args.timeout_secs.unwrap_or(600));

        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(&args.command);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        if let Some(ref dir) = args.working_dir {
            cmd.current_dir(dir);
        }

        let output = timeout(timeout_duration, cmd.output())
            .await
            .map_err(|_| ToolError::CommandTimeout)?
            .map_err(ToolError::Io)?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut result = String::new();

        if !stdout.is_empty() {
            result.push_str(&stdout);
        }

        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push_str("\n--- stderr ---\n");
            }
            result.push_str(&stderr);
        }

        if result.is_empty() {
            result = "(no output)".to_string();
        }

        // Add exit code info if non-zero
        if !output.status.success() {
            let exit_code = output.status.code().unwrap_or(-1);
            result.push_str(&format!("\n[Exit code: {}]", exit_code));
        }

        Ok(result)
    }
}
