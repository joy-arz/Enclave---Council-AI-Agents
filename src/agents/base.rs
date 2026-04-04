use std::sync::Arc;
use std::path::PathBuf;
use crate::core::model_provider;
use crate::core::providers_mod::StreamChunk;
use crate::core::tools::execute_tool;
use crate::core::BusyState;
use crate::core::approval::ApprovalPolicy;

#[derive(Debug, Clone)]
#[allow(non_camel_case_types)]
pub struct agent_result {
    pub response: String,
    pub tool_calls: Vec<tool_call_result>,
}

#[derive(Debug, Clone)]
#[allow(non_camel_case_types)]
pub struct tool_call_result {
    pub name: String,
    pub status: String,  // "running", "success", "error"
    pub output: Option<String>,
}

#[allow(non_camel_case_types)]
pub struct base_agent {
    pub name: String,
    pub role: String,
    pub system_prompt: String,
    pub provider: Arc<dyn model_provider>,
    pub model_name: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub is_autonomous: bool,
    pub workspace_dir: PathBuf,
    pub max_tool_iterations: usize,
    // Infinite loop protection: track consecutive failures per tool
    consecutive_tool_failures: usize,
    last_failed_tool: Option<String>,
    // Busy state for UI
    pub busy_state: BusyState,
    // Approval policy for non-autonomous mode
    approval_policy: Option<ApprovalPolicy>,
}

#[allow(non_camel_case_types)]
impl base_agent {
    pub fn new(
        name: &str,
        role: &str,
        system_prompt: &str,
        provider: Arc<dyn model_provider>,
        model_name: &str,
        temperature: f32,
        max_tokens: u32,
    ) -> Self {
        Self {
            name: name.to_string(),
            role: role.to_string(),
            system_prompt: system_prompt.to_string(),
            provider,
            model_name: model_name.to_string(),
            temperature,
            max_tokens,
            is_autonomous: false,
            workspace_dir: PathBuf::from("."),
            max_tool_iterations: 5,
            consecutive_tool_failures: 0,
            last_failed_tool: None,
            busy_state: BusyState::Idle,
            approval_policy: None,
        }
    }

    /// Update the busy state
    pub fn set_busy_state(&mut self, state: BusyState) {
        self.busy_state = state;
    }

    pub fn set_autonomous(&mut self, value: bool) {
        self.is_autonomous = value;
    }

    pub fn set_approval_policy(&mut self, policy: ApprovalPolicy) {
        self.approval_policy = Some(policy);
    }

    #[allow(dead_code)]
    pub fn set_workspace(&mut self, dir: PathBuf) {
        self.workspace_dir = dir;
    }

