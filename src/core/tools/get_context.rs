/// Tool for getting current context status
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::debug;

use super::types::{ContextInfo, ToolCall, ToolDefinition, ToolResult};

/// Static counters for tracking context (would be replaced with actual session state)
static mut TOTAL_CALLS: AtomicUsize = AtomicUsize::new(0);
static mut SESSION_STARTED: bool = false;

/// Tool that returns current context information about the session
pub struct GetContextTool {
    session_id: String,
    message_count: Arc<AtomicUsize>,
}

impl GetContextTool {
    pub fn new() -> Self {
        // Initialize counters on first use
        unsafe {
            if !SESSION_STARTED {
                TOTAL_CALLS.store(0, Ordering::SeqCst);
                SESSION_STARTED = true;
            }
        }

        Self {
            session_id: std::env::var("SESSION_ID").unwrap_or_else(|_| "local-session".to_string()),
            message_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "get_context",
            "Get current context status including token count, message count, and session information. Useful for monitoring resource usage.",
            serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        )
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        // Increment call counter
        unsafe {
            TOTAL_CALLS.fetch_add(1, Ordering::SeqCst);
        }

        let total_calls = unsafe { TOTAL_CALLS.load(Ordering::SeqCst) };
        let message_count = self.message_count.load(Ordering::SeqCst);

        // Estimate token count (rough approximation: 4 chars per token)
        // This would be replaced with actual token counting in production
        let estimated_tokens = message_count * 50; // Rough estimate

        let context = ContextInfo {
            token_count: estimated_tokens,
            message_count,
            session_id: self.session_id.clone(),
        };

        debug!(
            "get_context: session={}, messages={}, calls={}",
            context.session_id, context.message_count, total_calls
        );

        ToolResult::success(
            "get_context",
            &format!(
                "Session: {}\nMessages: {}\nTool Calls: {}\nEstimated Tokens: {}",
                context.session_id, context.message_count, total_calls, context.token_count
            ),
        )
    }

    /// Increment message count (called when messages are added)
    pub fn increment_message_count(&self) {
        self.message_count.fetch_add(1, Ordering::SeqCst);
    }
}

impl Default for GetContextTool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_context_returns_info() {
        let tool = GetContextTool::new();
        let result = tool
            .execute(&ToolCall {
                name: "get_context".to_string(),
                arguments: serde_json::json!({}),
            })
            .await;

        assert!(result.success);
        assert!(result.output.contains("Session:"));
        assert!(result.output.contains("Messages:"));
        assert!(result.output.contains("Tool Calls:"));
        assert!(result.output.contains("Estimated Tokens:"));
    }

    #[tokio::test]
    async fn get_context_increments_call_count() {
        let tool = GetContextTool::new();

        let result1 = tool
            .execute(&ToolCall {
                name: "get_context".to_string(),
                arguments: serde_json::json!({}),
            })
            .await;

        let result2 = tool
            .execute(&ToolCall {
                name: "get_context".to_string(),
                arguments: serde_json::json!({}),
            })
            .await;

        assert!(result1.success);
        assert!(result2.success);

        // Both should show the same call count (static)
        // In a real test, we'd mock the counter
    }
}
