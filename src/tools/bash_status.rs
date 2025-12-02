use crate::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Arguments for the BashStatus tool
#[derive(Debug, Deserialize)]
pub struct BashStatusArgs {
    /// Process ID to check
    pub process_id: String,
}

/// Tool to check status of a background process
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct BashStatus;

impl Tool for BashStatus {
    const NAME: &'static str = "bash_status";
    type Error = ToolError;
    type Args = BashStatusArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Check the status of a background bash process. Returns whether the process is running, completed, or failed.".to_string(),
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
                let status_str = match info.status {
                    crate::process_manager::ProcessStatus::Running => "Running",
                    crate::process_manager::ProcessStatus::Completed => "Completed",
                    crate::process_manager::ProcessStatus::Failed => "Failed",
                };

                let mut result = format!(
                    "Process ID: {}\nCommand: {}\nStatus: {}\nStart time: {}",
                    info.id,
                    info.command,
                    status_str,
                    info.start_time.format("%Y-%m-%d %H:%M:%S UTC")
                );

                if let Some(code) = info.exit_code {
                    result.push_str(&format!("\nExit code: {}", code));
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
