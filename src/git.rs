use std::path::Path;
use std::process::Command;

/// Git repository information
#[derive(Debug, Clone)]
pub struct GitInfo {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub is_dirty: bool,
    pub staged_count: usize,
    pub unstaged_count: usize,
    pub untracked_count: usize,
}

impl GitInfo {
    /// Detect git information for a directory
    pub fn detect(path: &str) -> Self {
        let path = Path::new(path);

        // Check if it's a git repository
        let is_repo = Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(path)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !is_repo {
            return Self {
                is_repo: false,
                branch: None,
                is_dirty: false,
                staged_count: 0,
                unstaged_count: 0,
                untracked_count: 0,
            };
        }

        // Get current branch
        let branch = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(path)
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    let branch = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if branch.is_empty() {
                        // Detached HEAD state - get short commit hash
                        Command::new("git")
                            .args(["rev-parse", "--short", "HEAD"])
                            .current_dir(path)
                            .output()
                            .ok()
                            .map(|o| format!("({})", String::from_utf8_lossy(&o.stdout).trim()))
                    } else {
                        Some(branch)
                    }
                } else {
                    None
                }
            });

        // Get status counts
        let (staged_count, unstaged_count, untracked_count) = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(path)
            .output()
            .map(|o| {
                if o.status.success() {
                    let output = String::from_utf8_lossy(&o.stdout);
                    let mut staged = 0;
                    let mut unstaged = 0;
                    let mut untracked = 0;

                    for line in output.lines() {
                        if line.len() >= 2 {
                            let status = &line[..2];
                            if status.starts_with('?') {
                                untracked += 1;
                            } else {
                                // First char is staged status, second is unstaged
                                if !status.starts_with(' ') && !status.starts_with('?') {
                                    staged += 1;
                                }
                                if status.chars().nth(1) != Some(' ') {
                                    unstaged += 1;
                                }
                            }
                        }
                    }
                    (staged, unstaged, untracked)
                } else {
                    (0, 0, 0)
                }
            })
            .unwrap_or((0, 0, 0));

        let is_dirty = staged_count > 0 || unstaged_count > 0 || untracked_count > 0;

        Self {
            is_repo: true,
            branch,
            is_dirty,
            staged_count,
            unstaged_count,
            untracked_count,
        }
    }

    /// Get a summary string for display
    pub fn summary(&self) -> String {
        if !self.is_repo {
            return "Not a git repository".to_string();
        }

        let branch = self.branch.as_deref().unwrap_or("unknown");
        let status = if self.is_dirty {
            let mut parts = Vec::new();
            if self.staged_count > 0 {
                parts.push(format!("{} staged", self.staged_count));
            }
            if self.unstaged_count > 0 {
                parts.push(format!("{} modified", self.unstaged_count));
            }
            if self.untracked_count > 0 {
                parts.push(format!("{} untracked", self.untracked_count));
            }
            format!(" ({})", parts.join(", "))
        } else {
            " (clean)".to_string()
        };

        format!("branch: {}{}", branch, status)
    }

    /// Get context string for LLM
    pub fn context_string(&self) -> String {
        if !self.is_repo {
            return String::new();
        }

        let branch = self.branch.as_deref().unwrap_or("unknown");
        let status = if self.is_dirty { "dirty" } else { "clean" };

        format!(
            "Git repository: branch '{}', status: {} ({} staged, {} modified, {} untracked files)",
            branch, status, self.staged_count, self.unstaged_count, self.untracked_count
        )
    }
}
