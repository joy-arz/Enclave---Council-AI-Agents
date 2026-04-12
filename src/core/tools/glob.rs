/// Tool for finding files matching glob patterns
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, error, info};

use super::types::{ToolCall, ToolDefinition, ToolResult};

pub struct GlobTool {
    workspace_root: PathBuf,
}

impl GlobTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }

    fn validate_base_path(&self, path: &str) -> Result<PathBuf, String> {
        let path_obj = PathBuf::from(path);
        if path_obj.is_absolute() {
            return Err("Absolute paths not allowed".to_string());
        }
        if path_obj.components().any(|c| c.as_os_str() == "..") {
            return Err("Path traversal not allowed".to_string());
        }
        let full_path = self.workspace_root.join(path);
        let workspace_resolved = self
            .workspace_root
            .canonicalize()
            .map_err(|e| format!("Workspace resolution failed: {}", e))?;

        if full_path.exists() {
            let resolved = full_path
                .canonicalize()
                .map_err(|e| format!("Path resolution failed: {}", e))?;
            if !resolved.starts_with(&workspace_resolved) {
                return Err("Path escapes workspace".to_string());
            }
            Ok(resolved)
        } else {
            Ok(full_path)
        }
    }

    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "glob",
            "Find files matching a glob pattern within a directory. Supports ** for matching any path, * for matching within a directory, and ? for single character matches.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match (e.g., '**/*.rs', 'src/**/*.js', '*.txt')"
                    },
                    "path": {
                        "type": "string",
                        "description": "Base directory to search in (relative to workspace root, default: '.')"
                    }
                },
                "required": ["pattern"]
            }),
        )
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        let pattern = call
            .arguments
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let path = call
            .arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        debug!("glob: pattern={}, path={}", pattern, path);

        if pattern.is_empty() {
            return ToolResult::error("glob", "pattern is required");
        }

        let base_path = match self.validate_base_path(path) {
            Ok(p) => p,
            Err(e) => {
                error!("glob: path validation failed: {}", e);
                return ToolResult::error("glob", &e);
            }
        };

        // Convert glob pattern to glob pattern for walking
        let matches = self.walk_glob(&base_path, pattern).await;

        match matches {
            Ok(files) => {
                let count = files.len();
                info!("glob: found {} matching files", count);
                if files.is_empty() {
                    ToolResult::success("glob", "No files match the pattern")
                } else {
                    ToolResult::success(
                        "glob",
                        &format!("Found {} file(s):\n\n{}", count, files.join("\n")),
                    )
                }
            }
            Err(e) => {
                error!("glob: failed to search: {}", e);
                ToolResult::error("glob", &format!("Failed to search: {}", e))
            }
        }
    }

    async fn walk_glob(&self, base_path: &PathBuf, pattern: &str) -> Result<Vec<String>, String> {
        let mut results = Vec::new();

        // Handle pattern splitting for **/* patterns
        let parts: Vec<&str> = pattern.split("**").collect();

        if parts.len() > 1 {
            // Complex pattern with ** - use iterative search
            results = self.walk_glob_iterative(base_path, pattern).await?;
        } else {
            // Simple pattern - do direct matching
            results = self.walk_simple_glob(base_path, pattern).await?;
        }

        // Sort results
        results.sort();
        results.dedup();

        Ok(results)
    }

    async fn walk_glob_iterative(
        &self,
        base_path: &PathBuf,
        pattern: &str,
    ) -> Result<Vec<String>, String> {
        let mut results = Vec::new();
        let mut stack: Vec<(PathBuf, String)> = vec![(base_path.clone(), String::new())];

        while let Some((current_path, current_prefix)) = stack.pop() {
            let entries = fs::read_dir(&current_path)
                .await
                .map_err(|e| format!("Failed to read directory: {}", e))?;

            let mut entries = entries;
            while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
                let path = entry.path();
                let file_name = entry.file_name().to_string_lossy().to_string();

                let full_path = if current_prefix.is_empty() {
                    file_name.clone()
                } else {
                    format!("{}/{}", current_prefix, file_name)
                };

                if path.is_dir() {
                    // Push directory onto stack for iterative traversal
                    stack.push((path, full_path.clone()));

                    // Also check if current path matches
                    if Self::pattern_matches(&full_path, pattern) {
                        results.push(full_path);
                    }
                } else {
                    if Self::pattern_matches(&full_path, pattern) {
                        results.push(full_path);
                    }
                }
            }
        }

        Ok(results)
    }

    async fn walk_simple_glob(
        &self,
        base_path: &PathBuf,
        pattern: &str,
    ) -> Result<Vec<String>, String> {
        let mut results = Vec::new();
        let mut stack: Vec<(PathBuf, String)> = vec![(base_path.clone(), String::new())];

        while let Some((current_path, current_prefix)) = stack.pop() {
            let entries = fs::read_dir(&current_path)
                .await
                .map_err(|e| format!("Failed to read directory: {}", e))?;

            let mut entries = entries;
            while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
                let path = entry.path();
                let file_name = entry.file_name().to_string_lossy().to_string();

                let full_path = if current_prefix.is_empty() {
                    file_name.clone()
                } else {
                    format!("{}/{}", current_prefix, file_name)
                };

                if path.is_dir() {
                    stack.push((path, full_path.clone()));
                } else if Self::pattern_matches(&file_name, pattern) {
                    results.push(full_path);
                }
            }
        }

        Ok(results)
    }

    fn pattern_matches(path: &str, pattern: &str) -> bool {
        // Simple glob matching
        // * matches anything except /
        // ** matches anything including /
        // ? matches single character except /

        let path_parts: Vec<&str> = path.split('/').collect();

        // Convert glob pattern to regex-like matching
        let pattern_parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();

        Self::match_parts(&path_parts, &pattern_parts, 0, 0)
    }

    fn match_parts(
        path_parts: &[&str],
        pattern_parts: &[&str],
        p_idx: usize,
        pat_idx: usize,
    ) -> bool {
        // If we've matched all pattern parts, everything remaining in path should not be a directory
        // (unless there's more pattern, which would be an error)

        if pat_idx >= pattern_parts.len() {
            // No more pattern parts - all remaining path parts should not require directory matching
            // But wait, if we have ** in the last pattern, it could match directories
            return p_idx >= path_parts.len();
        }

        if p_idx >= path_parts.len() {
            // No more path parts but still have pattern
            return pattern_parts[pat_idx] == "**" && pat_idx == pattern_parts.len() - 1;
        }

        let pat = pattern_parts[pat_idx];

        if pat == "**" {
            // ** can match zero or more directories
            // Try matching with ** consuming this path part
            if Self::match_parts(path_parts, pattern_parts, p_idx + 1, pat_idx) {
                return true;
            }
            // Or try ** consuming nothing (moving to next pattern)
            if pat_idx + 1 < pattern_parts.len() {
                return Self::match_parts(path_parts, pattern_parts, p_idx, pat_idx + 1);
            }
            return false;
        }

        if pat == "*" {
            // * matches anything except /
            return Self::match_parts(path_parts, pattern_parts, p_idx + 1, pat_idx + 1);
        }

        if pat == "?" {
            // ? matches single character
            let part = path_parts[p_idx];
            if part.len() == 1 && part.chars().next().unwrap() != '.' {
                return Self::match_parts(path_parts, pattern_parts, p_idx + 1, pat_idx + 1);
            }
            return false;
        }

        // Regular string match
        if pat.starts_with('*') || pat.ends_with('*') {
            // Partial glob
            if Self::glob_match(path_parts[p_idx], pat) {
                return Self::match_parts(path_parts, pattern_parts, p_idx + 1, pat_idx + 1);
            }
            return false;
        }

        // Exact match
        if path_parts[p_idx] == pat {
            return Self::match_parts(path_parts, pattern_parts, p_idx + 1, pat_idx + 1);
        }

        false
    }

    fn glob_match(text: &str, pattern: &str) -> bool {
        // Simple glob matching for a single component
        let pattern = pattern.trim_start_matches('*');
        let pattern = pattern.trim_end_matches('*');

        if pattern.contains('*') {
            // Complex pattern - convert to simple regex
            let mut regex_pattern = String::new();
            for ch in pattern.chars() {
                if ch == '?' {
                    regex_pattern.push('.');
                } else if ch == '*' {
                    regex_pattern.push_str(".*");
                } else if ch.is_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                    regex_pattern.push(ch);
                } else {
                    regex_pattern.push('\\');
                    regex_pattern.push(ch);
                }
            }
            text.matches(&glob_match_to_regex(&regex_pattern))
                .next()
                .is_some()
        } else {
            text == pattern
        }
    }
}

