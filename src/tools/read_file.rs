use crate::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use tokio::fs;

/// Arguments for the ReadFile tool
#[derive(Debug, Deserialize)]
pub struct ReadFileArgs {
    /// Absolute path to the file to read
    pub file_path: String,
    /// Optional starting line number (1-indexed)
    pub offset: Option<usize>,
    /// Optional number of lines to read
    pub limit: Option<usize>,
}

/// Tool to read file contents
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ReadFile;

impl Tool for ReadFile {
    const NAME: &'static str = "read_file";
    type Error = ToolError;
    type Args = ReadFileArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read the contents of a file. Returns the file content with line numbers.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "The absolute path to the file to read"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Optional starting line number (1-indexed). If not provided, starts from the beginning."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Optional number of lines to read. If not provided, reads the entire file."
                    }
                },
                "required": ["file_path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = Path::new(&args.file_path);

        // Check if file exists
        if !path.exists() {
            return Err(ToolError::file_not_found(&args.file_path));
        }

        // Check if it's a file (not a directory)
        if !path.is_file() {
            return Err(ToolError::invalid_path(format!(
                "{} is not a file",
                args.file_path
            )));
        }

        // Read file contents
        let contents = fs::read_to_string(path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                ToolError::permission_denied(&args.file_path)
            } else {
                ToolError::Io(e)
            }
        })?;

        // Apply offset and limit
        let lines: Vec<&str> = contents.lines().collect();
        let total_lines = lines.len();

        let start = args.offset.unwrap_or(1).saturating_sub(1);
        let end = args
            .limit
            .map(|l| (start + l).min(total_lines))
            .unwrap_or(total_lines);

        // Format with line numbers
        let mut output = String::new();
        for (idx, line) in lines.iter().enumerate().skip(start).take(end - start) {
            output.push_str(&format!("{:>6}\t{}\n", idx + 1, line));
        }

        if output.is_empty() {
            output = format!("(empty file or no lines in range {}-{})", start + 1, end);
        }

        Ok(output)
    }
}
