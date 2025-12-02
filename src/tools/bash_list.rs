use crate::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Arguments for the BashList tool (no arguments needed)
#[derive(Debug, Deserialize)]
pub struct BashListArgs {}

/// Tool to list all background processes
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct BashList;

impl Tool for BashList {
    const NAME: &'static str = "bash_list";
    type Error = ToolError;
    type Args = BashListArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "List all background bash processes with their IDs, commands, and status.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let processes = crate::process_manager::PROCESS_MANAGER.list_processes().await;

        if processes.is_empty() {
            return Ok("No background processes running.".to_string());
        }

        let mut result = format!("Background processes ({})\n\n", processes.len());

        for info in processes {
            let status_str = match info.status {
                crate::process_manager::ProcessStatus::Running => "Running",
                crate::process_manager::ProcessStatus::Completed => "Completed",
                crate::process_manager::ProcessStatus::Failed => "Failed",
            };

            result.push_str(&format!(
                "ID: {}\nCommand: {}\nStatus: {}\nStarted: {}\n",
                info.id,
                info.command,
                status_str,
                info.start_time.format("%Y-%m-%d %H:%M:%S UTC")
            ));

            if let Some(code) = info.exit_code {
                result.push_str(&format!("Exit code: {}\n", code));
            }

            result.push('\n');
        }

        Ok(result)
    }
}
