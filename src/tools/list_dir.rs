use crate::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use tokio::fs;

/// Arguments for the ListDir tool
#[derive(Debug, Deserialize)]
pub struct ListDirArgs {
    /// Path to the directory to list
    pub path: String,
}

/// Tool to list directory contents
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ListDir;

impl Tool for ListDir {
    const NAME: &'static str = "list_dir";
    type Error = ToolError;
    type Args = ListDirArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "List the contents of a directory. Returns file and directory names with type indicators.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the directory to list"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = Path::new(&args.path);

        if !path.exists() {
            return Err(ToolError::file_not_found(&args.path));
        }

        if !path.is_dir() {
            return Err(ToolError::invalid_path(format!(
                "{} is not a directory",
                args.path
            )));
        }

        let mut entries = Vec::new();
        let mut read_dir = fs::read_dir(path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                ToolError::permission_denied(&args.path)
            } else {
                ToolError::Io(e)
            }
        })?;

        while let Some(entry) = read_dir.next_entry().await? {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let file_type = entry.file_type().await?;

            let type_indicator = if file_type.is_dir() {
                "/"
            } else if file_type.is_symlink() {
                "@"
            } else {
                ""
            };

            entries.push(format!("{}{}", file_name, type_indicator));
        }

        entries.sort();

        if entries.is_empty() {
            Ok("(empty directory)".to_string())
        } else {
            Ok(entries.join("\n"))
        }
    }
}
