/// Tool for copying files and directories recursively within the workspace
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, error, info};

use super::types::{ToolCall, ToolDefinition, ToolResult};

pub struct CopyPathTool {
    workspace_root: PathBuf,
}

impl CopyPathTool {
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

impl CopyPathTool {
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "copy_path",
            "Copy a file or directory recursively within the workspace. Creates parent directories if needed.",
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

        debug!("copy_path: {} -> {}", source, destination);

        if source.is_empty() {
            return ToolResult::error("copy_path", "source is required");
        }
        if destination.is_empty() {
            return ToolResult::error("copy_path", "destination is required");
        }

        let source_path = match self.validate_path(source) {
            Ok(p) => p,
            Err(e) => {
                error!("copy_path: source path validation failed: {}", e);
                return ToolResult::error("copy_path", &e);
            }
        };

        // Validate destination path
        let dest_path_obj = PathBuf::from(destination);
        if dest_path_obj.is_absolute() {
            return ToolResult::error("copy_path", "Absolute paths not allowed for destination");
        }
        if dest_path_obj.components().any(|c| c.as_os_str() == "..") {
            return ToolResult::error("copy_path", "Path traversal not allowed for destination");
        }

        let dest_path = self.workspace_root.join(&dest_path_obj);

        // Check source exists
        if !source_path.exists() {
            return ToolResult::error(
                "copy_path",
                &format!("Source path does not exist: {}", source),
            );
        }

        // Create destination parent directory if needed
        if let Some(parent) = dest_path.parent() {
            if !parent.exists() {
                if let Err(e) = fs::create_dir_all(parent).await {
                    error!("copy_path: failed to create destination parent: {}", e);
                    return ToolResult::error(
                        "copy_path",
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
                                "copy_path",
                                &format!("Workspace resolution failed: {}", e),
                            );
                        }
                    };
                    if !canonical_parent.starts_with(&workspace_resolved) {
                        return ToolResult::error(
                            "copy_path",
                            "Destination path escapes workspace",
                        );
                    }
                }
                Err(e) => {
                    error!("copy_path: failed to resolve destination parent: {}", e);
                    return ToolResult::error(
                        "copy_path",
                        &format!("Failed to resolve destination: {}", e),
                    );
                }
            }
        }

        // Perform copy based on source type
        let metadata = match fs::metadata(&source_path).await {
            Ok(m) => m,
            Err(e) => {
                error!("copy_path: failed to get source metadata: {}", e);
                return ToolResult::error(
                    "copy_path",
                    &format!("Failed to stat source path: {}", e),
                );
            }
        };

        let result = if metadata.is_dir() {
            Self::copy_dir_recursive(&source_path, &dest_path).await
        } else {
            fs::copy(&source_path, &dest_path).await.map(|_| ())
        };

        match result {
            Ok(_) => {
                info!(
                    "copy_path: successfully copied {} -> {}",
                    source, destination
                );
                ToolResult::success(
                    "copy_path",
                    &format!("Copied {} -> {}", source, destination),
                )
            }
            Err(e) => {
                error!("copy_path: failed to copy: {}", e);
                ToolResult::error("copy_path", &format!("Failed to copy path: {}", e))
            }
        }
    }

    async fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> std::io::Result<()> {
        if !dst.exists() {
            fs::create_dir_all(dst).await?;
        }

        let mut stack: Vec<(PathBuf, PathBuf)> = vec![(src.clone(), dst.clone())];

        while let Some((current_src, current_dst)) = stack.pop() {
            if !current_dst.exists() {
                fs::create_dir_all(&current_dst).await?;
            }

            let mut entries = fs::read_dir(&current_src).await?;
            while let Some(entry) = entries.next_entry().await? {
                let src_path = entry.path();
                let dst_path = current_dst.join(entry.file_name());

                if src_path.is_dir() {
                    stack.push((src_path, dst_path));
                } else {
                    fs::copy(&src_path, &dst_path).await?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(source: &str, destination: &str) -> ToolCall {
        ToolCall {
            name: "copy_path".to_string(),
            arguments: serde_json::json!({
                "source": source,
                "destination": destination
            }),
        }
    }

    #[tokio::test]
    async fn copy_path_copies_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("source.txt"), "content").unwrap();

        let tool = CopyPathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("source.txt", "dest.txt")).await;

        assert!(result.success);
        assert!(dir.path().join("source.txt").exists());
        assert!(dir.path().join("dest.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("dest.txt")).unwrap(),
            "content"
        );
    }

    #[tokio::test]
    async fn copy_path_copies_directory_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        std::fs::create_dir(&src_dir).unwrap();
        std::fs::write(src_dir.join("file1.txt"), "content1").unwrap();
        std::fs::write(src_dir.join("file2.txt"), "content2").unwrap();

        let tool = CopyPathTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("src", "dst")).await;

        assert!(result.success);
        assert!(dir.path().join("src/file1.txt").exists());
        assert!(dir.path().join("dst/file1.txt").exists());
        assert!(dir.path().join("dst/file2.txt").exists());
    }

    #[tokio::test]
    async fn copy_path_fails_for_nonexistent_source() {
        let dir = tempfile::tempdir().unwrap();

        let tool = CopyPathTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_call("nonexistent.txt", "dest.txt"))
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("does not exist"));
    }
}
