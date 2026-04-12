/// Tool for renaming files and directories within the workspace
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, error, info};

use super::types::{ToolCall, ToolDefinition, ToolResult};

pub struct RenamePathTool {
    workspace_root: PathBuf,
}

impl RenamePathTool {
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

impl RenamePathTool {
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "rename_path",
            "Rename a file or directory within the workspace. Provides a simpler interface than move_path for simple rename operations.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Current path of the file or directory (relative to workspace root)"
                    },
                    "new_name": {
                        "type": "string",
                        "description": "New name for the file or directory (not a full path, just the name)"
                    }
                },
                "required": ["path", "new_name"]
            }),
        )
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        let path = call
            .arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new_name = call
            .arguments
            .get("new_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        debug!("rename_path: {} -> {}", path, new_name);

        if path.is_empty() {
            return ToolResult::error("rename_path", "path is required");
        }
        if new_name.is_empty() {
            return ToolResult::error("rename_path", "new_name is required");
        }

        // Check for path traversal in new_name
        let new_name_obj = PathBuf::from(new_name);
        if new_name
            != new_name_obj
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
        {
            return ToolResult::error(
                "rename_path",
                "new_name must be a simple name, not a path with directories",
            );
        }

        let source_path = match self.validate_path(path) {
            Ok(p) => p,
            Err(e) => {
                error!("rename_path: source path validation failed: {}", e);
                return ToolResult::error("rename_path", &e);
            }
        };

        // Check source exists
        if !source_path.exists() {
            return ToolResult::error("rename_path", &format!("Path does not exist: {}", path));
        }

        // Calculate destination path
        let parent = match source_path.parent() {
            Some(p) => p,
            None => {
                return ToolResult::error("rename_path", "Could not determine parent directory");
            }
        };
        let dest_path = parent.join(&new_name);

        // Verify destination parent is within workspace
        match parent.canonicalize() {
            Ok(canonical_parent) => {
                let workspace_resolved = match self.workspace_root.canonicalize() {
                    Ok(p) => p,
                    Err(e) => {
                        return ToolResult::error(
                            "rename_path",
                            &format!("Workspace resolution failed: {}", e),
                        );
                    }
                };
                if !canonical_parent.starts_with(&workspace_resolved) {
                    return ToolResult::error("rename_path", "Path escapes workspace");
                }
            }
            Err(e) => {
                error!("rename_path: failed to resolve parent: {}", e);
                return ToolResult::error(
                    "rename_path",
                    &format!("Failed to resolve parent: {}", e),
                );
            }
        }

        // Check if destination already exists
        if dest_path.exists() {
            return ToolResult::error(
                "rename_path",
                &format!("Destination already exists: {}", new_name),
            );
        }

        match fs::rename(&source_path, &dest_path).await {
            Ok(()) => {
                info!("rename_path: successfully renamed {} -> {}", path, new_name);
                ToolResult::success("rename_path", &format!("Renamed {} -> {}", path, new_name))
            }
            Err(e) => {
                error!("rename_path: failed to rename: {}", e);
                ToolResult::error("rename_path", &format!("Failed to rename path: {}", e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(path: &str, new_name: &str) -> ToolCall {
        ToolCall {
            name: "rename_path".to_string(),
            arguments: serde_json::json!({
                "path": path,
                "new_name": new_name
            }),
        }
    }

    #[tokio::test]
    async fn rename_path_renames_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.txt"), "content").unwrap();

        let tool = RenamePathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("old.txt", "new.txt")).await;

        assert!(result.success);
        assert!(!dir.path().join("old.txt").exists());
        assert!(dir.path().join("new.txt").exists());
    }

    #[tokio::test]
    async fn rename_path_fails_for_nonexistent_path() {
        let dir = tempfile::tempdir().unwrap();

        let tool = RenamePathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("nonexistent.txt", "new.txt")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("does not exist"));
    }

    #[tokio::test]
    async fn rename_path_fails_if_destination_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file1.txt"), "content1").unwrap();
        std::fs::write(dir.path().join("file2.txt"), "content2").unwrap();

        let tool = RenamePathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("file1.txt", "file2.txt")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("already exists"));
    }
}
