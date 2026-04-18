//! Event system for the Enclave agent
//! Provides structured event types with sequential IDs and timestamps

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Represents the current state of the agent for UI display
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BusyState {
    #[default]
    Idle,
    Thinking,
    Streaming,
    ToolRunning,
    ApprovalPending,
    Error,
}

/// Agent events for streaming and logging
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
#[allow(dead_code)]
pub enum AgentEvent {
    // Session lifecycle
    SessionStarted {
        session_id: String,
        workspace: String,
        model: String,
    },
    SessionEnded {
        reason: String,
    },

    // Message events
    MessageReceived {
        role: String,
        content: String,
    },

    // Streaming events
    TokensStreamed {
        delta: String,
    },

    // Tool events
    ToolCallStarted {
        call_id: String,
        tool: String,
        input: serde_json::Value,
    },
    ToolCallCompleted {
        call_id: String,
        output: String,
        success: bool,
    },
    ToolCallFailed {
        call_id: String,
        error: String,
    },

    // Approval events
    ApprovalRequested {
        call_id: String,
        tool: String,
        description: String,
    },
    ApprovalResolved {
        call_id: String,
        approved: bool,
    },

    // Cost/usage events
    CostUpdated {
        input_tokens: u64,
        output_tokens: u64,
        estimated_cost_usd: Option<f64>,
    },

    // Checkpoint/events for long operations
    Checkpoint {
        phase: String,
        detail: String,
        turn: u32,
    },

    // Error events
    Error {
        message: String,
    },

    // Hierarchical session events
    ChildSessionSpawned {
        child_session_id: String,
        task: String,
    },
    ChildSessionActivity {
        child_session_id: String,
        phase: String,
        detail: String,
    },
    ChildSessionCompleted {
        child_session_id: String,
        success: bool,
    },

    // Context management
    ContextWarning {
        message: String,
    },
    ContextCompaction {
        phase: String,
        message: String,
    },

    // UI state
    BusyStateChanged {
        state: BusyState,
    },

    // Question/clarification requests
    QuestionRequested {
        question: String,
    },
    QuestionResolved {
        question_id: String,
        selection: String,
    },
}

/// Wrapper for events with ordering metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct EventEnvelope {
    /// Sequential event ID for ordering
    pub id: u64,
    /// Timestamp when event was created
    pub ts: Option<DateTime<Utc>>,
    /// The actual event
    pub event: AgentEvent,
}

impl EventEnvelope {
    /// Create a new envelope with auto-incrementing ID
    #[allow(dead_code)]
    pub fn new(id: u64, event: AgentEvent) -> Self {
        Self {
            id,
            ts: Some(Utc::now()),
            event,
        }
    }

    /// Create a new envelope with current timestamp
    #[allow(dead_code)]
    pub fn with_now(id: u64, event: AgentEvent) -> Self {
        Self {
            id,
            ts: Some(Utc::now()),
            event,
        }
    }
}

/// Counter for generating sequential event IDs
#[derive(Default)]
#[allow(dead_code)]
pub struct EventIdCounter(u64);

impl EventIdCounter {
    #[allow(dead_code)]
    pub fn next(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }

    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.0 = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_envelope_creation() {
        let event = AgentEvent::SessionStarted {
            session_id: "test".to_string(),
            workspace: "/tmp".to_string(),
            model: "test-model".to_string(),
        };
        let envelope = EventEnvelope::new(1, event.clone());
        assert_eq!(envelope.id, 1);
        assert!(envelope.ts.is_some());
    }

    #[test]
    fn test_busy_state_default() {
        assert_eq!(BusyState::default(), BusyState::Idle);
    }
}
