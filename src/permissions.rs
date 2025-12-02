use std::collections::HashSet;

/// Manages tool permissions for batch mode
#[derive(Debug, Clone)]
pub struct GrantedPermissions {
    /// Set of granted tool names
    tools: HashSet<String>,
    /// If true, all tools are granted
    all_granted: bool,
    /// If true, skip all confirmations (implies all_granted)
    skip_confirmations: bool,
    /// If true, only simulate tool execution (dry-run mode)
    dry_run: bool,
}

impl GrantedPermissions {
    /// Create a new GrantedPermissions with specific tools granted
    pub fn new(granted_tools: Vec<String>, grant_all: bool, yes: bool, dry_run: bool) -> Self {
        let all_granted = grant_all || yes;
        let skip_confirmations = yes;

        let mut tools = HashSet::new();
        for tool in granted_tools {
            tools.insert(tool.trim().to_lowercase());
        }

        Self {
            tools,
            all_granted,
            skip_confirmations,
            dry_run,
        }
    }

    /// Create a GrantedPermissions that grants all tools (for interactive mode)
    pub fn allow_all() -> Self {
        Self {
            tools: HashSet::new(),
            all_granted: true,
            skip_confirmations: false,
            dry_run: false,
        }
    }

    /// Check if a tool is granted permission
    pub fn is_granted(&self, tool_name: &str) -> bool {
        if self.all_granted {
            return true;
        }

        // Normalize tool name to lowercase for comparison
        let normalized = tool_name.to_lowercase();

        // Check if tool is in the granted set
        self.tools.contains(&normalized)
    }

    /// Check if confirmations should be skipped
    pub fn should_skip_confirmations(&self) -> bool {
        self.skip_confirmations
    }

    /// Check if we're in dry-run mode
    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// Get a summary of granted permissions for display
    pub fn summary(&self) -> String {
        if self.all_granted {
            "All tools granted".to_string()
        } else if self.tools.is_empty() {
            "No tools granted".to_string()
        } else {
            let mut tools_list: Vec<_> = self.tools.iter().map(|s| s.as_str()).collect();
            tools_list.sort();
            format!("Granted tools: {}", tools_list.join(", "))
        }
    }
}

/// Define tool categories for convenience
pub const READ_ONLY_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "grep",
    "glob",
    "bash_status",
    "bash_output",
    "bash_list",
    "web_fetch",
    "web_search",
    "search_routine_memory",
    "search_key_memory",
];

pub const WRITE_TOOLS: &[&str] = &[
    "write_file",
    "edit_file",
    "store_key_memory",
];

pub const EXECUTE_TOOLS: &[&str] = &[
    "bash",
    "bash_kill",
];

pub const RUST_ANALYZER_TOOLS: &[&str] = &[
    "ra_diagnostics",
    "ra_goto_definition",
    "ra_find_references",
    "ra_hover",
    "ra_symbols",
    "ra_completion",
    "ra_code_actions",
    "ra_rename",
    "ra_format",
];

/// Expand tool categories to individual tool names
pub fn expand_tool_categories(grants: Vec<String>) -> Vec<String> {
    let mut expanded = Vec::new();

    for grant in grants {
        let grant = grant.trim().to_lowercase();
        match grant.as_str() {
            "read-only" | "readonly" | "read" => {
                expanded.extend(READ_ONLY_TOOLS.iter().map(|s| s.to_string()));
            }
            "write" => {
                expanded.extend(WRITE_TOOLS.iter().map(|s| s.to_string()));
            }
            "execute" | "exec" | "bash" => {
                expanded.extend(EXECUTE_TOOLS.iter().map(|s| s.to_string()));
            }
            "rust-analyzer" | "ra" => {
                expanded.extend(RUST_ANALYZER_TOOLS.iter().map(|s| s.to_string()));
            }
            "all" => {
                // Grant everything
                expanded.extend(READ_ONLY_TOOLS.iter().map(|s| s.to_string()));
                expanded.extend(WRITE_TOOLS.iter().map(|s| s.to_string()));
                expanded.extend(EXECUTE_TOOLS.iter().map(|s| s.to_string()));
                expanded.extend(RUST_ANALYZER_TOOLS.iter().map(|s| s.to_string()));
                expanded.push("spawn_agent".to_string());
                expanded.push("math_calc".to_string());
            }
            _ => {
                // Treat as individual tool name
                expanded.push(grant);
            }
        }
    }

    expanded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grant_specific_tools() {
        let perms = GrantedPermissions::new(
            vec!["read_file".to_string(), "bash".to_string()],
            false,
            false,
            false,
        );

        assert!(perms.is_granted("read_file"));
        assert!(perms.is_granted("bash"));
        assert!(!perms.is_granted("write_file"));
    }

    #[test]
    fn test_grant_all() {
        let perms = GrantedPermissions::new(vec![], true, false, false);

        assert!(perms.is_granted("read_file"));
        assert!(perms.is_granted("write_file"));
        assert!(perms.is_granted("bash"));
    }

    #[test]
    fn test_expand_categories() {
        let expanded = expand_tool_categories(vec!["read-only".to_string()]);
        assert!(expanded.contains(&"read_file".to_string()));
        assert!(expanded.contains(&"grep".to_string()));
        assert!(!expanded.contains(&"write_file".to_string()));
    }
}
