/// Tool for creating directories within the workspace
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, error, info};

use super::types::{ToolCall, ToolDefinition, ToolResult};

pub struct CreateDirectoryTool {
    workspace_root: PathBuf,
}

impl CreateDirectoryTool {
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
            "create_directory",
            "Create a directory and all parent directories within the workspace if they don't exist. Similar to 'mkdir -p'.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the directory to create from workspace root"
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

        debug!("create_directory: path={}", path);

        if path.is_empty() {
            return ToolResult::error("create_directory", "path is required");
        }

        let full_path = match self.validate_path(path) {
            Ok(p) => p,
            Err(e) => {
                error!("create_directory: path validation failed: {}", e);
                return ToolResult::error("create_directory", &e);
            }
        };

        if full_path.exists() {
            if full_path.is_dir() {
                return ToolResult::error(
                    "create_directory",
                    &format!("Directory already exists: {}", path),
                );
            } else {
                return ToolResult::error(
                    "create_directory",
                    &format!("A file already exists at path: {}", path),
                );
            }
        }

        match fs::create_dir_all(&full_path).await {
            Ok(()) => {
                info!("create_directory: successfully created directory {}", path);
                ToolResult::success("create_directory", &format!("Created directory {}", path))
            }
            Err(e) => {
                error!("create_directory: failed to create directory: {}", e);
                ToolResult::error(
                    "create_directory",
                    &format!("Failed to create directory: {}", e),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(path: &str) -> ToolCall {
        ToolCall {
            name: "create_directory".to_string(),
            arguments: serde_json::json!({
                "path": path
            }),
        }
    }

    #[tokio::test]
    async fn create_directory_creates_nested_directories() {
        let dir = tempfile::tempdir().unwrap();
        let tool = CreateDirectoryTool::new(dir.path().to_path_buf());

        let result = tool.execute(&make_call("foo/bar/baz")).await;

        assert!(result.success);
        assert!(dir.path().join("foo/bar/baz").exists());
        assert!(dir.path().join("foo/bar/baz").is_dir());
    }

    #[tokio::test]
    async fn create_directory_fails_for_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("existing")).unwrap();

        let tool = CreateDirectoryTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("existing")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("already exists"));
    }

    #[tokio::test]
    async fn create_directory_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let tool = CreateDirectoryTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("../outside")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("traversal"));
    }

    #[tokio::test]
    async fn create_directory_rejects_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let tool = CreateDirectoryTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("/tmp/malicious")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Absolute paths"));
    }
}
