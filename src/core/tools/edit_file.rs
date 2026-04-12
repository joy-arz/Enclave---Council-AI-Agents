/// Tool for editing specific text in a file
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, error, info};

use super::types::{ToolCall, ToolDefinition, ToolResult};

pub struct EditFileTool {
    workspace_root: PathBuf,
}

impl EditFileTool {
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

impl EditFileTool {
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "edit_file",
            "Edit specific text in a file. Replaces old_text with new_text. Use replace_all=true to replace all occurrences, otherwise fails if multiple matches exist.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file from workspace root"
                    },
                    "old_text": {
                        "type": "string",
                        "description": "Text to find and replace"
                    },
                    "new_text": {
                        "type": "string",
                        "description": "Replacement text"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "If true, replace all occurrences. If false, error on multiple matches (default: false)"
                    }
                },
                "required": ["path", "old_text", "new_text"]
            }),
        )
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        let path = call
            .arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let old_text = call
            .arguments
            .get("old_text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new_text = call
            .arguments
            .get("new_text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let replace_all = call
            .arguments
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        debug!("edit_file: path={}, replace_all={}", path, replace_all);

        if old_text.is_empty() {
            return ToolResult::error("edit_file", "old_text must not be empty");
        }

        let resolved_path = match self.validate_path(path) {
            Ok(p) => p,
            Err(e) => {
                error!("edit_file: path validation failed: {}", e);
                return ToolResult::error("edit_file", &e);
            }
        };

        let content = match fs::read_to_string(&resolved_path).await {
            Ok(c) => c,
            Err(e) => {
                error!("edit_file: failed to read file: {}", e);
                return ToolResult::error("edit_file", &format!("Failed to read file: {}", e));
            }
        };

        let occurrence_count = content.matches(old_text).count();
        if occurrence_count == 0 {
            return ToolResult::error("edit_file", "old_text was not found in file");
        }

        let updated = if replace_all {
            info!("edit_file: replacing all {} occurrences", occurrence_count);
            content.replace(old_text, new_text)
        } else if occurrence_count > 1 {
            return ToolResult::error(
                "edit_file",
                &format!("old_text matched {} occurrences; use replace_all=true or use replace_match for precise edit", occurrence_count)
            );
        } else if let Some(index) = content.find(old_text) {
            let mut updated = content.clone();
            updated.replace_range(index..index + old_text.len(), new_text);
            updated
        } else {
            return ToolResult::error("edit_file", "old_text was not found");
        };

        match fs::write(&resolved_path, &updated).await {
            Ok(()) => {
                info!("edit_file: successfully edited file");
                ToolResult::success(
                    "edit_file",
                    &format!(
                        "Edited {} (replaced {} occurrence{})",
                        path,
                        occurrence_count,
                        if occurrence_count == 1 { "" } else { "s" }
                    ),
                )
            }
            Err(e) => {
                error!("edit_file: failed to write file: {}", e);
                ToolResult::error("edit_file", &format!("Failed to write file: {}", e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(path: &str, old_text: &str, new_text: &str, replace_all: bool) -> ToolCall {
        ToolCall {
            name: "edit_file".to_string(),
            arguments: serde_json::json!({
                "path": path,
                "old_text": old_text,
                "new_text": new_text,
                "replace_all": replace_all
            }),
        }
    }

    #[tokio::test]
    async fn edit_file_replaces_text() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world").unwrap();

        let tool = EditFileTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("test.txt", "world", "rust")).await;

        assert!(result.success);
        let content = std::fs::read_to_string(dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "hello rust");
    }

    #[tokio::test]
    async fn edit_file_fails_on_multiple_matches_without_replace_all() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "foo foo").unwrap();

        let tool = EditFileTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_call("test.txt", "foo", "bar", false))
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("multiple matches"));
    }

    #[tokio::test]
    async fn edit_file_replace_all_replaces_all_occurrences() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "foo foo").unwrap();

        let tool = EditFileTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_call("test.txt", "foo", "bar", true))
            .await;

        assert!(result.success);
        let content = std::fs::read_to_string(dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "bar bar");
    }
}
