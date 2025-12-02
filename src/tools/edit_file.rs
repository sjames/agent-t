use crate::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use tokio::fs;

/// Arguments for the EditFile tool
#[derive(Debug, Deserialize)]
pub struct EditFileArgs {
    /// Absolute path to the file to edit
    pub file_path: String,
    /// The exact text to find and replace
    pub old_string: String,
    /// The text to replace it with
    pub new_string: String,
    /// Whether to replace all occurrences (default: false)
    pub replace_all: Option<bool>,
}

/// Tool to edit files by replacing text
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct EditFile;

impl Tool for EditFile {
    const NAME: &'static str = "edit_file";
    type Error = ToolError;
    type Args = EditFileArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Edit a file by replacing exact text matches. The old_string must match exactly (including whitespace and indentation).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "The absolute path to the file to edit"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The exact text to find and replace"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The text to replace it with"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Whether to replace all occurrences (default: false, replaces only first)"
                    }
                },
                "required": ["file_path", "old_string", "new_string"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = Path::new(&args.file_path);

        // Check if file exists
        if !path.exists() {
            return Err(ToolError::file_not_found(&args.file_path));
        }

        if !path.is_file() {
            return Err(ToolError::invalid_path(format!(
                "{} is not a file",
                args.file_path
            )));
        }

        // Read current contents
        let contents = fs::read_to_string(path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                ToolError::permission_denied(&args.file_path)
            } else {
                ToolError::Io(e)
            }
        })?;

        // Check if old_string exists in file
        if !contents.contains(&args.old_string) {
            return Err(ToolError::invalid_arguments(format!(
                "The string to replace was not found in {}. Make sure the old_string matches exactly, including whitespace.",
                args.file_path
            )));
        }

        // Perform replacement
        let (new_contents, count) = if args.replace_all.unwrap_or(false) {
            let count = contents.matches(&args.old_string).count();
            (contents.replace(&args.old_string, &args.new_string), count)
        } else {
            (contents.replacen(&args.old_string, &args.new_string, 1), 1)
        };

        // Write back
        fs::write(path, &new_contents).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                ToolError::permission_denied(&args.file_path)
            } else {
                ToolError::Io(e)
            }
        })?;

        Ok(format!(
            "Successfully replaced {} occurrence(s) in {}",
            count, args.file_path
        ))
    }
}
