use crate::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Arguments for the BashOutput tool
#[derive(Debug, Deserialize)]
pub struct BashOutputArgs {
    /// Process ID to get output from
    pub process_id: String,
}

/// Tool to get output from a background process
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct BashOutput;

impl Tool for BashOutput {
    const NAME: &'static str = "bash_output";
    type Error = ToolError;
    type Args = BashOutputArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Get the accumulated stdout and stderr output from a background bash process. Returns all output collected so far.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "process_id": {
                        "type": "string",
                        "description": "The process ID returned by bash command with background=true"
                    }
                },
                "required": ["process_id"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let process_info = crate::process_manager::PROCESS_MANAGER
            .get_process(&args.process_id)
            .await;

        match process_info {
            Some(info) => {
                let mut result = String::new();

                if !info.stdout.is_empty() {
                    result.push_str("=== STDOUT ===\n");
                    result.push_str(&info.stdout);
                    result.push('\n');
                }

                if !info.stderr.is_empty() {
                    result.push_str("=== STDERR ===\n");
                    result.push_str(&info.stderr);
                    result.push('\n');
                }

                if result.is_empty() {
                    result = "(no output yet)".to_string();
                }

                // Add status information
                let status_str = match info.status {
                    crate::process_manager::ProcessStatus::Running => "still running",
                    crate::process_manager::ProcessStatus::Completed => "completed",
                    crate::process_manager::ProcessStatus::Failed => "failed",
                };

                result.push_str(&format!("\n[Process {} - {}]", info.id, status_str));

                if let Some(code) = info.exit_code {
                    result.push_str(&format!(" [Exit code: {}]", code));
                }

                Ok(result)
            }
            None => Err(ToolError::Other(format!(
                "Process {} not found. It may have been cleaned up or the ID is incorrect.",
                args.process_id
            ))),
        }
    }
}
