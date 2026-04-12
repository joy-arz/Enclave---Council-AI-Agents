/// Tool for replacing text at exact path:line:column coordinates
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, error, info};

use super::types::{ToolCall, ToolDefinition, ToolResult};

pub struct ReplaceMatchTool {
    workspace_root: PathBuf,
}

impl ReplaceMatchTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }

    fn canonicalize_workspace_path(&self, path: &str) -> Result<PathBuf, String> {
        let path_obj = PathBuf::from(path);
        if path_obj.is_absolute() {
            return Err("Absolute paths not allowed".to_string());
        }
        if path_obj.components().any(|c| c.as_os_str() == "..") {
            return Err("Path traversal not allowed".to_string());
        }
        let full_path = self.workspace_root.join(path);
        let canonical = full_path
            .canonicalize()
            .map_err(|e| format!("Failed to resolve path '{}': {}", path, e))?;
        let workspace_resolved = self
            .workspace_root
            .canonicalize()
            .map_err(|e| format!("Workspace resolution error: {}", e))?;
        if !canonical.starts_with(&workspace_resolved) {
            return Err("Path escapes workspace".to_string());
        }
        Ok(canonical)
    }

    fn line_segment(content: &str, target_line: usize) -> Option<(usize, &str)> {
        if target_line == 0 {
            return None;
        }
        let mut start = 0usize;
        for (index, segment) in content.split_inclusive('\n').enumerate() {
            if index + 1 == target_line {
                return Some((start, segment));
            }
            start += segment.len();
        }

        if !content.is_empty() && !content.ends_with('\n') {
            let line_count = content.lines().count();
            if target_line == line_count {
                let start = content
                    .rmatch_indices('\n')
                    .next()
                    .map(|(idx, _)| idx + 1)
                    .unwrap_or(0);
                return Some((start, &content[start..]));
            }
        }
        None
    }

    fn line_body(segment: &str) -> &str {
        segment
            .strip_suffix('\n')
            .unwrap_or(segment)
            .strip_suffix('\r')
            .unwrap_or(segment)
    }

    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "replace_match",
            "Replace text at exact path:line:column coordinates. Verifies old_text matches before replacing. Use this for precise edits when you know the exact location.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file from workspace root"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number where the match starts"
                    },
                    "column": {
                        "type": "integer",
                        "description": "1-based byte column where the match starts"
                    },
                    "old_text": {
                        "type": "string",
                        "description": "Exact text expected at the specified location"
                    },
                    "new_text": {
                        "type": "string",
                        "description": "Replacement text"
                    }
                },
                "required": ["path", "line", "column", "old_text", "new_text"]
            }),
        )
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        let path = call
            .arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let line = call
            .arguments
            .get("line")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let column = call
            .arguments
            .get("column")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
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

        debug!(
            "replace_match: path={}, line={}, column={}",
            path, line, column
        );

        if old_text.is_empty() {
            return ToolResult::error("replace_match", "old_text must not be empty");
        }
        if line == 0 || column == 0 {
            return ToolResult::error("replace_match", "line and column must both be >= 1");
        }

        let canonical = match self.canonicalize_workspace_path(path) {
            Ok(p) => p,
            Err(e) => {
                error!("replace_match: path validation failed: {}", e);
                return ToolResult::error("replace_match", &e);
            }
        };

        let content = match fs::read_to_string(&canonical).await {
            Ok(c) => c,
            Err(e) => {
                error!("replace_match: failed to read file: {}", e);
                return ToolResult::error("replace_match", &format!("Failed to read file: {}", e));
            }
        };

        let total_occurrences = content.matches(old_text).count();

        let (line_start, segment) = match Self::line_segment(&content, line) {
            Some(s) => s,
            None => {
                return ToolResult::error(
                    "replace_match",
                    &format!("line {} does not exist in {}", line, path),
                );
            }
        };

        let body = Self::line_body(segment);
        let byte_column = column - 1;
        if byte_column > body.len() {
            return ToolResult::error(
                "replace_match",
                &format!("column {} is outside line {} in {}", column, line, path),
            );
        }

        let absolute_start = line_start + byte_column;
        let absolute_end = absolute_start + old_text.len();

        let found_text = match content.get(absolute_start..absolute_end) {
            Some(t) => t,
            None => {
                return ToolResult::error(
                    "replace_match",
                    &format!("old_text does not fit at {}:{}:{}", path, line, column),
                );
            }
        };

        if found_text != old_text {
            return ToolResult::error(
                "replace_match",
                &format!(
                    "Expected '{}' at {}:{}:{}, found '{}'",
                    old_text, path, line, column, found_text
                ),
            );
        }

        let mut updated = content;
        updated.replace_range(absolute_start..absolute_end, new_text);

        match fs::write(&canonical, &updated).await {
            Ok(()) => {
                info!(
                    "replace_match: successfully replaced match at {}:{}:{}",
                    path, line, column
                );
                ToolResult::success(
                    "replace_match",
                    &format!(
                        "Replaced match at {}:{}:{} ({} total occurrence(s) of old_text in file)",
                        path, line, column, total_occurrences
                    ),
                )
            }
            Err(e) => {
                error!("replace_match: failed to write file: {}", e);
                ToolResult::error("replace_match", &format!("Failed to write file: {}", e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(path: &str, line: u64, column: u64, old_text: &str, new_text: &str) -> ToolCall {
        ToolCall {
            name: "replace_match".to_string(),
            arguments: serde_json::json!({
                "path": path,
                "line": line,
                "column": column,
                "old_text": old_text,
                "new_text": new_text
            }),
        }
    }

    #[tokio::test]
    async fn replace_match_replaces_targeted_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("main.rs"),
            "fn main() { let first = alpha; let second = alpha; }\n",
        )
        .unwrap();

        let tool = ReplaceMatchTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_call("main.rs", 1, 45, "alpha", "beta"))
            .await;

        assert!(result.success, "{:?}", result.error);
        let updated = std::fs::read_to_string(dir.path().join("main.rs")).unwrap();
        assert_eq!(
            updated,
            "fn main() { let first = alpha; let second = beta; }\n"
        );
    }

    #[tokio::test]
    async fn replace_match_fails_when_coordinate_does_not_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() { alpha; }\n").unwrap();

        let tool = ReplaceMatchTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(&make_call("main.rs", 1, 1, "alpha", "beta"))
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Expected 'alpha'"));
    }
}
