/// Tool for checking if a path exists within the workspace
use std::path::PathBuf;
use tracing::{debug, error};

use super::types::{ToolCall, ToolDefinition, ToolResult};

pub struct PathExistsTool {
    workspace_root: PathBuf,
}

impl PathExistsTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf, String> {
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

        // Check if parent exists and is within workspace
        if let Some(parent) = full_path.parent() {
            if parent.exists() {
                let resolved = parent
                    .canonicalize()
                    .map_err(|e| format!("Path resolution failed: {}", e))?;
                if !resolved.starts_with(&workspace_resolved) {
                    return Err("Path escapes workspace".to_string());
                }
            }
        }
        Ok(full_path)
    }

    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "path_exists",
            "Check if a file or directory exists at the given path within the workspace.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to check from workspace root"
                    }
                },
                "required": ["path"]
            }),
        )
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        let path = call
            .arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        debug!("path_exists: path={}", path);

        if path.is_empty() {
            return ToolResult::error("path_exists", "path is required");
        }

        let full_path = match self.validate_path(path) {
            Ok(p) => p,
            Err(e) => {
                error!("path_exists: path validation failed: {}", e);
                return ToolResult::error("path_exists", &e);
            }
        };

        let exists = full_path.exists();

        if exists {
            let is_dir = full_path.is_dir();
            let metadata = fs::metadata(&full_path).await.ok();
            let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

            ToolResult::success(
                "path_exists",
                &format!(
                    "true ({} {})",
                    if is_dir { "directory" } else { "file" },
                    size
                ),
            )
        } else {
            ToolResult::success("path_exists", "false")
        }
    }
}

use tokio::fs;

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(path: &str) -> ToolCall {
        ToolCall {
            name: "path_exists".to_string(),
            arguments: serde_json::json!({
                "path": path
            }),
        }
    }

    #[tokio::test]
    async fn path_exists_returns_true_for_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "content").unwrap();

        let tool = PathExistsTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("test.txt")).await;

        assert!(result.success);
        assert!(result.output.contains("true"));
    }

    #[tokio::test]
    async fn path_exists_returns_true_for_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("testdir")).unwrap();

        let tool = PathExistsTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("testdir")).await;

        assert!(result.success);
        assert!(result.output.contains("true"));
        assert!(result.output.contains("directory"));
    }

    #[tokio::test]
    async fn path_exists_returns_false_for_nonexistent_path() {
        let dir = tempfile::tempdir().unwrap();

        let tool = PathExistsTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("nonexistent.txt")).await;

        assert!(result.success);
        assert_eq!(result.output, "false");
    }

    #[tokio::test]
    async fn path_exists_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();

        let tool = PathExistsTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("../etc/passwd")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("traversal"));
    }

    #[tokio::test]
    async fn path_exists_rejects_absolute_path() {
        let dir = tempfile::tempdir().unwrap();

        let tool = PathExistsTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("/etc/passwd")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Absolute paths"));
    }
}
