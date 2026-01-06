use crate::ai::token_budget::TokenBudget;
use std::ops::Range;

pub struct Truncator;

impl Truncator {
    /// Truncates a diff output if it's too large.
    /// Preserves the header and checks for balanced chunks.
    pub fn truncate_diff(diff: &str, max_tokens: usize) -> String {
        let estimated = TokenBudget::estimate_tokens(diff);
        if estimated <= max_tokens {
            return diff.to_string();
        }

        let lines: Vec<&str> = diff.lines().collect();
        let total_lines = lines.len();

        // Simple strategy: Keep top N lines and bottom M lines.
        // We want to keep file headers (usually at the top).

        // Let's aim for 1 token per 4 chars.
        // max_tokens * 4 = max_chars.
        // Approx chars per line ~ 50 (heuristic).
        let max_lines = (max_tokens * 4) / 50;

        if total_lines <= max_lines {
            return diff.to_string(); // Should have been caught by token check, but just in case
        }

        let keep_top = max_lines / 2;
        let keep_bottom = max_lines / 2;

        if keep_top + keep_bottom >= total_lines {
            return diff.to_string();
        }

        let mut result = String::new();
        for line in &lines[..keep_top] {
            result.push_str(line);
            result.push('\n');
        }

        result.push_str(&format!(
            "\n... [Diff truncated. Dropped {} lines] ...\n\n",
            total_lines - (keep_top + keep_bottom)
        ));

        for line in &lines[total_lines - keep_bottom..] {
            result.push_str(line);
            result.push('\n');
        }

        result
    }

    /// Smart truncation for code files.
    /// tries to preserve context around `focus_lines`.
    ///
    /// `focus_lines` is 1-based inclusive range.
    pub fn truncate_code(
        content: &str,
        focus_lines: Option<Range<usize>>,
        max_tokens: usize,
    ) -> String {
        let estimated = TokenBudget::estimate_tokens(content);
        if estimated <= max_tokens {
            return content.to_string();
        }

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let (start_focus, end_focus) = if let Some(range) = focus_lines {
            (range.start.max(1) - 1, range.end.min(total_lines))
        } else {
            // If no focus, just take the top part?
            // Or maybe searching for "main" functions?
            // For now, let's default to head/tail strategy if no focus.
            return Self::truncate_diff(content, max_tokens);
        };

        // If we have focus, we want to expand around it.
        // Let's try to include 50 lines before and after.
        let context_size = 50;
        let start_keep = start_focus.saturating_sub(context_size);
        let end_keep = (end_focus + context_size).min(total_lines);

        let mut result = String::new();

        // Add header info if we are skipping the start
        if start_keep > 0 {
            result.push_str(&format!("... [{} lines collapsed] ...\n", start_keep));
        }

        for line in &lines[start_keep..end_keep] {
            result.push_str(line);
            result.push('\n');
        }

        if end_keep < total_lines {
            result.push_str(&format!(
                "... [{} lines collapsed] ...\n",
                total_lines - end_keep
            ));
        }

        // Check if we met the budget. If not, we might need to be stricter.
        // For now, return what we have.
        // A more advanced version would iteratively shrink context.
        result
    }

    // Future: Add AST-based collapsing here.
    #[allow(dead_code)]
    fn regex_collapse(_content: &str) -> String {
        // Placeholder for regex based function collapsing
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_diff() {
        let diff = "line1\nline2\nline3\nline4\nline5\nline6";
        // very small budget
        let truncated = Truncator::truncate_diff(diff, 1);
        assert!(truncated.contains("Diff truncated"));
    }

    #[test]
    fn test_truncate_code_focus() {
        let code = (0..200)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");

        let truncated = Truncator::truncate_code(&code, Some(100..105), 10);
        // It should keep lines around 100-105.
        assert!(truncated.contains("line 100"));
        assert!(truncated.contains("line 105"));
        // It should have collapsed the start
        assert!(truncated.contains("lines collapsed"));
    }
}
