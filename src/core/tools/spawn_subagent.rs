/// Tool for spawning a sub-agent session for parallel task execution

use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info};

use super::types::{ToolCall, ToolDefinition, ToolResult};

/// Request sent to spawn a child agent session
#[derive(Debug)]
pub struct SpawnRequest {
    pub task: String,
    pub focus_files: Vec<String>,
    pub use_worktree: bool,
    pub reply: oneshot::Sender<SpawnResponse>,
}

/// Response from the runtime after spawning a child session
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpawnResponse {
    pub child_session_id: String,
    pub status: String,
    pub output: String,
    pub workspace: String,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
}

/// Tool that spawns a sub-agent to handle tasks in parallel
pub struct SpawnSubagentTool {
    spawn_tx: Option<mpsc::Sender<SpawnRequest>>,
}

impl SpawnSubagentTool {
    pub fn new() -> Self {
        // In a full implementation, this would receive a channel from the runtime
        Self { spawn_tx: None }
    }

    /// Create with a spawn channel for actual sub-agent spawning
    pub fn with_channel(tx: mpsc::Sender<SpawnRequest>) -> Self {
        Self { spawn_tx: Some(tx) }
    }

    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "spawn_subagent",
            "Spawn a sub-agent that runs as a separate session to handle a specific task in parallel. \
            The sub-agent inherits the conversation context and workspace. Use this to delegate \
            independent tasks (e.g., creating files, running builds) to child agents that work \
            in isolated git worktrees.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "A clear, self-contained description of what the sub-agent should do"
                    },
                    "focus_files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of file paths the sub-agent should focus on"
                    },
                    "use_worktree": {
                        "type": "boolean",
                        "description": "If true, the sub-agent runs in an isolated git worktree branch (default: true)"
                    }
                },
                "required": ["task"]
            }),
        )
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        let task = call
            .arguments
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if task.is_empty() {
            return ToolResult::error("spawn_subagent", "task is required");
        }

        let focus_files: Vec<String> = call
            .arguments
            .get("focus_files")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let use_worktree = call
            .arguments
            .get("use_worktree")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        debug!(
            "spawn_subagent: task='{}', focus_files={}, use_worktree={}",
            task,
            focus_files.len(),
            use_worktree
        );

        // If we have a spawn channel, try to spawn a real sub-agent
        if let Some(ref tx) = self.spawn_tx {
            let (reply_tx, reply_rx) = oneshot::channel();

            let req = SpawnRequest {
                task,
                focus_files,
                use_worktree,
                reply: reply_tx,
            };

            if tx.send(req).await.is_err() {
                return ToolResult::error(
                    "spawn_subagent",
                    "Sub-agent spawner is not available (channel closed)",
                );
            }

            // Wait for response with timeout
            match tokio::time::timeout(std::time::Duration::from_secs(600), reply_rx).await {
                Ok(Ok(response)) => {
                    let output = serde_json::to_string_pretty(&response).unwrap_or_default();
                    let success = response.status == "completed";
                    info!(
                        "spawn_subagent: child {} finished with status {}",
                        response.child_session_id, response.status
                    );
                    ToolResult {
                        name: "spawn_subagent".to_string(),
                        success,
                        output,
                        error: if success {
                            None
                        } else {
                            Some(format!(
                                "Sub-agent finished with status: {}",
                                response.status
                            ))
                        },
                        call_id: None,
                    }
                }
                Ok(Err(_)) => ToolResult::error(
                    "spawn_subagent",
                    "Sub-agent spawner dropped the reply channel",
                ),
                Err(_) => {
                    ToolResult::error("spawn_subagent", "Sub-agent timed out after 600 seconds")
                }
            }
        } else {
            // No spawn channel - return a simulated response for testing/non-interactive mode
            info!(
                "spawn_subagent: No spawner available, returning simulated response for task: {}",
                task
            );

            let simulated_response = SpawnResponse {
                child_session_id: format!("sim-{}-{}", std::process::id(), rand_id()),
                status: "completed".to_string(),
                output: format!(
                    "Simulated sub-agent completed task: {}\n\
                    Focus files: {:?}\n\
                    Use worktree: {}\n\
                    \n\
                    Note: This is a simulated response. In production, the spawner \
                    would create an actual child agent session.",
                    task, focus_files, use_worktree
                ),
                workspace: ".".to_string(),
                branch: Some("main".to_string()),
                worktree_path: None,
            };

            let output = serde_json::to_string_pretty(&simulated_response).unwrap_or_default();
            ToolResult {
                name: "spawn_subagent".to_string(),
                success: true,
                output,
                error: None,
                call_id: None,
            }
        }
    }

    /// Check if spawner is available
    pub fn is_available(&self) -> bool {
        self.spawn_tx.is_some()
    }
}

impl Default for SpawnSubagentTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate a random ID for simulated sessions
fn rand_id() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    nanos % 10000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(task: &str, focus_files: Vec<&str>, use_worktree: bool) -> ToolCall {
        ToolCall {
            name: "spawn_subagent".to_string(),
            arguments: serde_json::json!({
                "task": task,
                "focus_files": focus_files,
                "use_worktree": use_worktree
            }),
        }
    }

    #[tokio::test]
    async fn spawn_subagent_requires_task() {
        let tool = SpawnSubagentTool::new();
        let result = tool.execute(&make_call("", vec![], true)).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("task is required"));
    }

    #[tokio::test]
    async fn spawn_subagent_returns_simulated_response_when_no_spawner() {
        let tool = SpawnSubagentTool::new();
        let result = tool
            .execute(&make_call("Build the project", vec!["src/main.rs"], true))
            .await;

        assert!(result.success);
        assert!(result.output.contains("Build the project"));
        assert!(result.output.contains("sim-"));
        assert!(result.output.contains("completed"));
    }

    #[tokio::test]
    async fn spawn_subagent_parses_focus_files() {
        let tool = SpawnSubagentTool::new();
        let result = tool
            .execute(&make_call(
                "Test files",
                vec!["tests/test1.rs", "tests/test2.rs"],
                false,
            ))
            .await;

        assert!(result.success);
        assert!(result.output.contains("test1.rs"));
        assert!(result.output.contains("test2.rs"));
    }

    #[tokio::test]
    async fn spawn_subagent_includes_worktree_setting() {
        let tool = SpawnSubagentTool::new();
        let result = tool
            .execute(&make_call("Task with worktree", vec![], true))
            .await;

        assert!(result.success);
        assert!(result.output.contains("Use worktree: true"));
    }

    #[tokio::test]
    async fn spawn_subagent_includes_task_in_output() {
        let tool = SpawnSubagentTool::new();
        let result = tool
            .execute(&make_call("Run cargo test", vec![], true))
            .await;

        assert!(result.success);
        assert!(result.output.contains("Run cargo test"));
    }

    #[tokio::test]
    async fn spawn_subagent_empty_focus_files() {
        let tool = SpawnSubagentTool::new();
        let result = tool.execute(&make_call("Clean build", vec![], true)).await;

        assert!(result.success);
        assert!(result.output.contains("Focus files: []"));
    }

    #[tokio::test]
    async fn spawn_subagent_definition_has_correct_schema() {
        let def = SpawnSubagentTool::definition();
        assert_eq!(def.name, "spawn_subagent");
        assert!(def.description.contains("Spawn a sub-agent"));
        assert!(def.description.contains("parallel"));
    }

    #[tokio::test]
    async fn spawn_subagent_is_available_returns_false_by_default() {
        let tool = SpawnSubagentTool::new();
        assert!(!tool.is_available());
    }
}
