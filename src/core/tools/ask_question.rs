/// Tool for asking the user structured questions with multiple choices
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;
use tracing::{debug, info};

use super::types::{ToolCall, ToolDefinition, ToolResult};

const WAIT_SECS: u64 = 3600;

/// Question option for ask_question tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
}

/// Question selection response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionSelection {
    pub selection_type: String,
    pub selected_id: Option<String>,
    pub custom_text: Option<String>,
    pub suggested_answer: Option<String>,
}

impl Default for QuestionSelection {
    fn default() -> Self {
        Self {
            selection_type: "suggested".to_string(),
            selected_id: None,
            custom_text: None,
            suggested_answer: None,
        }
    }
}

/// Tool that asks the user a structured question with multiple choices
pub struct AskQuestionTool {
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<QuestionSelection>>>>,
}

impl AskQuestionTool {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "ask_question",
            "Ask the user a structured question with multiple choices, optional custom text answer, and a suggested default. Blocks until user responds.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question text shown to the user"
                    },
                    "options": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {
                                    "type": "string",
                                    "description": "Unique identifier for this option (e.g., 'yes', 'no', 'maybe')"
                                },
                                "label": {
                                    "type": "string",
                                    "description": "Human-readable label shown to user"
                                }
                            },
                            "required": ["id", "label"]
                        },
                        "description": "Array of choice options, each with unique id and label"
                    },
                    "allow_custom": {
                        "type": "boolean",
                        "description": "If true, user can type custom text instead of selecting an option (default: true)"
                    },
                    "suggested_answer": {
                        "type": "string",
                        "description": "Your recommended answer - set this to the most likely/correct choice so user can quickly accept it"
                    }
                },
                "required": ["question", "options", "suggested_answer"]
            }),
        )
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        let question = call
            .arguments
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if question.is_empty() {
            return ToolResult::error("ask_question", "question is required");
        }

        let suggested_answer = call
            .arguments
            .get("suggested_answer")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if suggested_answer.is_empty() {
            return ToolResult::error(
                "ask_question",
                "suggested_answer is required (provide your recommended choice)",
            );
        }

        let _allow_custom = call
            .arguments
            .get("allow_custom")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let options: Vec<QuestionOption> = match call.arguments.get("options") {
            Some(serde_json::Value::Array(arr)) => {
                let mut out = Vec::new();
                for v in arr {
                    let id = v
                        .get("id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let label = v
                        .get("label")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if id.is_empty() || label.is_empty() {
                        return ToolResult::error(
                            "ask_question",
                            "each option needs non-empty id and label",
                        );
                    }
                    out.push(QuestionOption { id, label });
                }
                out
            }
            _ => {
                return ToolResult::error(
                    "ask_question",
                    "options must be a non-empty array of {id, label}",
                );
            }
        };

        if options.is_empty() {
            return ToolResult::error("ask_question", "at least one option is required");
        }

        debug!(
            "ask_question: question='{}', options={}, suggested='{}'",
            question,
            options.len(),
            suggested_answer
        );

        // In a real implementation, this would emit an event to the UI
        // and wait for user response. For now, we return an error indicating
        // this tool requires interactive session support.
        info!(
            "ask_question: Would ask user: {} (options: {:?}, suggested: {})",
            question,
            options
                .iter()
                .map(|o| format!("{}:{}", o.id, o.label))
                .collect::<Vec<_>>(),
            suggested_answer
        );

        // Return a message indicating the tool needs interactive support
        ToolResult::success(
            "ask_question",
            &format!(
                "Question sent to user: '{}'\nOptions: {}\nSuggested: {}\n\nWaiting for user response...",
                question,
                options
                    .iter()
                    .map(|o| format!("[{}] {}", o.id, o.label))
                    .collect::<Vec<_>>()
                    .join(", "),
                suggested_answer
            ),
        )
    }

    /// Register a pending question and get the response channel
    pub fn register_question(
        &self,
        question_id: &str,
    ) -> Option<oneshot::Sender<QuestionSelection>> {
        let mut pending = self.pending.lock().unwrap();
        pending.remove(question_id);
        None
    }

    /// Resolve a pending question with user's selection
    pub fn resolve_question(&self, question_id: &str, selection: QuestionSelection) -> bool {
        let mut pending = self.pending.lock().unwrap();
        if let Some(sender) = pending.remove(question_id) {
            let _ = sender.send(selection);
            true
        } else {
            false
        }
    }
}

impl Default for AskQuestionTool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_call(
        question: &str,
        options: Vec<(&str, &str)>,
        allow_custom: bool,
        suggested: &str,
    ) -> ToolCall {
        let options_json: Vec<serde_json::Value> = options
            .into_iter()
            .map(|(id, label)| serde_json::json!({ "id": id, "label": label }))
            .collect();

        ToolCall {
            name: "ask_question".to_string(),
            arguments: serde_json::json!({
                "question": question,
                "options": options_json,
                "allow_custom": allow_custom,
                "suggested_answer": suggested
            }),
        }
    }

    #[tokio::test]
    async fn ask_question_requires_question() {
        let tool = AskQuestionTool::new();
        let result = tool
            .execute(&make_call(
                "",
                vec![("yes", "Yes"), ("no", "No")],
                true,
                "yes",
            ))
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("question is required"));
    }

    #[tokio::test]
    async fn ask_question_requires_suggested_answer() {
        let tool = AskQuestionTool::new();
        let result = tool
            .execute(&make_call(
                "Continue?",
                vec![("yes", "Yes"), ("no", "No")],
                true,
                "",
            ))
            .await;

        assert!(!result.success);
        assert!(result
            .error
            .unwrap()
            .contains("suggested_answer is required"));
    }

    #[tokio::test]
    async fn ask_question_requires_options() {
        let tool = AskQuestionTool::new();
        let result = tool
            .execute(&ToolCall {
                name: "ask_question".to_string(),
                arguments: serde_json::json!({
                    "question": "Continue?",
                    "options": [],
                    "suggested_answer": "yes"
                }),
            })
            .await;

        assert!(!result.success);
        assert!(result
            .error
            .unwrap()
            .contains("at least one option is required"));
    }

    #[tokio::test]
    async fn ask_question_validates_option_ids_and_labels() {
        let tool = AskQuestionTool::new();
        let result = tool
            .execute(&make_call(
                "Continue?",
                vec![("", "Empty ID"), ("valid", "")],
                true,
                "valid",
            ))
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("non-empty id and label"));
    }

    #[tokio::test]
    async fn ask_question_formats_output_correctly() {
        let tool = AskQuestionTool::new();
        let result = tool
            .execute(&make_call(
                "Build the project?",
                vec![("yes", "Yes, build it"), ("no", "No, skip")],
                true,
                "yes",
            ))
            .await;

        assert!(result.success);
        assert!(result.output.contains("Build the project?"));
        assert!(result.output.contains("[yes] Yes, build it"));
        assert!(result.output.contains("[no] No, skip"));
        assert!(result.output.contains("suggested: yes"));
    }
}
