use crate::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Arguments for the BashKill tool
#[derive(Debug, Deserialize)]
pub struct BashKillArgs {
    /// Process ID to kill
    pub process_id: String,
}

/// Tool to kill a background process
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct BashKill;

impl Tool for BashKill {
    const NAME: &'static str = "bash_kill";
    type Error = ToolError;
    type Args = BashKillArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Terminate a background bash process. Use this to stop a long-running process.".to_string(),
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
        crate::process_manager::PROCESS_MANAGER
            .kill_process(&args.process_id)
            .await
            .map_err(ToolError::Other)?;

        Ok(format!(
            "Process {} termination signal sent. Use bash_status to verify termination.",
            args.process_id
        ))
    }
}