fn glob_match_to_regex(pattern: &str) -> String {
    let mut result = String::from("^");
    let mut chars = pattern.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '.' => result.push_str("\\."),
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    result.push_str(".*");
                } else {
                    result.push_str("[^/]*");
                }
            }
            '?' => result.push('.'),
            '[' => {
                result.push('[');
                while let Some(&c) = chars.peek() {
                    if c == ']' {
                        break;
                    }
                    result.push(c);
                    chars.next();
                }
                if chars.peek() == Some(&']') {
                    result.push(']');
                    chars.next();
                }
            }
            _ => result.push(ch),
        }
    }

    result.push('$');
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(pattern: &str, path: &str) -> ToolCall {
        ToolCall {
            name: "glob".to_string(),
            arguments: serde_json::json!({
                "pattern": pattern,
                "path": path
            }),
        }
    }

    #[tokio::test]
    async fn glob_finds_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file1.txt"), "content1").unwrap();
        std::fs::write(dir.path().join("file2.txt"), "content2").unwrap();
        std::fs::write(dir.path().join("file3.md"), "content3").unwrap();

        let tool = GlobTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("*.txt", ".")).await;

        assert!(result.success);
        assert!(result.output.contains("file1.txt"));
        assert!(result.output.contains("file2.txt"));
        assert!(!result.output.contains("file3.md"));
    }

    #[tokio::test]
    async fn glob_finds_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        std::fs::write(dir.path().join("root.txt"), "root").unwrap();
        std::fs::write(subdir.join("nested.txt"), "nested").unwrap();

        let tool = GlobTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("**/*.txt", ".")).await;

        assert!(result.success);
        assert!(result.output.contains("root.txt"));
        assert!(result.output.contains("subdir/nested.txt"));
    }

    #[tokio::test]
    async fn glob_returns_empty_for_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), "content").unwrap();

        let tool = GlobTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("*.rs", ".")).await;

        assert!(result.success);
        assert!(result.output.contains("No files match"));
    }

    #[tokio::test]
    async fn glob_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let tool = GlobTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("*.txt", "../outside")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("traversal"));
    }

    #[tokio::test]
    async fn glob_rejects_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let tool = GlobTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("*.txt", "/etc")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Absolute paths"));
    }
}