    pub fn build_full_system_prompt(&self) -> String {
        let safety_instruction = if self.is_autonomous {
            "you are in AUTONOMOUS mode. you have full permission to use tools to directly modify the codebase. your goal is to drive the workflow forward until the task is complete. you have full access to the workspace root and all its subfolders recursively. do not just talk; ACT by reading and editing any necessary files in the workspace (including nested folders). assume ALL changes are pre-accepted and proceed immediately with implementation."
        } else {
            "you are in PROPOSAL mode. you must not modify files directly. instead, you should provide your suggested changes in your response using the following format:\n\n[PROPOSE_CHANGE:path/to/file]\n[new content of the file]\n[/PROPOSE_CHANGE]\n\nthe user will review these proposals and choose whether to apply them. you can propose multiple file changes in a single response."
        };

        // Include actual tool definitions in JSON format
        let tools_json = crate::core::tools::get_tools_json();

        let tools_instruction = format!(r#"AVAILABLE TOOLS (MUST use JSON format when calling tools):
{}

RESPONSE RULES (CRITICAL):
1. Keep responses SHORT - maximum 2-3 sentences
2. If using a tool, output ONLY the JSON object and nothing else
3. Do NOT explain what you are about to do or what you did - just do it
4. NEVER start with "Based on...", "Looking at...", "The user wants me to..." - just answer directly
5. Output JSON in this exact format: {{"name": "tool_name", "arguments": {{"param": "value"}}}}

Examples of GOOD responses:
- "The project has Cargo.toml, src/, and tests/."
- {{"name": "list_directory", "arguments": {{"path": "."}}}}
- "There's a bug in line 42 - missing null check."

Examples of BAD responses (do not do these):
- "Based on my analysis of the workspace, I can see that..."
- "The user wants me to list files, so let me do that by calling..."
- "Looking at the previous tool results, I notice that...""#, tools_json);

        format!(
            "you are a {}.\n{}\n\nresponsibilities:\n{}\n\n{}",
            self.role, safety_instruction, self.system_prompt, tools_instruction
        )
    }

    /// Execute a response with tool calls, continuing until no more tool calls or max iterations reached
    pub async fn get_response_with_tools(&mut self, history: &str) -> Result<agent_result, anyhow::Error> {
        let mut current_history = history.to_string();
        let mut iterations = 0;
        let mut final_text = String::new();
        let mut all_tool_calls: Vec<tool_call_result> = Vec::new();

        // Get tools JSON for API providers
        let tools_json = crate::core::tools::get_tools_json();

        loop {
            iterations += 1;
            if iterations > self.max_tool_iterations {
                final_text.push_str("\n[Max tool iterations reached]");
                break;
            }

            let prompt = format!(
                "current conversation history:\n{}\n\nrespond as the {}. use tools if needed to complete the task.",
                current_history, self.name
            );

            // Use streaming API to get real-time tool events
            let mut rx = self.provider.call_model_streaming(
                &self.model_name,
                &prompt,
                Some(&self.build_full_system_prompt()),
                self.temperature,
                self.max_tokens,
                Some(&tools_json),
            ).await?;

            // Streaming state
            let mut current_tool_input = String::new();
            let mut text_buffer = String::new();
            let mut tool_calls_from_stream: Vec<(String, String)> = Vec::new(); // (name, input)

            // Process stream
            while let Some(chunk) = rx.recv().await {
                match chunk {
                    StreamChunk::TextDelta(text) => {
                        text_buffer.push_str(&text);
                    }
                    StreamChunk::ToolUse { name, input, .. } => {
                        // Tool call detected - capture name and input
                        current_tool_input.clear();
                        tool_calls_from_stream.push((name, input.to_string()));
                    }
                    StreamChunk::ToolInputDelta(delta) => {
                        current_tool_input.push_str(&delta);
                    }
                    StreamChunk::Usage { .. } => {}
                    StreamChunk::Done => break,
                    StreamChunk::Error(e) => {
                        tracing::warn!("Streaming error: {}", e);
                        break;
                    }
                    StreamChunk::ThinkingDelta(_) => {
                        // Ignore thinking blocks for now
                    }
                }
            }

            // Add accumulated text to final text
            let trimmed_text = text_buffer.trim();
            if !trimmed_text.is_empty() {
                if !final_text.is_empty() {
                    final_text.push('\n');
                }
                final_text.push_str(trimmed_text);
            }

            // Execute tool calls found in stream
            if tool_calls_from_stream.is_empty() {
                // No tool calls - this is the final response
                break;
            }

            // Execute each tool and collect results
            let mut tool_results = Vec::new();
            for (name, input) in &tool_calls_from_stream {
                let args: serde_json::Map<String, serde_json::Value> = if input.is_empty() {
                    serde_json::Map::new()
                } else {
                    serde_json::from_str(input).unwrap_or_else(|_| serde_json::Map::new())
                };
                
                let call = crate::core::tools::ToolCall {
                    name: name.clone(),
                    arguments: serde_json::Value::Object(args),
                };

                // In autonomous mode, pass None (no approval needed)
                // In non-autonomous mode, use approval_policy if set
                let policy_ref = if self.is_autonomous {
                    None
                } else {
                    self.approval_policy.as_ref()
                };

                let result = execute_tool(&call, &self.workspace_dir, policy_ref).await;

                // Handle pending approval in non-autonomous mode
                if !self.is_autonomous {
                    if let Some(ref err) = result.error {
                        if err == "PENDING_APPROVAL" {
                            // Return special result indicating pending approval
                            tool_results.push(crate::core::tools::ToolResult {
                                name: name.clone(),
                                success: false,
                                output: String::new(),
                                error: Some(format!("[Approval Required] Tool '{}' requires approval. Suggest using autonomous mode or adding to allow list.", name)),
                            });
                            continue;
                        }
                    }
                }

                tool_results.push(result);
            }

            // Format tool results for the next iteration
            let tool_results_str = tool_results.iter()
                .map(|r| {
                    if r.success {
                        format!("[{}]\n{}", r.name, r.output)
                    } else {
                        format!("[{} Error]\n{}", r.name, r.error.as_ref().unwrap_or(&"Unknown error".to_string()))
                    }
                })
                .collect::<Vec<_>>()
                .join("\n---\n");

            current_history.push_str(&format!(
                "\n\n[Tool Results]\n{}\n\n[End Results]",
                tool_results_str
            ));

            // Build tool info for display
            for (i, result) in tool_results.iter().enumerate() {
                all_tool_calls.push(tool_call_result {
                    name: tool_calls_from_stream[i].0.clone(),
                    status: if result.success { "success".to_string() } else { "error".to_string() },
                    output: Some(if result.success { result.output.clone() } else { result.error.clone().unwrap_or_default() }),
                });
            }

            // For display, summarize what was done
            if !final_text.is_empty() {
                final_text.push('\n');
            }
            let summaries: Vec<String> = tool_results.iter().map(|r| {
                if r.success {
                    format!("✓ {}", r.name)
                } else {
                    format!("✗ {}", r.name)
                }
            }).collect();
            final_text.push_str(&summaries.join(" "));

            // Infinite loop protection: track consecutive failures
            let all_same_tool = tool_calls_from_stream.iter().all(|(name, _)| {
                name == &tool_calls_from_stream.first().unwrap().0
            });
            let all_failed = tool_results.iter().all(|r| !r.success);

            if all_same_tool && all_failed && tool_calls_from_stream.len() == 1 {
                if let Some(ref last) = self.last_failed_tool {
                    if *last == tool_calls_from_stream.first().unwrap().0 {
                        self.consecutive_tool_failures += 1;
                    } else {
                        self.consecutive_tool_failures = 1;
                        self.last_failed_tool = Some(tool_calls_from_stream.first().unwrap().0.clone());
                    }
                } else {
                    self.consecutive_tool_failures = 1;
                    self.last_failed_tool = Some(tool_calls_from_stream.first().unwrap().0.clone());
                }

                if self.consecutive_tool_failures >= 3 {
                    final_text.push_str(&format!(
                        "\n[Infinite tool loop detected: {} failed 3 times consecutively. Stopping.]",
                        tool_calls_from_stream.first().unwrap().0
                    ));
                    break;
                }
            } else {
                self.consecutive_tool_failures = 0;
                self.last_failed_tool = None;
            }
        }

        Ok(agent_result {
            response: final_text,
            tool_calls: all_tool_calls,
        })
    }

    /// Simple response without tool execution
    #[allow(dead_code)]
    pub async fn get_response(&self, history: &str) -> Result<agent_result, anyhow::Error> {
        let prompt = format!(
            "current conversation history:\n{}\n\nrespond as the {}. provide your actual response, actions taken, or findings - not just a plan of what you will do.",
            history, self.name
        );

        let (response, _) = self.provider.call_model(
            &self.model_name,
            &prompt,
            Some(&self.build_full_system_prompt()),
            self.temperature,
            self.max_tokens,
            None,  // No tools for simple responses
        ).await?;

        Ok(agent_result {
            response,
            tool_calls: vec![],
        })
    }

    pub fn clone_for_parallel(&self) -> Self {
        Self {
            name: self.name.clone(),
            role: self.role.clone(),
            system_prompt: self.system_prompt.clone(),
            provider: self.provider.clone(),
            model_name: self.model_name.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            is_autonomous: self.is_autonomous,
            workspace_dir: self.workspace_dir.clone(),
            max_tool_iterations: self.max_tool_iterations,
            consecutive_tool_failures: 0,
            last_failed_tool: None,
            busy_state: BusyState::Idle,
            approval_policy: self.approval_policy.clone(),
        }
    }
}
