/// Tool for moving or renaming files and directories within the workspace
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, error, info};

use super::types::{ToolCall, ToolDefinition, ToolResult};

pub struct MovePathTool {
    workspace_root: PathBuf,
}

impl MovePathTool {
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

impl MovePathTool {
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "move_path",
            "Move or rename a file or directory within the workspace. Can be used to rename a file (by providing a new name in the same directory) or move to a different directory.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "Source path (relative to workspace root)"
                    },
                    "destination": {
                        "type": "string",
                        "description": "Destination path (relative to workspace root)"
                    }
                },
                "required": ["source", "destination"]
            }),
        )
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        let source = call
            .arguments
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let destination = call
            .arguments
            .get("destination")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        debug!("move_path: {} -> {}", source, destination);

        if source.is_empty() {
            return ToolResult::error("move_path", "source is required");
        }
        if destination.is_empty() {
            return ToolResult::error("move_path", "destination is required");
        }

        let source_path = match self.validate_path(source) {
            Ok(p) => p,
            Err(e) => {
                error!("move_path: source path validation failed: {}", e);
                return ToolResult::error("move_path", &e);
            }
        };

        // Validate destination path
        let dest_path_obj = PathBuf::from(destination);
        if dest_path_obj.is_absolute() {
            return ToolResult::error("move_path", "Absolute paths not allowed for destination");
        }
        if dest_path_obj.components().any(|c| c.as_os_str() == "..") {
            return ToolResult::error("move_path", "Path traversal not allowed for destination");
        }

        let dest_path = self.workspace_root.join(&dest_path_obj);

        // Ensure destination parent directory exists
        if let Some(parent) = dest_path.parent() {
            if !parent.exists() {
                if let Err(e) = fs::create_dir_all(parent).await {
                    error!("move_path: failed to create destination parent: {}", e);
                    return ToolResult::error(
                        "move_path",
                        &format!("Failed to create destination directory: {}", e),
                    );
                }
            }
            // Verify parent is within workspace
            match parent.canonicalize() {
                Ok(canonical_parent) => {
                    let workspace_resolved = match self.workspace_root.canonicalize() {
                        Ok(p) => p,
                        Err(e) => {
                            return ToolResult::error(
                                "move_path",
                                &format!("Workspace resolution failed: {}", e),
                            );
                        }
                    };
                    if !canonical_parent.starts_with(&workspace_resolved) {
                        return ToolResult::error(
                            "move_path",
                            "Destination path escapes workspace",
                        );
                    }
                }
                Err(e) => {
                    error!("move_path: failed to resolve destination parent: {}", e);
                    return ToolResult::error(
                        "move_path",
                        &format!("Failed to resolve destination: {}", e),
                    );
                }
            }
        }

        // Check source exists
        if !source_path.exists() {
            return ToolResult::error(
                "move_path",
                &format!("Source path does not exist: {}", source),
            );
        }

        match fs::rename(&source_path, &dest_path).await {
            Ok(()) => {
                info!(
                    "move_path: successfully moved {} -> {}",
                    source, destination
                );
                ToolResult::success("move_path", &format!("Moved {} -> {}", source, destination))
            }
            Err(e) => {
                error!("move_path: failed to move: {}", e);
                ToolResult::error("move_path", &format!("Failed to move path: {}", e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(source: &str, destination: &str) -> ToolCall {
        ToolCall {
            name: "move_path".to_string(),
            arguments: serde_json::json!({
                "source": source,
                "destination": destination
            }),
        }
    }

    #[tokio::test]
    async fn move_path_renames_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.txt"), "content").unwrap();

        let tool = MovePathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("old.txt", "new.txt")).await;

        assert!(result.success);
        assert!(!dir.path().join("old.txt").exists());
        assert!(dir.path().join("new.txt").exists());
    }

    #[tokio::test]
    async fn move_path_moves_file_to_directory() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        std::fs::write(dir.path().join("file.txt"), "content").unwrap();

        let tool = MovePathTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_call("file.txt", "subdir/file.txt"))
            .await;

        assert!(result.success);
        assert!(!dir.path().join("file.txt").exists());
        assert!(subdir.join("file.txt").exists());
    }

    #[tokio::test]
    async fn move_path_fails_for_nonexistent_source() {
        let dir = tempfile::tempdir().unwrap();

        let tool = MovePathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("nonexistent.txt", "new.txt")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("does not exist"));
    }
}
