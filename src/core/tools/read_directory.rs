/// Tool for reading directory contents with detailed metadata
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, error, info};

use super::types::{FileEntry, ToolCall, ToolDefinition, ToolResult};

pub struct ReadDirectoryTool {
    workspace_root: PathBuf,
}

impl ReadDirectoryTool {
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

    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "read_directory",
            "List contents of a directory with detailed file metadata including size and modification time. Unlike list_directory which provides ls-style output, this returns structured data.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to directory (default: '.')"
                    },
                    "include_hidden": {
                        "type": "boolean",
                        "description": "Include hidden files (starting with .) (default: false)"
                    }
                },
                "required": []
            }),
        )
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        let path = call
            .arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        let include_hidden = call
            .arguments
            .get("include_hidden")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        debug!(
            "read_directory: path={}, include_hidden={}",
            path, include_hidden
        );

        let resolved_path = match self.validate_path(path) {
            Ok(p) => p,
            Err(e) => {
                error!("read_directory: path validation failed: {}", e);
                return ToolResult::error("read_directory", &e);
            }
        };

        // Check if path is a directory
        let metadata = match fs::metadata(&resolved_path).await {
            Ok(m) => m,
            Err(e) => {
                error!("read_directory: failed to stat path: {}", e);
                return ToolResult::error("read_directory", &format!("Path does not exist: {}", e));
            }
        };

        if !metadata.is_dir() {
            return ToolResult::error(
                "read_directory",
                &format!("Path is not a directory: {}", path),
            );
        }

        let mut entries = match fs::read_dir(&resolved_path).await {
            Ok(e) => e,
            Err(e) => {
                error!("read_directory: failed to read directory: {}", e);
                return ToolResult::error(
                    "read_directory",
                    &format!("Failed to read directory: {}", e),
                );
            }
        };

        let mut files: Vec<FileEntry> = Vec::new();

        while let Some(entry) = entries.next_entry().await.unwrap_or(None) {
            let file_name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files unless requested
            if !include_hidden && file_name.starts_with('.') {
                continue;
            }

            let file_path = entry.path();
            let file_meta = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };

            let modified = file_meta.modified().ok().map(|t| {
                let datetime: chrono::DateTime<chrono::Utc> = t.into();
                datetime.format("%Y-%m-%d %H:%M:%S").to_string()
            });

            let entry = FileEntry::new(
                &file_name,
                &file_path.to_string_lossy(),
                file_meta.is_dir(),
                file_meta.len(),
                modified,
            );
            files.push(entry);
        }

        // Sort by name
        files.sort_by(|a, b| a.name.cmp(&b.name));

        let output = if files.is_empty() {
            "(empty directory)".to_string()
        } else {
            let mut out = String::new();
            out.push_str(&format!("{:->50}\n", ""));
            out.push_str(&format!("{:<40} {:>10} {}\n", "NAME", "SIZE", "MODIFIED"));
            out.push_str(&format!("{:->50}\n", ""));
            for entry in &files {
                let type_char = if entry.is_dir { "d" } else { "f" };
                let size = if entry.size > 1024 * 1024 {
                    format!("{:.1}M", entry.size as f64 / (1024.0 * 1024.0))
                } else if entry.size > 1024 {
                    format!("{:.1}K", entry.size as f64 / 1024.0)
                } else {
                    format!("{}B", entry.size)
                };
                let modified = entry.modified.as_deref().unwrap_or("-");
                out.push_str(&format!(
                    "{}{:<38} {:>10} {}\n",
                    type_char, entry.name, size, modified
                ));
            }
            out.push_str(&format!("{:->50}\n", ""));
            out.push_str(&format!("{} entries", files.len()));
            out
        };

        info!("read_directory: listed {} entries in {}", files.len(), path);
        ToolResult::success("read_directory", &output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(path: &str, include_hidden: bool) -> ToolCall {
        ToolCall {
            name: "read_directory".to_string(),
            arguments: serde_json::json!({
                "path": path,
                "include_hidden": include_hidden
            }),
        }
    }

    #[tokio::test]
    async fn read_directory_lists_files_with_metadata() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file1.txt"), "content1").unwrap();
        std::fs::write(dir.path().join("file2.txt"), "content2").unwrap();

        let tool = ReadDirectoryTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call(".", false)).await;

        assert!(result.success);
        assert!(result.output.contains("file1.txt"));
        assert!(result.output.contains("file2.txt"));
    }

    #[tokio::test]
    async fn read_directory_excludes_hidden_files_by_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("visible.txt"), "content").unwrap();
        std::fs::write(dir.path().join(".hidden.txt"), "content").unwrap();

        let tool = ReadDirectoryTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call(".", false)).await;

        assert!(result.success);
        assert!(result.output.contains("visible.txt"));
        assert!(!result.output.contains(".hidden.txt"));
    }

    #[tokio::test]
    async fn read_directory_includes_hidden_when_requested() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("visible.txt"), "content").unwrap();
        std::fs::write(dir.path().join(".hidden.txt"), "content").unwrap();

        let tool = ReadDirectoryTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call(".", true)).await;

        assert!(result.success);
        assert!(result.output.contains("visible.txt"));
        assert!(result.output.contains(".hidden.txt"));
    }

    #[tokio::test]
    async fn read_directory_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadDirectoryTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("../etc/passwd", false)).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("traversal"));
    }

    #[tokio::test]
    async fn read_directory_rejects_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadDirectoryTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("/etc", false)).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Absolute paths"));
    }

    #[tokio::test]
    async fn read_directory_shows_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        std::fs::write(dir.path().join("file.txt"), "content").unwrap();

        let tool = ReadDirectoryTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call(".", false)).await;

        assert!(result.success);
        // Directories shown with 'd' prefix
        assert!(result.output.contains("d"));
    }
}
