/// Tool for deleting files and directories within the workspace
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, error, info, warn};

use super::types::{ToolCall, ToolDefinition, ToolResult};

pub struct DeletePathTool {
    workspace_root: PathBuf,
}

impl DeletePathTool {
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
        let resolved = full_path
            .canonicalize()
            .map_err(|e| format!("Path resolution failed: {}", e))?;
        let workspace_resolved = self
            .workspace_root
            .canonicalize()
            .map_err(|e| format!("Workspace resolution failed: {}", e))?;
        if !resolved.starts_with(&workspace_resolved) {
            return Err("Path escapes workspace".to_string());
        }
        Ok(resolved)
    }
}

impl DeletePathTool {
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "delete_path",
            "Delete a file or directory within the workspace. Use recursive=true to delete directories and their contents.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file or directory from workspace root"
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "If true, delete directories and all their contents. If false, only delete files or empty directories (default: false)"
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
        let recursive = call
            .arguments
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        debug!("delete_path: path={}, recursive={}", path, recursive);

        if path.is_empty() {
            return ToolResult::error("delete_path", "path is required");
        }

        // Disallow deleting workspace root
        if path == "." || path == "./" {
            return ToolResult::error("delete_path", "Cannot delete workspace root");
        }

        let resolved_path = match self.validate_path(path) {
            Ok(p) => p,
            Err(e) => {
                error!("delete_path: path validation failed: {}", e);
                return ToolResult::error("delete_path", &e);
            }
        };

        // Check path exists
        let metadata = match fs::metadata(&resolved_path).await {
            Ok(m) => m,
            Err(e) => {
                error!("delete_path: failed to stat path: {}", e);
                return ToolResult::error("delete_path", &format!("Path does not exist: {}", e));
            }
        };

        let is_dir = metadata.is_dir();

        // Determine what delete operation to perform
        let result = if is_dir {
            if recursive {
                warn!("delete_path: recursively deleting directory {}", path);
                fs::remove_dir_all(&resolved_path).await
            } else {
                fs::remove_dir(&resolved_path).await
            }
        } else {
            fs::remove_file(&resolved_path).await
        };

        match result {
            Ok(()) => {
                info!("delete_path: successfully deleted {}", path);
                let type_str = if is_dir {
                    if recursive {
                        "directory tree"
                    } else {
                        "directory"
                    }
                } else {
                    "file"
                };
                ToolResult::success("delete_path", &format!("Deleted {} '{}'", type_str, path))
            }
            Err(e) => {
                error!("delete_path: failed to delete: {}", e);
                let msg = if is_dir && !recursive {
                    format!(
                        "Failed to delete directory '{}': not empty. Use recursive=true to delete contents. Error: {}",
                        path, e
                    )
                } else {
                    format!("Failed to delete '{}': {}", path, e)
                };
                ToolResult::error("delete_path", &msg)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(path: &str, recursive: bool) -> ToolCall {
        ToolCall {
            name: "delete_path".to_string(),
            arguments: serde_json::json!({
                "path": path,
                "recursive": recursive
            }),
        }
    }

    #[tokio::test]
    async fn delete_path_deletes_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), "content").unwrap();

        let tool = DeletePathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("file.txt", false)).await;

        assert!(result.success);
        assert!(!dir.path().join("file.txt").exists());
    }

    #[tokio::test]
    async fn delete_path_deletes_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("emptydir");
        std::fs::create_dir(&subdir).unwrap();

        let tool = DeletePathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("emptydir", false)).await;

        assert!(result.success);
        assert!(!subdir.exists());
    }

    #[tokio::test]
    async fn delete_path_fails_on_nonempty_directory_without_recursive() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        std::fs::write(subdir.join("file.txt"), "content").unwrap();

        let tool = DeletePathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("subdir", false)).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("not empty"));
    }

    #[tokio::test]
    async fn delete_path_deletes_directory_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        std::fs::write(subdir.join("file.txt"), "content").unwrap();

        let tool = DeletePathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("subdir", true)).await;

        assert!(result.success);
        assert!(!subdir.exists());
    }

    #[tokio::test]
    async fn delete_path_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), "content").unwrap();

        let tool = DeletePathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("../etc/passwd", false)).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("traversal"));
    }

    #[tokio::test]
    async fn delete_path_rejects_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), "content").unwrap();

        let tool = DeletePathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("/etc/passwd", false)).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Absolute paths"));
    }

    #[tokio::test]
    async fn delete_path_rejects_workspace_root() {
        let dir = tempfile::tempdir().unwrap();

        let tool = DeletePathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call(".", false)).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("workspace root"));
    }
}
