use similar::{ChangeTag, TextDiff};

/// Represents a unified diff with line numbers for display
#[derive(Debug, Clone)]
pub struct UnifiedDiff {
    pub file_path: String,
    pub lines: Vec<DiffLine>,
}

/// A single line in a unified diff
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub old_line_num: Option<usize>,
    pub new_line_num: Option<usize>,
    pub change_type: DiffChangeType,
    pub content: String,
}

/// Type of change in a diff line
#[derive(Debug, Clone, PartialEq)]
pub enum DiffChangeType {
    Context,  // Unchanged line
    Addition, // Added line
    Deletion, // Removed line
}

impl UnifiedDiff {
    /// Generate a unified diff between two texts
    pub fn from_texts(file_path: String, old_text: &str, new_text: &str) -> Self {
        let diff = TextDiff::from_lines(old_text, new_text);
        let mut lines = Vec::new();

        let mut old_line = 1;
        let mut new_line = 1;

        for change in diff.iter_all_changes() {
            let change_type = match change.tag() {
                ChangeTag::Equal => DiffChangeType::Context,
                ChangeTag::Insert => DiffChangeType::Addition,
                ChangeTag::Delete => DiffChangeType::Deletion,
            };

            let (old_num, new_num) = match change.tag() {
                ChangeTag::Equal => {
                    let nums = (Some(old_line), Some(new_line));
                    old_line += 1;
                    new_line += 1;
                    nums
                }
                ChangeTag::Insert => {
                    let nums = (None, Some(new_line));
                    new_line += 1;
                    nums
                }
                ChangeTag::Delete => {
                    let nums = (Some(old_line), None);
                    old_line += 1;
                    nums
                }
            };

            // Remove trailing newline from content for display
            let content = change.value().trim_end_matches('\n').to_string();

            lines.push(DiffLine {
                old_line_num: old_num,
                new_line_num: new_num,
                change_type,
                content,
            });
        }

        UnifiedDiff { file_path, lines }
    }

    /// Get a summary of the changes (e.g., "+5, -3")
    pub fn summary(&self) -> String {
        let additions = self
            .lines
            .iter()
            .filter(|l| l.change_type == DiffChangeType::Addition)
            .count();
        let deletions = self
            .lines
            .iter()
            .filter(|l| l.change_type == DiffChangeType::Deletion)
            .count();

        format!("+{}, -{}", additions, deletions)
    }

    /// Check if the diff contains any changes
    pub fn has_changes(&self) -> bool {
        self.lines
            .iter()
            .any(|l| l.change_type != DiffChangeType::Context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_diff() {
        let old = "line 1\nline 2\nline 3\n";
        let new = "line 1\nline 2 modified\nline 3\n";

        let diff = UnifiedDiff::from_texts("test.txt".to_string(), old, new);

        assert!(diff.has_changes());
        assert_eq!(diff.summary(), "+1, -1");
    }

    #[test]
    fn test_addition_only() {
        let old = "line 1\n";
        let new = "line 1\nline 2\n";

        let diff = UnifiedDiff::from_texts("test.txt".to_string(), old, new);

        assert_eq!(diff.summary(), "+1, -0");
    }

    #[test]
    fn test_deletion_only() {
        let old = "line 1\nline 2\n";
        let new = "line 1\n";

        let diff = UnifiedDiff::from_texts("test.txt".to_string(), old, new);

        assert_eq!(diff.summary(), "+0, -1");
    }
}
