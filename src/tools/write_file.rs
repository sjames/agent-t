use crate::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use tokio::fs;

/// Arguments for the WriteFile tool
#[derive(Debug, Deserialize)]
pub struct WriteFileArgs {
    /// Absolute path to the file to write
    pub file_path: String,
    /// Content to write to the file
    pub content: String,
}

/// Tool to write content to a file (creates or overwrites)
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct WriteFile;

impl Tool for WriteFile {
    const NAME: &'static str = "write_file";
    type Error = ToolError;
    type Args = WriteFileArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Write content to a file. Creates the file if it doesn't exist, or overwrites if it does. Creates parent directories if needed.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "The absolute path to the file to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write to the file"
                    }
                },
                "required": ["file_path", "content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = Path::new(&args.file_path);

        // Create parent directories if they don't exist
        if let Some(parent) = path.parent()
            && !parent.exists() {
                fs::create_dir_all(parent).await.map_err(|e| {
                    if e.kind() == std::io::ErrorKind::PermissionDenied {
                        ToolError::permission_denied(parent.display().to_string())
                    } else {
                        ToolError::Io(e)
                    }
                })?;
            }

        // Write the file
        fs::write(path, &args.content).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                ToolError::permission_denied(&args.file_path)
            } else {
                ToolError::Io(e)
            }
        })?;

        let line_count = args.content.lines().count();
        let byte_count = args.content.len();

        Ok(format!(
            "Successfully wrote {} bytes ({} lines) to {}",
            byte_count, line_count, args.file_path
        ))
    }
}
