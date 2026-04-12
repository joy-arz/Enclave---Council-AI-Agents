/// Tool for applying unified diff patches to files
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, error, info};

use super::types::{ToolCall, ToolDefinition, ToolResult};

pub struct ApplyPatchTool {
    workspace_root: PathBuf,
}

impl ApplyPatchTool {
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
            "apply_patch",
            "Apply a unified diff patch to a file. Takes a path and a patch string in unified diff format.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file to patch from workspace root"
                    },
                    "patch": {
                        "type": "string",
                        "description": "Unified diff patch content"
                    }
                },
                "required": ["path", "patch"]
            }),
        )
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        let path = call
            .arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let patch = call
            .arguments
            .get("patch")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        debug!("apply_patch: path={}", path);

        if patch.is_empty() {
            return ToolResult::error("apply_patch", "patch must not be empty");
        }

        let resolved_path = match self.validate_path(path) {
            Ok(p) => p,
            Err(e) => {
                error!("apply_patch: path validation failed: {}", e);
                return ToolResult::error("apply_patch", &e);
            }
        };

        // Create backup
        let backup_path = resolved_path.with_extension("bak");
        if let Err(e) = fs::copy(&resolved_path, &backup_path).await {
            debug!("apply_patch: could not create backup: {}", e);
        }

        // Parse unified diff and apply
        match self.apply_unified_diff(&resolved_path, patch).await {
            Ok(count) => {
                info!("apply_patch: successfully applied {} hunk(s)", count);
                ToolResult::success(
                    "apply_patch",
                    &format!(
                        "Successfully applied patch to {} ({} hunk(s) applied)",
                        path, count
                    ),
                )
            }
            Err(e) => {
                error!("apply_patch: failed to apply patch: {}", e);
                // Try to restore from backup
                if backup_path.exists() {
                    let _ = fs::copy(&backup_path, &resolved_path).await;
                    let _ = fs::remove_file(&backup_path).await;
                }
                ToolResult::error("apply_patch", &format!("Failed to apply patch: {}", e))
            }
        }
    }

    async fn apply_unified_diff(&self, path: &PathBuf, patch: &str) -> Result<usize, String> {
        let content = fs::read_to_string(path).await.map_err(|e| e.to_string())?;
        let mut lines: Vec<&str> = content.lines().collect();
        let mut patch_lines = patch.lines().peekable();
        let mut hunks_applied = 0;
        let line_index = 0;

        while let Some(line) = patch_lines.next() {
            if !line.starts_with("@@ ") {
                continue;
            }

            // Parse hunk header: @@ -start,count +start,count @@
            let hunk_header = line;
            let parts: Vec<&str> = hunk_header.split_whitespace().collect();
            if parts.len() < 3 {
                return Err(format!("Invalid hunk header: {}", hunk_header));
            }

            let old_range = parts[1].trim_start_matches('-');
            let new_range = parts[2].trim_start_matches('+');

            let (old_start, _) = Self::parse_range(old_range);
            let (_new_start, _) = Self::parse_range(new_range);

            // Consume the hunk header
            let _ = patch_lines.next();

            // Build hunk content
            let mut hunk_lines: Vec<&str> = Vec::new();
            while let Some(&line) = patch_lines.peek() {
                if line.starts_with("@@ ") || line.starts_with("-- ") {
                    break;
                }
                hunk_lines.push(line);
                patch_lines.next();
            }

            // Apply hunk - simple implementation for common cases
            let mut old_idx = old_start.saturating_sub(1);
            let mut hunk_idx = 0;

            while hunk_idx < hunk_lines.len() && old_idx < lines.len() {
                let hunk_line = hunk_lines[hunk_idx];

                if hunk_line.starts_with(' ') || hunk_line.is_empty() {
                    // Context line - should match
                    if lines.get(old_idx) == Some(&hunk_line.trim_start_matches('\\')) {
                        old_idx += 1;
                        hunk_idx += 1;
                    } else if hunk_line == "\\ No newline at end of file" {
                        hunk_idx += 1;
                    } else {
                        // Try to find matching context
                        old_idx += 1;
                    }
                } else if hunk_line.starts_with('-') {
                    // Line to delete
                    let content_line = &hunk_line[1..];
                    if lines.get(old_idx) == Some(&content_line) {
                        lines.remove(old_idx);
                        hunk_idx += 1;
                    } else {
                        return Err(format!(
                            "Delete mismatch at line {}: expected '{}', found '{:?}'",
                            old_idx + 1,
                            content_line,
                            lines.get(old_idx)
                        ));
                    }
                } else if hunk_line.starts_with('+') {
                    // Line to insert
                    let content_line = &hunk_line[1..];
                    lines.insert(old_idx, content_line);
                    old_idx += 1;
                    hunk_idx += 1;
                } else {
                    hunk_idx += 1;
                }
            }

            // Handle remaining hunk lines (additions at end of file)
            while hunk_idx < hunk_lines.len() {
                let hunk_line = hunk_lines[hunk_idx];
                if hunk_line.starts_with('+') {
                    lines.push(&hunk_line[1..]);
                } else if !hunk_line.starts_with('-') && !hunk_line.starts_with('\\') {
                    lines.push(hunk_line);
                }
                hunk_idx += 1;
            }

            hunks_applied += 1;
        }

        // Write back
        let new_content = if content.ends_with('\n') {
            lines.join("\n") + "\n"
        } else {
            lines.join("\n")
        };

        fs::write(path, new_content)
            .await
            .map_err(|e| e.to_string())?;

        Ok(hunks_applied)
    }

    fn parse_range(range: &str) -> (usize, usize) {
        let parts: Vec<&str> = range.split(',').collect();
        let start = parts.first().and_then(|s| s.parse().ok()).unwrap_or(1);
        let count = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
        (start, count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(path: &str, patch: &str) -> ToolCall {
        ToolCall {
            name: "apply_patch".to_string(),
            arguments: serde_json::json!({
                "path": path,
                "patch": patch
            }),
        }
    }

    #[tokio::test]
    async fn apply_patch_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();

        let tool = ApplyPatchTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("../etc/passwd", "")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("traversal"));
    }

    #[tokio::test]
    async fn apply_patch_rejects_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();

        let tool = ApplyPatchTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("/etc/passwd", "")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Absolute paths"));
    }
}
