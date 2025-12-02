use chrono::{DateTime, Local};
use std::collections::HashMap;
use std::env;
use std::path::Path;

use crate::git::GitInfo;

/// Context for template variable replacement
#[derive(Debug, Clone)]
pub struct TemplateContext {
    variables: HashMap<String, String>,
}

impl TemplateContext {
    /// Create a new template context with standard variables populated
    pub fn new(working_dir: &str, model: &str, agent_name: &str) -> Self {
        let mut variables = HashMap::new();

        // Time variables
        let now: DateTime<Local> = Local::now();
        variables.insert("date".to_string(), now.format("%Y-%m-%d").to_string());
        variables.insert("time".to_string(), now.format("%H:%M:%S %Z").to_string());
        variables.insert("datetime".to_string(), now.format("%Y-%m-%d %H:%M:%S %Z").to_string());
        variables.insert("timezone".to_string(), now.format("%Z").to_string());

        // System variables
        variables.insert("username".to_string(),
            env::var("USER")
                .or_else(|_| env::var("USERNAME"))
                .unwrap_or_else(|_| "unknown".to_string())
        );

        variables.insert("hostname".to_string(),
            hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_else(|| "unknown".to_string())
        );

        variables.insert("os".to_string(), std::env::consts::OS.to_string());
        variables.insert("platform".to_string(),
            format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
        );

        // Working directory and project
        variables.insert("working_dir".to_string(), working_dir.to_string());

        let project_name = Path::new(working_dir)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        variables.insert("project_name".to_string(), project_name);

        // Git information
        let git_info = GitInfo::detect(working_dir);
        variables.insert("git_branch".to_string(),
            git_info.branch.unwrap_or_default()
        );
        variables.insert("git_status".to_string(),
            if git_info.is_dirty { "dirty".to_string() } else { "clean".to_string() }
        );

        // Agent information
        variables.insert("model".to_string(), model.to_string());
        variables.insert("agent_name".to_string(), agent_name.to_string());

        Self { variables }
    }

    /// Add or override a custom variable
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.variables.insert(key.into(), value.into());
    }

    /// Get a variable value
    pub fn get(&self, key: &str) -> Option<&str> {
        self.variables.get(key).map(|s| s.as_str())
    }

    /// Render a template string by replacing {{variable}} patterns
    pub fn render(&self, template: &str) -> String {
        let mut result = template.to_string();

        // Replace all {{variable}} patterns
        for (key, value) in &self.variables {
            let pattern = format!("{{{{{}}}}}", key);
            result = result.replace(&pattern, value);
        }

        result
    }

    /// Get all variables as a HashMap (useful for debugging)
    pub fn variables(&self) -> &HashMap<String, String> {
        &self.variables
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_rendering() {
        let mut ctx = TemplateContext::new("/home/user/project", "qwen3-coder", "test-agent");

        // Test basic variable replacement
        let template = "Hello {{username}}, working in {{working_dir}}";
        let rendered = ctx.render(template);
        assert!(rendered.contains("working in /home/user/project"));

        // Test model variable
        assert_eq!(ctx.get("model"), Some("qwen3-coder"));

        // Test agent name
        assert_eq!(ctx.get("agent_name"), Some("test-agent"));

        // Test project name extraction
        assert_eq!(ctx.get("project_name"), Some("project"));
    }

    #[test]
    fn test_custom_variables() {
        let mut ctx = TemplateContext::new("/test", "test-model", "test-agent");
        ctx.set("custom_var", "custom_value");

        let template = "Custom: {{custom_var}}";
        let rendered = ctx.render(template);
        assert_eq!(rendered, "Custom: custom_value");
    }

    #[test]
    fn test_missing_variables() {
        let ctx = TemplateContext::new("/test", "test-model", "test-agent");

        // Missing variables should remain unchanged
        let template = "Missing: {{nonexistent}}";
        let rendered = ctx.render(template);
        assert_eq!(rendered, "Missing: {{nonexistent}}");
    }

    #[test]
    fn test_time_variables_exist() {
        let ctx = TemplateContext::new("/test", "test-model", "test-agent");

        // Just verify these exist and aren't empty
        assert!(ctx.get("date").is_some());
        assert!(ctx.get("time").is_some());
        assert!(ctx.get("datetime").is_some());
        assert!(ctx.get("timezone").is_some());

        assert!(!ctx.get("date").unwrap().is_empty());
    }
}
