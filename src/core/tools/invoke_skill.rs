/// Tool for invoking skills by name
use std::path::PathBuf;
use tracing::{debug, error, info};

use crate::core::skills::SkillCatalog;

use super::types::{ToolCall, ToolDefinition, ToolResult};

pub struct InvokeSkillTool {
    workspace_root: PathBuf,
}

impl InvokeSkillTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }

    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "invoke_skill",
            "Load and execute a skill by name. Skills are reusable workflows defined in AGENTS.md or .nca/skills/ directories. Use this when you need to perform a specific task that matches an available skill. Available skills can be discovered in the system prompt.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The slash command to invoke the skill (e.g., 'review', 'debug', 'refactor')"
                    }
                },
                "required": ["command"]
            }),
        )
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        let command = call
            .arguments
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        debug!("invoke_skill: command={}", command);

        if command.is_empty() {
            return ToolResult::error("invoke_skill", "command is required");
        }

        // Discover all available skills
        let skills = match SkillCatalog::discover(&self.workspace_root, &[]) {
            Ok(skills) => skills,
            Err(e) => {
                error!("invoke_skill: failed to discover skills: {}", e);
                return ToolResult::error(
                    "invoke_skill",
                    &format!("Failed to discover skills: {}", e),
                );
            }
        };

        // Find the skill by command
        let skill = skills.iter().find(|s| s.command == command);

        match skill {
            Some(skill) => {
                info!(
                    "invoke_skill: loaded skill '{}' from {:?}",
                    command, skill.source
                );
                let body = skill.expanded_body();
                ToolResult::success(
                    "invoke_skill",
                    &format!(
                        "Skill `{}` loaded. Use the following instructions:\n\n{}\n\n---\n\nSkill Metadata:\n- Name: {}\n- Description: {}\n- Source: {}\n- Context: {:?}\n",
                        command,
                        body,
                        skill.name,
                        skill.description.as_deref().unwrap_or("N/A"),
                        skill.source_label(),
                        skill.context
                    ),
                )
            }
            None => {
                // List available skills for better error message
                let available: Vec<String> = skills.iter().map(|s| s.summary_line()).collect();
                let available_str = if available.is_empty() {
                    "No skills found.".to_string()
                } else {
                    available.join("\n")
                };
                error!("invoke_skill: skill '{}' not found", command);
                ToolResult::error(
                    "invoke_skill",
                    &format!(
                        "Skill '{}' not found. Available skills:\n{}",
                        command, available_str
                    ),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_call(command: &str) -> ToolCall {
        ToolCall {
            name: "invoke_skill".to_string(),
            arguments: serde_json::json!({
                "command": command
            }),
        }
    }

    #[tokio::test]
    async fn invoke_skill_returns_error_for_nonexistent_skill() {
        let dir = tempfile::tempdir().unwrap();
        let tool = InvokeSkillTool::new(dir.path().to_path_buf());

        let result = tool.execute(&make_call("nonexistent")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn invoke_skill_returns_error_for_empty_command() {
        let dir = tempfile::tempdir().unwrap();
        let tool = InvokeSkillTool::new(dir.path().to_path_buf());

        let result = tool.execute(&make_call("")).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("command is required"));
    }

    #[tokio::test]
    async fn invoke_skill_discovers_agents_md_skill() {
        // Create a temporary workspace with AGENTS.md
        let dir = tempfile::tempdir().unwrap();
        let agents_md = r#"## Code Review

- Review code for bugs and issues

Review the code carefully.
"#;
        fs::write(dir.path().join("AGENTS.md"), agents_md).unwrap();

        let tool = InvokeSkillTool::new(dir.path().to_path_buf());
        let result = tool.execute(&make_call("code-review")).await;

        assert!(result.success);
        assert!(result.output.contains("Review the code carefully"));
    }
}
