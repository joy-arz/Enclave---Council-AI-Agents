use crate::core::approval::ApprovalPolicy;
use crate::core::model_provider;
use crate::core::providers_mod::StreamChunk;
use crate::core::tools::execute_tool;
use crate::core::BusyState;
use regex::Regex;
use std::path::PathBuf;
use std::sync::Arc;

/// Parse tool calls from text content as a fallback for APIs that don't send proper tool_use events
/// Handles multiple formats including MiniMax's pseudo-JSON with nested braces
fn parse_tool_calls_from_text(text: &str) -> Vec<(String, String)> {
    let mut tool_calls = Vec::new();

    let re_name = Regex::new(r#"tool\s*=>\s*"([^"]+)""#).ok();
    let re_arg = Regex::new(r#"--(\w+)\s+""#).ok();

    // Try to find complete tool call objects in the text
    // Match [TOOL_CALL] blocks - end is marked by [/TOOL_CALL]
    let mut search_start = 0;
    while let Some(tool_start) = text[search_start..].find("[TOOL_CALL]") {
        let abs_tool_start = search_start + tool_start;
        let after_tool_call = &text[abs_tool_start + 10..];

        // Find the closing [/TOOL_CALL] as the delimiter
        let block_end = after_tool_call
            .find("[/TOOL_CALL]")
            .unwrap_or(after_tool_call.len());

        let block_content = &after_tool_call[..block_end];

        // Try to parse the block as JSON
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(block_content) {
            let name = json
                .get("name")
                .or_else(|| json.get("tool"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let args = json
                .get("args")
                .or_else(|| json.get("arguments"))
                .or_else(|| json.get("input"))
                .map(|v| v.to_string())
                .unwrap_or_else(|| "{}".to_string());

            if !name.is_empty() {
                tool_calls.push((name.to_string(), args));
            }
        } else {
            // Try MiniMax pseudo-JSON format: {tool => "name", args => {...}}
            if let Some(ref re) = re_name {
                if let Some(cap) = re.captures(block_content) {
                    if let Some(name_match) = cap.get(1) {
                        let tool_name = name_match.as_str().to_string();

                        // Extract args by finding balanced braces after args =>
                        let mut args_text = String::new();
                        if let Some(args_pos) = block_content.find("args") {
                            let after_args = &block_content[args_pos..];
                            if let Some(eq_pos) = after_args.find("=>") {
                                let after_eq = &after_args[eq_pos + 2..].trim_start();
                                if after_eq.starts_with('{') {
                                    let mut depth = 0;
                                    let mut end_pos = 0;
                                    let mut in_str = false;
                                    let mut esc = false;
                                    for (i, c) in after_eq.chars().enumerate() {
                                        if esc {
                                            esc = false;
                                            continue;
                                        }
                                        if c == '\\' {
                                            esc = true;
                                            continue;
                                        }
                                        if c == '"' {
                                            in_str = !in_str;
                                            continue;
                                        }
                                        if in_str {
                                            continue;
                                        }
                                        if c == '{' {
                                            if depth == 0 {
                                                end_pos = i;
                                            }
                                            depth += 1;
                                        } else if c == '}' {
                                            depth -= 1;
                                            if depth == 0 {
                                                end_pos = i + 1;
                                                break;
                                            }
                                        }
                                    }
                                    args_text = after_eq[..end_pos].to_string();
                                }
                            }
                        }

                        // Parse individual --key "value" arguments, handling escaped quotes
                        let mut args_map = serde_json::Map::new();

                        // Find --key "value" patterns, being careful with escaped quotes
                        if let Some(ref re_a) = re_arg {
                            for arg_cap in re_a.captures_iter(&args_text) {
                                if let Some(key_match) = arg_cap.get(1) {
                                    let key = key_match.as_str().to_string();
                                    // Find the opening quote
                                    let after_key_and_quote =
                                        &args_text[arg_cap.get(0).map(|m| m.end()).unwrap_or(0)..];
                                    // Find the closing quote (unescaped) by scanning for " not preceded by \
                                    let mut value = String::new();
                                    let mut chars = after_key_and_quote.chars().peekable();
                                    while let Some(c) = chars.next() {
                                        if c == '\\' && chars.peek() == Some(&'"') {
                                            value.push('"');
                                            chars.next(); // consume the "
                                        } else if c == '"' {
                                            break; // end of value
                                        } else {
                                            value.push(c);
                                        }
                                    }
                                    args_map.insert(key, serde_json::Value::String(value));
                                }
                            }
                        }

                        tool_calls
                            .push((tool_name, serde_json::Value::Object(args_map).to_string()));
                    }
                }
            }
        }

        search_start = abs_tool_start + 10 + block_end;
    }

    // Also check for bare JSON objects with "tool" or "name" at the start
    let re_bare = Regex::new(r#"\{\s*"(?:tool|name)":\s*"([^"]+)""#).ok();
    if let Some(ref re) = re_bare {
        for cap in re.captures_iter(text) {
            if let Some(name_match) = cap.get(1) {
                let tool_name = name_match.as_str().to_string();

                // Find the full object starting from this position
                let start_pos = cap.get(0).map(|m| m.start()).unwrap_or(0);
                let after_start = &text[start_pos..];

                // Find balanced closing brace
                let mut depth = 0;
                let mut end_pos = 0;
                let mut in_str = false;
                let mut esc = false;
                for (i, c) in after_start.chars().enumerate() {
                    if esc {
                        esc = false;
                        continue;
                    }
                    if c == '\\' {
                        esc = true;
                        continue;
                    }
                    if c == '"' {
                        in_str = !in_str;
                        continue;
                    }
                    if in_str {
                        continue;
                    }
                    if c == '{' {
                        if depth == 0 {
                            end_pos = i;
                        }
                        depth += 1;
                    } else if c == '}' {
                        depth -= 1;
                        if depth == 0 {
                            end_pos = i + 1;
                            break;
                        }
                    }
                }

                if end_pos > 0 {
                    let json_str = &after_start[..end_pos];
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str) {
                        let args = json
                            .get("args")
                            .or_else(|| json.get("arguments"))
                            .or_else(|| json.get("input"))
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "{}".to_string());

                        // Only add if not already found via [TOOL_CALL] wrapper
                        if !tool_calls.iter().any(|(n, _)| n == &tool_name) {
                            tool_calls.push((tool_name, args));
                        }
                    }
                }
            }
        }
    }

    tool_calls
}

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
    pub status: String, // "running", "success", "error"
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn set_busy_state(&mut self, state: BusyState) {
        self.busy_state = state;
    }

    pub fn set_autonomous(&mut self, value: bool) {
        self.is_autonomous = value;
    }

    #[allow(dead_code)]
    pub fn set_approval_policy(&mut self, policy: ApprovalPolicy) {
        self.approval_policy = Some(policy);
    }

    #[allow(dead_code)]
    pub fn set_workspace(&mut self, dir: PathBuf) {
        self.workspace_dir = dir;
    }

    pub fn build_full_system_prompt(&self) -> String {
        let autonomy_instruction = if self.is_autonomous {
            "AUTONOMOUS MODE: You have full permission to use tools to read and modify the codebase. Read files, understand the task, and implement changes immediately. Do NOT ask for permission."
        } else {
            "PROPOSAL MODE: Do NOT modify files directly. If you make changes, describe them so the user can review. Use tools to investigate but note what you would change."
        };

        let tools_json = crate::core::tools::get_tools_json();

        let instruction = format!(
            r#"You are {}.

{}
YOUR TASK: FOCUS ON THE USER'S REQUEST - do not repeat introductions or explain your role.

{}
AVAILABLE TOOLS (use JSON format):
{}

TOOL USAGE: When you call a tool, output ONLY the JSON and nothing else. When done with tools, give your actual response.

CRITICAL RULES:
- Answer the ACTUAL question asked - do not re-introduce yourself
- Keep responses focused and direct
- If asked "what can you do?", demonstrate with examples, don't explain your role
- If asked to do something, DO IT - read files, make changes, run commands
- NEVER start with "As a [role], I..." or "I am [role]..." or "My role is..."

Examples of what to do:
- User: "what files exist?" -> Use list_directory tool, then show results
- User: "read src/main.rs" -> Use read_file tool, then show content
- User: "add error handling" -> Read relevant files, identify where to add, make changes
- User: "explain this code" -> Read code, give direct explanation

Examples of what NOT to do:
- "As a security specialist, I would be happy to..."
- "I am a code reviewer and I can help you..."
- "My role is to find bugs, so let me start by..."
"#,
            self.role, autonomy_instruction, self.system_prompt, tools_json
        );

        instruction
    }

    /// Execute a response with tool calls, continuing until no more tool calls or max iterations reached
    pub async fn get_response_with_tools(
        &mut self,
        history: &str,
    ) -> Result<agent_result, anyhow::Error> {
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

            let prompt = format!("HISTORY:\n{}\n\nTASK: {}", current_history, self.name);

            // Use streaming API to get real-time tool events
            let mut rx = self
                .provider
                .call_model_streaming(
                    &self.model_name,
                    &prompt,
                    Some(&self.build_full_system_prompt()),
                    self.temperature,
                    self.max_tokens,
                    Some(&tools_json),
                )
                .await?;

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
            if tool_calls_from_stream.is_empty() && !text_buffer.is_empty() {
                // Fallback: try to parse tool calls from text content
                // This handles APIs like MiniMax that put tool calls in thinking blocks as text
                let parsed = parse_tool_calls_from_text(&text_buffer);
                if !parsed.is_empty() {
                    tracing::debug!("Parsed {} tool calls from text content", parsed.len());
                    tool_calls_from_stream = parsed;
                }
            }

            if tool_calls_from_stream.is_empty() {
                // No tool calls found - this is the final response
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
            let tool_results_str = tool_results
                .iter()
                .map(|r| {
                    if r.success {
                        format!("[{}]\n{}", r.name, r.output)
                    } else {
                        format!(
                            "[{} Error]\n{}",
                            r.name,
                            r.error.as_ref().unwrap_or(&"Unknown error".to_string())
                        )
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
                    status: if result.success {
                        "success".to_string()
                    } else {
                        "error".to_string()
                    },
                    output: Some(if result.success {
                        result.output.clone()
                    } else {
                        result.error.clone().unwrap_or_default()
                    }),
                });
            }

            // For display, summarize what was done
            if !final_text.is_empty() {
                final_text.push('\n');
            }
            let summaries: Vec<String> = tool_results
                .iter()
                .map(|r| {
                    if r.success {
                        format!("✓ {}", r.name)
                    } else {
                        format!("✗ {}", r.name)
                    }
                })
                .collect();
            final_text.push_str(&summaries.join(" "));

            // Infinite loop protection: track consecutive failures
            let all_same_tool = tool_calls_from_stream
                .first()
                .map(|first| {
                    tool_calls_from_stream
                        .iter()
                        .all(|(name, _)| name == &first.0)
                })
                .unwrap_or(false);
            let all_failed = tool_results.iter().all(|r| !r.success);

            if all_same_tool && all_failed && tool_calls_from_stream.len() == 1 {
                let tool_name = tool_calls_from_stream.first().unwrap().0.clone();
                if let Some(ref last) = self.last_failed_tool {
                    if *last == tool_name {
                        self.consecutive_tool_failures += 1;
                    } else {
                        self.consecutive_tool_failures = 1;
                        self.last_failed_tool = Some(tool_name.clone());
                    }
                } else {
                    self.consecutive_tool_failures = 1;
                    self.last_failed_tool = Some(tool_name.clone());
                }

                if self.consecutive_tool_failures >= 3 {
                    final_text.push_str(&format!(
                        "\n[Infinite tool loop detected: {} failed 3 times consecutively. Stopping.]",
                        tool_name
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
        let prompt = format!("HISTORY:\n{}\n\nTASK: {}", history, self.name);

        let (response, _) = self
            .provider
            .call_model(
                &self.model_name,
                &prompt,
                Some(&self.build_full_system_prompt()),
                self.temperature,
                self.max_tokens,
                None, // No tools for simple responses
            )
            .await?;

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
