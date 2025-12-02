use crate::error::ToolError;
use glob::glob;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Arguments for the GlobFiles tool
#[derive(Debug, Deserialize)]
pub struct GlobArgs {
    /// The glob pattern to match files
    pub pattern: String,
    /// Base directory for the search (defaults to current directory)
    pub base_dir: Option<String>,
}

/// Tool to find files matching a glob pattern
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct GlobFiles;

impl Tool for GlobFiles {
    const NAME: &'static str = "glob";
    type Error = ToolError;
    type Args = GlobArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Find files matching a glob pattern (e.g., '**/*.rs', 'src/**/*.ts'). Returns matching file paths.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "The glob pattern to match files (e.g., '**/*.rs', 'src/*.ts')"
                    },
                    "base_dir": {
                        "type": "string",
                        "description": "Base directory for the search (defaults to current directory)"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // Build the full pattern
        let full_pattern = match &args.base_dir {
            Some(base) => {
                let base = base.trim_end_matches('/');
                format!("{}/{}", base, args.pattern)
            }
            None => args.pattern.clone(),
        };

        // Execute glob (this is blocking, but typically fast)
        let entries = glob(&full_pattern).map_err(|e| ToolError::pattern_error(e.to_string()))?;

        let mut files: Vec<String> = Vec::new();
        for entry in entries {
            match entry {
                Ok(path) => {
                    files.push(path.display().to_string());
                }
                Err(e) => {
                    // Log but continue on individual errors
                    eprintln!("Glob entry error: {}", e);
                }
            }
        }

        // Sort for consistent output
        files.sort();

        if files.is_empty() {
            Ok(format!("No files matching pattern: {}", full_pattern))
        } else {
            let count = files.len();
            let mut result = files.join("\n");
            result.push_str(&format!("\n\n({} files found)", count));
            Ok(result)
        }
    }
}
