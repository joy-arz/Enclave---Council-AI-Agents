use std::sync::Arc;
use std::path::PathBuf;
use crate::core::model_provider;
use crate::core::tools::{execute_tool, parse_tool_calls};

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
        }
    }

    pub fn set_autonomous(&mut self, value: bool) {
        self.is_autonomous = value;
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

        let tools_instruction = "CRITICAL - TOOL USAGE:\nWhen you need to perform an action (like listing files, reading files, running commands), you MUST output the tool call in JSON format like this:\n```json\n{\"name\": \"list_directory\", \"arguments\": {\"path\": \".\"}}\n```\nYou MUST output ONLY the JSON, nothing else before or after.\nValid tool names: list_directory, read_file, write_file, run_shell_command, grep\nExample for reading a file: ```json\n{\"name\": \"read_file\", \"arguments\": {\"path\": \"Cargo.toml\"}}\n```\nExample for running a command: ```json\n{\"name\": \"run_shell_command\", \"arguments\": {\"command\": \"ls -la\"}}\n```\nIMPORTANT: Output ONLY the JSON code block, no explanations.";

        format!(
            "you are a {}.\n{}\n\nresponsibilities:\n{}\n\n{}",
            self.role, safety_instruction, self.system_prompt, tools_instruction
        )
    }

    /// Execute a response with tool calls, continuing until no more tool calls or max iterations reached
    pub async fn get_response_with_tools(&self, history: &str) -> Result<(String, String), anyhow::Error> {
        let mut current_history = history.to_string();
        let mut iterations = 0;
        let mut final_text = String::new();

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

            let (response, _raw) = self.provider.call_model(
                &self.model_name,
                &prompt,
                Some(&self.build_full_system_prompt()),
                self.temperature,
                self.max_tokens,
            ).await?;

            // Parse tool calls from response
            let tool_calls = parse_tool_calls(&response);

            // Debug: log the response and detected tool calls
            tracing::debug!("{} response (iteration {}): {} chars, {} tool calls detected",
                self.name, iterations, response.len(), tool_calls.len());
            if !tool_calls.is_empty() {
                tracing::debug!("Tool calls: {:?}", tool_calls);
            }

            if tool_calls.is_empty() {
                // No tool calls, this is the final response
                if final_text.is_empty() {
                    final_text = response;
                } else {
                    final_text.push_str("\n\n");
                    final_text.push_str(&response);
                }
                break;
            }

            // Execute tool calls and collect results
            let mut tool_results = Vec::new();
            for call in &tool_calls {
                let result = execute_tool(call, &self.workspace_dir).await;
                tool_results.push(result);
            }

            // Append tool results to history for next iteration
            let tool_results_str = tool_results.iter()
                .map(|r| {
                    if r.success {
                        format!("[{}] Success:\n{}", r.name, r.output)
                    } else {
                        format!("[{}] Error:\n{}", r.name, r.error.as_ref().unwrap_or(&"Unknown error".to_string()))
                    }
                })
                .collect::<Vec<_>>()
                .join("\n\n");

            current_history.push_str(&format!(
                "\n\n[{} used tools]\n{}\n\n[End of tool results]",
                self.name, tool_results_str
            ));

            // Keep track of text content
            if final_text.is_empty() {
                // Extract just the text before tool calls
                let text_only = extract_text_before_tools(&response);
                final_text = text_only;
            } else {
                final_text.push_str("\n\n");
                final_text.push_str(&extract_text_before_tools(&response));
            }
        }

        Ok((final_text.clone(), final_text))
    }

    /// Simple response without tool execution
    #[allow(dead_code)]
    pub async fn get_response(&self, history: &str) -> Result<(String, String), anyhow::Error> {
        let prompt = format!(
            "current conversation history:\n{}\n\nrespond as the {}. provide your actual response, actions taken, or findings - not just a plan of what you will do.",
            history, self.name
        );

        self.provider.call_model(
            &self.model_name,
            &prompt,
            Some(&self.build_full_system_prompt()),
            self.temperature,
            self.max_tokens,
        ).await
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
        }
    }
}

/// Extract text content before any tool call block
fn extract_text_before_tools(response: &str) -> String {
    // Find first tool call marker
    let markers = ["```json", "<tool_call>", "<function>", "read_file(", "write_file(", "run_shell_command(", "list_directory(", "grep("];

    let mut earliest = usize::MAX;
    for marker in &markers {
        if let Some(pos) = response.find(marker) {
            if pos < earliest {
                earliest = pos;
            }
        }
    }

    if earliest == usize::MAX {
        response.trim().to_string()
    } else {
        response[..earliest].trim().to_string()
    }
}
