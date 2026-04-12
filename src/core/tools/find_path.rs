/// Tool for finding files by name within the workspace
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use tokio::fs;
use tracing::{debug, error, info};

use super::types::{ToolCall, ToolDefinition, ToolResult};

pub struct FindPathTool {
    workspace_root: PathBuf,
}

impl FindPathTool {
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
            "find_path",
            "Find files by name within a directory tree. Searches recursively for files matching the given name pattern. Unlike glob which matches by pattern, find_path matches by exact or partial filename.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "File name to search for (can include * for wildcard matching)"
                    },
                    "path": {
                        "type": "string",
                        "description": "Base directory to search in (relative to workspace root, default: '.')"
                    },
                    "case_sensitive": {
                        "type": "boolean",
                        "description": "Whether the search is case sensitive (default: true)"
                    }
                },
                "required": ["name"]
            }),
        )
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        let name = call
            .arguments
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let path = call
            .arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        let case_sensitive = call
            .arguments
            .get("case_sensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        debug!(
            "find_path: name={}, path={}, case_sensitive={}",
            name, path, case_sensitive
        );

        if name.is_empty() {
            return ToolResult::error("find_path", "name is required");
        }

        let base_path = match self.validate_base_path(path) {
            Ok(p) => p,
            Err(e) => {
                error!("find_path: path validation failed: {}", e);
                return ToolResult::error("find_path", &e);
            }
        };

        // Convert name pattern to glob pattern if it contains wildcards
        let pattern = if name.contains('*') || name.contains('?') {
            name.to_string()
        } else {
            format!("*{}*", name)
        };

        let matches = self.walk_find(&base_path, &pattern, case_sensitive).await;

        match matches {
            Ok(files) => {
                let count = files.len();
                info!("find_path: found {} matching files", count);
                if files.is_empty() {
                    ToolResult::success("find_path", "No files match the name")
                } else {
                    ToolResult::success(
                        "find_path",
                        &format!("Found {} file(s):\n\n{}", count, files.join("\n")),
                    )
                }
            }
            Err(e) => {
                error!("find_path: failed to search: {}", e);
                ToolResult::error("find_path", &format!("Failed to search: {}", e))
            }
        }
    }

    async fn walk_find(
        &self,
        base_path: &PathBuf,
        pattern: &str,
        case_sensitive: bool,
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
                }

                if Self::name_matches(&file_name, pattern, case_sensitive) {
                    results.push(full_path);
                }
            }
        }

        results.sort();
        results.dedup();
        Ok(results)
    }

    fn name_matches(name: &str, pattern: &str, case_sensitive: bool) -> bool {
        let name_lower = if case_sensitive {
            name.to_string()
        } else {
            name.to_lowercase()
        };

        let pattern_lower = if case_sensitive {
            pattern.to_string()
        } else {
            pattern.to_lowercase()
        };

        // Convert glob pattern to simple matching
        let pattern_clean = pattern_lower.trim_start_matches('*').trim_end_matches('*');

        if pattern.starts_with('*') && pattern.ends_with('*') {
            name_lower.contains(&pattern_clean)
        } else if pattern.starts_with('*') {
            name_lower.ends_with(&pattern_clean)
        } else if pattern.ends_with('*') {
            name_lower.starts_with(&pattern_clean)
        } else {
            name_lower == pattern_lower
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(name: &str, path: &str, case_sensitive: bool) -> ToolCall {
        ToolCall {
            name: "find_path".to_string(),
            arguments: serde_json::json!({
                "name": name,
                "path": path,
                "case_sensitive": case_sensitive
            }),
        }
    }

    #[tokio::test]
    async fn find_path_finds_exact_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file1.txt"), "content1").unwrap();
        std::fs::write(dir.path().join("file2.txt"), "content2").unwrap();

        let tool = FindPathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("file1.txt", ".", true)).await;

        assert!(result.success);
        assert!(result.output.contains("file1.txt"));
        assert!(!result.output.contains("file2.txt"));
    }

    #[tokio::test]
    async fn find_path_finds_with_wildcard() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file1.txt"), "content1").unwrap();
        std::fs::write(dir.path().join("file2.txt"), "content2").unwrap();

        let tool = FindPathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("*.txt", ".", true)).await;

        assert!(result.success);
        assert!(result.output.contains("file1.txt"));
        assert!(result.output.contains("file2.txt"));
    }

    #[tokio::test]
    async fn find_path_finds_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        std::fs::write(dir.path().join("root.txt"), "root").unwrap();
        std::fs::write(subdir.join("nested.txt"), "nested").unwrap();

        let tool = FindPathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("*.txt", ".", true)).await;

        assert!(result.success);
        assert!(result.output.contains("root.txt"));
        assert!(result.output.contains("subdir/nested.txt"));
    }

    #[tokio::test]
    async fn find_path_case_insensitive_search() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("File1.txt"), "content1").unwrap();
        std::fs::write(dir.path().join("FILE2.txt"), "content2").unwrap();

        let tool = FindPathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("file1.txt", ".", false)).await;

        assert!(result.success);
        assert!(result.output.contains("File1.txt"));
    }

    #[tokio::test]
    async fn find_path_returns_empty_for_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), "content").unwrap();

        let tool = FindPathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("nonexistent.txt", ".", true)).await;

        assert!(result.success);
        assert!(result.output.contains("No files match"));
    }

    #[tokio::test]
    async fn find_path_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let tool = FindPathTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_call("file.txt", "../outside", true))
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("traversal"));
    }

    #[tokio::test]
    async fn find_path_rejects_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let tool = FindPathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("file.txt", "/etc", true)).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Absolute paths"));
    }

    #[tokio::test]
    async fn find_path_partial_name_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test_file.txt"), "content").unwrap();
        std::fs::write(dir.path().join("other.txt"), "content").unwrap();

        let tool = FindPathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("test", ".", true)).await;

        assert!(result.success);
        assert!(result.output.contains("test_file.txt"));
        assert!(!result.output.contains("other.txt"));
    }
}
