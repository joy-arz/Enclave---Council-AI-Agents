use crate::utils::logger_mod::session_logger;
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Stream chunk types for streaming responses
/// Matches nca-cli's Provider streaming pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum StreamChunk {
    /// Text delta (streaming token)
    TextDelta(String),
    /// Thinking block content (for models that support it)
    ThinkingDelta(String),
    /// Tool call detected (full tool call object)
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool call input delta (partial JSON as it comes in)
    ToolInputDelta(String),
    /// Usage statistics at end of response
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    /// Done marker - response complete
    Done,
    /// Error occurred
    Error(String),
}

#[async_trait]
#[allow(non_camel_case_types)]
pub trait model_provider: Send + Sync {
    // Returns (cleaned_content, full_terminal_output)
    // tools parameter: JSON array of tool definitions to send to the model
    async fn call_model(
        &self,
        model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        temperature: f32,
        max_tokens: u32,
        tools: Option<&str>, // JSON string of tool definitions
    ) -> Result<(String, String), anyhow::Error>;

    // Streaming version - returns a channel of stream chunks
    async fn call_model_streaming(
        &self,
        model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        temperature: f32,
        max_tokens: u32,
        tools: Option<&str>,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>, anyhow::Error>;
}

// local cli provider for executing binaries like gemini-cli or qwen-cli
#[allow(non_camel_case_types)]
pub struct cli_provider {
    pub binary_path: String,
    pub workspace_dir: PathBuf,
    pub logger: Option<Arc<session_logger>>,
    pub is_autonomous: bool,
}

#[allow(non_camel_case_types)]
impl cli_provider {
    pub fn new(binary_path: String, workspace_dir: PathBuf) -> Self {
        Self {
            binary_path,
            workspace_dir,
            logger: None,
            is_autonomous: false,
        }
    }

    pub fn with_autonomous(mut self, autonomous: bool) -> Self {
        self.is_autonomous = autonomous;
        self
    }
}

#[async_trait]
#[allow(non_camel_case_types)]
impl model_provider for cli_provider {
    async fn call_model(
        &self,
        _model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        _temperature: f32,
        _max_tokens: u32,
        _tools: Option<&str>, // CLI binaries get tools via prompt
    ) -> Result<(String, String), anyhow::Error> {
        let full_prompt = if let Some(sys) = system_prompt {
            format!("{}\n\n{}", sys, prompt)
        } else {
            prompt.to_string()
        };

        // log the command being run
        if let Some(ref l) = self.logger {
            let _ = l.log(&format!("executing cli: {}", self.binary_path)).await;
        }

        // Parse command and determine autonomous mode flag based on AI CLI binary name
        // Each CLI has its own flag for auto-approval/yolo mode
        let final_cmd = if self.is_autonomous {
            let base_cmd = self.binary_path.trim();

            // Check if already has an autonomous flag by looking for known flag patterns
            let has_autonomous_flag = base_cmd.contains("--full-auto")
                || base_cmd.contains("-a never")
                || base_cmd.contains("--dangerously-skip-permissions")
                || base_cmd.contains("--yolo")
                || base_cmd.contains(" -y"); // space-y flag

            if has_autonomous_flag {
                // Already has a flag, use as-is
                base_cmd.to_string()
            } else {
                // Extract binary name (last component after / or space, or the whole string if no separators)
                let binary_name = base_cmd
                    .split(['/', ' ', '\\'])
                    .next_back()
                    .unwrap_or(base_cmd)
                    .to_lowercase();

                // Use exact matching against known autonomous binaries
                // This prevents false positives like "not-claude" matching
                let autonomous_flag = match binary_name.as_str() {
                    "codex" | "codex-cli" => "--full-auto",
                    "claude" | "claude-cli" | "claude-code" => "--dangerously-skip-permissions",
                    "gemini" | "gemini-cli" | "google-gemini" => "-y",
                    "qwen" | "qwen-cli" | "qwen-coder" => "--yolo",
                    "opencode" | "opencode-cli" => "--yolo",
                    // Default for unknown binaries - don't add any flag
                    // User must explicitly include the flag in the binary path
                    _ => {
                        tracing::warn!(
                            "Unknown CLI binary '{}' for autonomous mode, not adding flags.",
                            binary_name
                        );
                        return Err(anyhow::anyhow!(
                            "Cannot enable autonomous mode for this binary. Add autonomous flags manually to the binary path if needed."
                        ));
                    }
                };

                format!("{} {}", base_cmd, autonomous_flag)
            }
        } else {
            self.binary_path.clone()
        };

        #[cfg(target_os = "windows")]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&final_cmd);
            c
        };

        #[cfg(not(target_os = "windows"))]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.arg("-c").arg(&final_cmd);
            c
        };

        // the prompt is passed via stdin
        let mut child = cmd
            .current_dir(&self.workspace_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("failed to open stdin"))?;
        stdin.write_all(full_prompt.as_bytes()).await?;
        stdin.flush().await?;
        drop(stdin); // close stdin so the process knows input is finished

        // Use tokio::time::timeout to wrap the entire operation
        let output = match tokio::time::timeout(std::time::Duration::from_secs(600), child.wait_with_output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(anyhow::anyhow!("io error waiting for cli: {}", e)),
            Err(_) => return Err(anyhow::anyhow!("cli execution timed out after 10 minutes - the task may be too complex or the cli might be hanging")),
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            // Clean output: trim whitespace, preserve natural line breaks
            let cleaned = stdout.trim().to_string();

            // Full terminal output for logs
            let full_terminal = format!(
                "=== STDOUT ===\n{}\n\n=== STDERR ===\n{}",
                stdout.trim(),
                stderr.trim()
            );

            if let Some(ref l) = self.logger {
                let _ = l.log("cli execution successful.").await;
            }
            Ok((cleaned, full_terminal))
        } else {
            let error_msg = stderr.trim().to_string();
            if let Some(ref l) = self.logger {
                let _ = l.log(&format!("cli execution failed: {}", error_msg)).await;
            }
            Err(anyhow::anyhow!("cli execution failed: {}", error_msg))
        }
    }

    async fn call_model_streaming(
        &self,
        _model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        _temperature: f32,
        _max_tokens: u32,
        _tools: Option<&str>,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>, anyhow::Error> {
        let full_prompt = if let Some(sys) = system_prompt {
            format!("{}\n\n{}", sys, prompt)
        } else {
            prompt.to_string()
        };

        let binary_path = self.binary_path.clone();
        let workspace_dir = self.workspace_dir.clone();
        let is_autonomous = self.is_autonomous;

        let (tx, rx) = tokio::sync::mpsc::channel(100);

        tokio::spawn(async move {
            // Parse command and determine autonomous mode flag based on AI CLI binary name
            let final_cmd = if is_autonomous {
                let base_cmd = binary_path.trim();
                let has_autonomous_flag = base_cmd.contains("--full-auto")
                    || base_cmd.contains("-a never")
                    || base_cmd.contains("--dangerously-skip-permissions")
                    || base_cmd.contains("--yolo")
                    || base_cmd.contains(" -y");

                if has_autonomous_flag {
                    base_cmd.to_string()
                } else {
                    let binary_name = base_cmd
                        .split(['/', ' ', '\\'])
                        .next_back()
                        .unwrap_or(base_cmd)
                        .to_lowercase();

                    if binary_name.contains("codex") {
                        format!("{} --full-auto", base_cmd)
                    } else if binary_name.contains("claude") {
                        format!("{} --dangerously-skip-permissions", base_cmd)
                    } else {
                        format!("{} --yolo", base_cmd)
                    }
                }
            } else {
                binary_path.clone()
            };

            #[cfg(target_os = "windows")]
            let mut cmd = {
                let mut c = Command::new("cmd");
                c.arg("/C").arg(&final_cmd);
                c
            };

            #[cfg(not(target_os = "windows"))]
            let mut cmd = {
                let mut c = Command::new("sh");
                c.arg("-c").arg(&final_cmd);
                c
            };

            let mut child = match cmd
                .current_dir(&workspace_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx
                        .send(StreamChunk::Error(format!("spawn error: {}", e)))
                        .await;
                    return;
                }
            };

            let mut stdin = match child.stdin.take() {
                Some(s) => s,
                None => {
                    let _ = tx
                        .send(StreamChunk::Error("failed to open stdin".into()))
                        .await;
                    return;
                }
            };

            if let Err(e) = stdin.write_all(full_prompt.as_bytes()).await {
                let _ = tx
                    .send(StreamChunk::Error(format!("stdin error: {}", e)))
                    .await;
                return;
            }

            if let Err(e) = stdin.flush().await {
                let _ = tx
                    .send(StreamChunk::Error(format!("stdin flush error: {}", e)))
                    .await;
                return;
            }

            drop(stdin);

            let output = match tokio::time::timeout(
                std::time::Duration::from_secs(600),
                child.wait_with_output(),
            )
            .await
            {
                Ok(Ok(out)) => out,
                Ok(Err(e)) => {
                    let _ = tx
                        .send(StreamChunk::Error(format!("io error: {}", e)))
                        .await;
                    return;
                }
                Err(_) => {
                    let _ = tx
                        .send(StreamChunk::Error("cli execution timed out".into()))
                        .await;
                    return;
                }
            };

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();

            if output.status.success() {
                const CHUNK_SIZE: usize = 256;
                if stdout.len() <= CHUNK_SIZE {
                    let _ = tx.send(StreamChunk::TextDelta(stdout)).await;
                } else {
                    for chunk in stdout.chars().collect::<Vec<_>>().chunks(CHUNK_SIZE) {
                        let _ = tx.send(StreamChunk::TextDelta(chunk.iter().collect())).await;
                    }
                }
            }

            let _ = tx.send(StreamChunk::Done).await;
        });

        Ok(rx)
    }
}

/// OpenAI API provider with timeout
#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct openai_provider {
    client: Client,
    api_key: String,
}

#[allow(dead_code)]
impl openai_provider {
    pub fn new(api_key: String) -> Result<Self, anyhow::Error> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| anyhow::anyhow!("failed to create HTTP client: {}", e))?;
        Ok(Self { client, api_key })
    }
}

#[async_trait]
#[allow(non_camel_case_types)]
impl model_provider for openai_provider {
    async fn call_model(
        &self,
        model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        temperature: f32,
        max_tokens: u32,
        tools: Option<&str>,
    ) -> Result<(String, String), anyhow::Error> {
        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(serde_json::json!({ "role": "system", "content": sys }));
        }
        messages.push(serde_json::json!({ "role": "user", "content": prompt }));

        let mut body_map = serde_json::Map::new();
        body_map.insert("model".to_string(), serde_json::json!(model));
        body_map.insert("messages".to_string(), serde_json::json!(messages));
        body_map.insert("temperature".to_string(), serde_json::json!(temperature));
        body_map.insert("max_tokens".to_string(), serde_json::json!(max_tokens));

        // Add tools if provided
        if let Some(tools_json) = tools {
            if let Ok(tools) = serde_json::from_str::<serde_json::Value>(tools_json) {
                body_map.insert("tools".to_string(), tools);
            }
        }

        let body = serde_json::Value::Object(body_map);

        let res = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await?;

        let data: serde_json::Value = res.json().await?;

        // safe array access with bounds checking
        let choices = data["choices"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("openai response: choices is not an array"))?;

        if choices.is_empty() {
            return Err(anyhow::anyhow!("openai response: choices array is empty"));
        }

        let content = choices[0]["message"]["content"]
            .as_str()
            .ok_or_else(|| {
                anyhow::anyhow!("openai response: message content is missing or not a string")
            })?
            .to_string();

        Ok((content.clone(), content))
    }

    async fn call_model_streaming(
        &self,
        model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        temperature: f32,
        max_tokens: u32,
        tools: Option<&str>,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>, anyhow::Error> {
        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(serde_json::json!({ "role": "system", "content": sys }));
        }
        messages.push(serde_json::json!({ "role": "user", "content": prompt }));

        let mut body_map = serde_json::Map::new();
        body_map.insert("model".to_string(), serde_json::json!(model));
        body_map.insert("messages".to_string(), serde_json::json!(messages));
        body_map.insert("temperature".to_string(), serde_json::json!(temperature));
        body_map.insert("max_tokens".to_string(), serde_json::json!(max_tokens));
        body_map.insert("stream".to_string(), serde_json::json!(true));

        if let Some(tools_json) = tools {
            if let Ok(tools_val) = serde_json::from_str::<serde_json::Value>(tools_json) {
                body_map.insert("tools".to_string(), tools_val);
            }
        }

        let body = serde_json::Value::Object(body_map);

        let client = self.client.clone();
        let api_key = self.api_key.clone();

        let (tx, rx) = tokio::sync::mpsc::channel(100);

        tokio::spawn(async move {
            let res = match client
                .post("https://api.openai.com/v1/chat/completions")
                .header("Authorization", format!("Bearer {}", api_key))
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx
                        .send(StreamChunk::Error(format!("request failed: {}", e)))
                        .await;
                    return;
                }
            };

            if !res.status().is_success() {
                let body_text = res.text().await.unwrap_or_default();
                let _ = tx
                    .send(StreamChunk::Error(format!("API error: {}", body_text)))
                    .await;
                return;
            }

            // True SSE streaming - parse the stream line by line
            let mut buffer = Vec::new();

            let mut stream = res.bytes_stream();

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        for &byte in &bytes {
                            if byte == b'\n' {
                                let line = String::from_utf8_lossy(&buffer).to_string();
                                buffer.clear();

                                if line.starts_with("data:") {
                                    let data_str = line.trim_start_matches("data:").trim();
                                    if data_str == "[DONE]" {
                                        let _ = tx.send(StreamChunk::Done).await;
                                        return;
                                    }
                                    if let Ok(data) =
                                        serde_json::from_str::<serde_json::Value>(data_str)
                                    {
                                        // OpenAI chat completion chunk format
                                        if let Some(choices) = data["choices"].as_array() {
                                            for choice in choices {
                                                if let Some(delta) = choice
                                                    .get("delta")
                                                    .or_else(|| choice.get("message"))
                                                {
                                                    // Text content
                                                    if let Some(content) = delta.get("content") {
                                                        if let Some(text) = content.as_str() {
                                                            let _ = tx
                                                                .send(StreamChunk::TextDelta(
                                                                    text.to_string(),
                                                                ))
                                                                .await;
                                                        }
                                                    }
                                                    // Tool calls (OpenAI function calling format)
                                                    if let Some(tool_calls) = delta
                                                        .get("tool_calls")
                                                        .and_then(|t| t.as_array())
                                                    {
                                                        for tool_call in tool_calls {
                                                            if let Some(func) =
                                                                tool_call.get("function")
                                                            {
                                                                let name = func
                                                                    .get("name")
                                                                    .and_then(|n| n.as_str())
                                                                    .unwrap_or("")
                                                                    .to_string();
                                                                let arguments = func
                                                                    .get("arguments")
                                                                    .and_then(|a| a.as_str())
                                                                    .unwrap_or("");

                                                                if !name.is_empty()
                                                                    && !arguments.is_empty()
                                                                {
                                                                    // Parse the JSON arguments
                                                                    if let Ok(args_val) =
                                                                        serde_json::from_str::<
                                                                            serde_json::Value,
                                                                        >(
                                                                            arguments
                                                                        )
                                                                    {
                                                                        let _ = tx.send(StreamChunk::ToolUse {
                                                                            id: tool_call.get("id").and_then(|i| i.as_str()).unwrap_or("tool_call").to_string(),
                                                                            name,
                                                                            input: args_val,
                                                                        }).await;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        // Usage in delta
                                        if let Some(usage) =
                                            data.get("usage").and_then(|u| u.as_object())
                                        {
                                            let input_tokens = usage
                                                .get("prompt_tokens")
                                                .and_then(|t| t.as_u64())
                                                .unwrap_or(0);
                                            let output_tokens = usage
                                                .get("completion_tokens")
                                                .and_then(|t| t.as_u64())
                                                .unwrap_or(0);
                                            let _ = tx
                                                .send(StreamChunk::Usage {
                                                    input_tokens,
                                                    output_tokens,
                                                })
                                                .await;
                                        }
                                    }
                                }
                            } else {
                                buffer.push(byte);
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(StreamChunk::Error(format!("stream error: {}", e)))
                            .await;
                        return;
                    }
                }
            }

            let _ = tx.send(StreamChunk::Done).await;
        });

        Ok(rx)
    }
}

/// MiniMax API provider using Anthropic-compatible endpoint
/// Endpoint: /v1/messages (Anthropic format)
/// Base URL: https://api.minimax.io/anthropic
#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct minimax_provider {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
}

#[allow(dead_code)]
impl minimax_provider {
    pub fn new(api_key: String, model: String, base_url: String) -> Result<Self, anyhow::Error> {
        let mut headers = reqwest::header::HeaderMap::new();

        headers.insert(
            "Authorization",
            format!("Bearer {}", api_key).parse().map_err(
                |e: reqwest::header::InvalidHeaderValue| {
                    anyhow::anyhow!("invalid authorization header: {}", e)
                },
            )?,
        );
        headers.insert(
            "x-api-key",
            api_key
                .parse()
                .map_err(|e: reqwest::header::InvalidHeaderValue| {
                    anyhow::anyhow!("invalid api key header: {}", e)
                })?,
        );
        headers.insert(
            "anthropic-version",
            "2023-06-01"
                .parse()
                .map_err(|e: reqwest::header::InvalidHeaderValue| {
                    anyhow::anyhow!("invalid anthropic version header: {}", e)
                })?,
        );

        let client = Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| anyhow::anyhow!("failed to create HTTP client: {}", e))?;

        Ok(Self {
            client,
            api_key,
            model,
            base_url,
        })
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }
}

/// Extract text content from MiniMax response, handling thinking/tool_use blocks
/// MiniMax M2.5/M2.7 returns extended thinking - the thinking content IS the actual response
fn extract_minimax_text(data: &serde_json::Value) -> Result<String, anyhow::Error> {
    // Try to get text directly if it's a string
    if let Some(text) = data["content"].as_str() {
        return Ok(text.to_string());
    }

    // Otherwise, parse as array of blocks
    let content = data["content"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("minimax response: content is not an array or string"))?;

    // For MiniMax models, the thinking block contains the actual response
    // text blocks come after thinking and may be empty or supplementary
    // tool_use blocks are internal - skip them
    let mut thinking_content = Vec::new();
    let mut text_content = Vec::new();

    for block in content {
        let block_type = block["type"].as_str().unwrap_or("");

        match block_type {
            "thinking" => {
                // Thinking content is the main response for MiniMax
                if let Some(thinking) = block["thinking"].as_str() {
                    let trimmed = thinking.trim();
                    if !trimmed.is_empty() {
                        thinking_content.push(trimmed.to_string());
                    }
                }
            }
            "text" => {
                if let Some(text) = block["text"].as_str() {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        text_content.push(trimmed.to_string());
                    }
                }
            }
            "tool_use" | "tool_result" | "tool_use_block" => {
                // Skip tool-related blocks
            }
            _ => {
                // Try to extract any text content
                for key in &["text", "content", "thinking"] {
                    if let Some(text) = block[key].as_str() {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            text_content.push(trimmed.to_string());
                        }
                        break;
                    }
                }
            }
        }
    }

    // Prefer thinking content (MiniMax's actual response), fallback to text
    let final_text = if !thinking_content.is_empty() {
        thinking_content.join("\n")
    } else if !text_content.is_empty() {
        text_content.join("\n")
    } else {
        // Fallback: check if there's any text field at all
        if let Some(text) = data["text"].as_str() {
            text.to_string()
        } else {
            let debug_info = serde_json::to_string_pretty(data).unwrap_or_default();
            tracing::warn!(
                "minimax response: no text extracted, full response: {}",
                debug_info
            );
            return Err(anyhow::anyhow!("minimax response: no text content found"));
        }
    };

    Ok(final_text)
}

#[async_trait]
#[allow(non_camel_case_types)]
impl model_provider for minimax_provider {
    async fn call_model(
        &self,
        _model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        temperature: f32,
        max_tokens: u32,
        tools: Option<&str>,
    ) -> Result<(String, String), anyhow::Error> {
        let model = if self.model.is_empty() {
            "MiniMax-M2.5".to_string()
        } else {
            self.model.clone()
        };

        // Build messages array (Anthropic format)
        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            if !sys.is_empty() {
                // System messages should use "system" role
                messages.push(serde_json::json!({
                    "role": "system",
                    "content": sys
                }));
            }
        }
        messages.push(serde_json::json!({
            "role": "user",
            "content": prompt
        }));

        // Build request body with optional tools (Anthropic-compatible format)
        let mut body_map = serde_json::Map::new();
        body_map.insert("model".to_string(), serde_json::json!(model));
        body_map.insert("messages".to_string(), serde_json::json!(messages));
        body_map.insert("max_tokens".to_string(), serde_json::json!(max_tokens));
        body_map.insert("temperature".to_string(), serde_json::json!(temperature));

        // Add tools if provided (Anthropic-compatible format)
        if let Some(tools_json) = tools {
            if let Ok(tools_val) = serde_json::from_str::<serde_json::Value>(tools_json) {
                body_map.insert("tools".to_string(), tools_val);
            }
        }

        let body = serde_json::Value::Object(body_map);

        // Debug: log the request
        tracing::debug!(
            "minimax request to {}: {}",
            self.endpoint(),
            serde_json::to_string(&body).unwrap_or_default()
        );

        let res = self.client.post(self.endpoint()).json(&body).send().await?;

        let status = res.status();
        let body_text = res.text().await.unwrap_or_default();
        tracing::debug!("minimax response status: {}, body: {}", status, body_text);

        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "minimax API error {}: {}",
                status,
                body_text
            ));
        }

        let data: serde_json::Value = serde_json::from_str(&body_text)?;

        // Debug: log the full response
        tracing::debug!(
            "minimax response: {}",
            serde_json::to_string(&data).unwrap_or_default()
        );

        // MiniMax can return different content types: text, thinking, tool_use
        // We need to extract text content, handling thinking blocks
        let content = extract_minimax_text(&data)?;

        Ok((content.clone(), content))
    }

    async fn call_model_streaming(
        &self,
        _model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        temperature: f32,
        max_tokens: u32,
        tools: Option<&str>,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>, anyhow::Error> {
        let model = if self.model.is_empty() {
            "MiniMax-M2.5".to_string()
        } else {
            self.model.clone()
        };

        // Build messages array (Anthropic format)
        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            if !sys.is_empty() {
                // System messages should use "system" role
                messages.push(serde_json::json!({
                    "role": "system",
                    "content": sys
                }));
            }
        }
        messages.push(serde_json::json!({
            "role": "user",
            "content": prompt
        }));

        // Build request body with streaming + optional tools (Anthropic-compatible format)
        let mut body_map = serde_json::Map::new();
        body_map.insert("model".to_string(), serde_json::json!(model));
        body_map.insert("messages".to_string(), serde_json::json!(messages));
        body_map.insert("max_tokens".to_string(), serde_json::json!(max_tokens));
        body_map.insert("temperature".to_string(), serde_json::json!(temperature));
        body_map.insert("stream".to_string(), serde_json::json!(true));

        // Add tools if provided (Anthropic-compatible format)
        if let Some(tools_json) = tools {
            if let Ok(tools_val) = serde_json::from_str::<serde_json::Value>(tools_json) {
                body_map.insert("tools".to_string(), tools_val);
            }
        }

        let body = serde_json::Value::Object(body_map);

        let client = self.client.clone();
        let endpoint = self.endpoint();

        let (tx, rx) = tokio::sync::mpsc::channel(100);

        tokio::spawn(async move {
            // True SSE streaming - parse the stream as it arrives
            let res = match client.post(&endpoint).json(&body).send().await {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx
                        .send(StreamChunk::Error(format!("request failed: {}", e)))
                        .await;
                    return;
                }
            };

            if !res.status().is_success() {
                let body_text = res.text().await.unwrap_or_default();
                let _ = tx
                    .send(StreamChunk::Error(format!("API error: {}", body_text)))
                    .await;
                return;
            }

            // Collect all bytes first to check if it's SSE or batch
            let body_bytes = res.bytes().await.unwrap_or_default();
            let body_str = String::from_utf8_lossy(&body_bytes);

            // Log the full response for debugging
            tracing::info!(
                "MiniMax response length: {} bytes, first 500 chars: {:?}",
                body_str.len(),
                &body_str[..body_str.len().min(500)]
            );

            // Check if it's SSE format (starts with "event:" or "data:")
            let is_sse = body_str.starts_with("event:") || body_str.starts_with("data:");

            if is_sse {
                // Parse as SSE
                let mut buffer = Vec::new();
                let mut current_tool_id: Option<String> = None;
                let mut current_tool_name: Option<String> = None;
                let mut current_tool_input = String::new();

                for &byte in body_bytes.as_ref() {
                    if byte == b'\n' {
                        let line = String::from_utf8_lossy(&buffer).to_string();
                        buffer.clear();

                        if line.starts_with("event:") {
                            let event_type = line.trim_start_matches("event:").trim();
                            if event_type == "content_block_start"
                                || event_type == "message_delta"
                                || event_type == "content_block_delta"
                            {
                                continue;
                            }
                        } else if line.starts_with("data:") {
                            let data_str = line.trim_start_matches("data:").trim();
                            if let Ok(data) = serde_json::from_str::<serde_json::Value>(data_str) {
                                let event_type = data["type"].as_str().unwrap_or("");
                                match event_type {
                                    "content_block_start" => {
                                        if let Some(block) = data["content_block"].as_object() {
                                            if block.get("type").and_then(|v| v.as_str())
                                                == Some("tool_use")
                                            {
                                                current_tool_id = data["index"]
                                                    .as_u64()
                                                    .map(|i| format!("tool_{}", i));
                                                current_tool_name =
                                                    block["name"].as_str().map(|s| s.to_string());
                                                current_tool_input.clear();
                                            }
                                        }
                                    }
                                    "content_block_delta" => {
                                        if let Some(delta) = data["delta"].as_object() {
                                            if delta.get("type").and_then(|v| v.as_str())
                                                == Some("input_json_delta")
                                            {
                                                if let Some(partial) =
                                                    delta["partial_json"].as_str()
                                                {
                                                    current_tool_input.push_str(partial);
                                                    let _ = tx
                                                        .send(StreamChunk::ToolInputDelta(
                                                            partial.to_string(),
                                                        ))
                                                        .await;
                                                }
                                            } else if delta.get("type").and_then(|v| v.as_str())
                                                == Some("text_delta")
                                            {
                                                if let Some(text) = delta["text"].as_str() {
                                                    let _ = tx
                                                        .send(StreamChunk::TextDelta(
                                                            text.to_string(),
                                                        ))
                                                        .await;
                                                }
                                            }
                                        }
                                    }
                                    "content_block_stop" => {
                                        // Emit ToolUse when the block is complete
                                        if let (Some(id), Some(name)) =
                                            (current_tool_id.clone(), current_tool_name.clone())
                                        {
                                            let input: serde_json::Value = if current_tool_input
                                                .is_empty()
                                            {
                                                serde_json::Value::Object(serde_json::Map::new())
                                            } else {
                                                serde_json::from_str(&current_tool_input).unwrap_or(
                                                    serde_json::Value::Object(
                                                        serde_json::Map::new(),
                                                    ),
                                                )
                                            };
                                            let _ = tx
                                                .send(StreamChunk::ToolUse { id, name, input })
                                                .await;
                                        }
                                        current_tool_id = None;
                                        current_tool_name = None;
                                        current_tool_input.clear();
                                    }
                                    "message_delta" => {
                                        if let Some(delta) = data["delta"].as_object() {
                                            if delta.get("type").and_then(|v| v.as_str())
                                                == Some("text_delta")
                                            {
                                                if let Some(text) = delta["text"].as_str() {
                                                    let _ = tx
                                                        .send(StreamChunk::TextDelta(
                                                            text.to_string(),
                                                        ))
                                                        .await;
                                                }
                                            }
                                        }
                                        if let Some(usage) = data["usage"].as_object() {
                                            let input_tokens =
                                                usage["input_tokens"].as_u64().unwrap_or(0);
                                            let output_tokens =
                                                usage["output_tokens"].as_u64().unwrap_or(0);
                                            let _ = tx
                                                .send(StreamChunk::Usage {
                                                    input_tokens,
                                                    output_tokens,
                                                })
                                                .await;
                                        }
                                    }
                                    "message_stop" => {
                                        let _ = tx.send(StreamChunk::Done).await;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    } else if byte != b'\r' {
                        buffer.push(byte);
                    }
                }
            } else {
                // It's a batch JSON response - parse it directly
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&body_str) {
                    // Extract text content
                    if let Ok(content) = extract_minimax_text(&data) {
                        for c in content.chars() {
                            let _ = tx.send(StreamChunk::TextDelta(c.to_string())).await;
                        }
                    }

                    // Extract tool calls
                    if let Some(content_arr) = data["content"].as_array() {
                        for block in content_arr {
                            if block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                                let id = block["id"].as_str().unwrap_or("").to_string();
                                let name = block["name"].as_str().unwrap_or("").to_string();
                                let input = block["input"].clone();

                                let _ = tx.send(StreamChunk::ToolUse { id, name, input }).await;
                            }
                        }
                    }
                }
            }
            let _ = tx.send(StreamChunk::Done).await;
        });

        Ok(rx)
    }
}

/// OpenRouter provider (unified API for multiple models)
#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct openrouter_provider {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
}

#[allow(dead_code)]
impl openrouter_provider {
    pub fn new(api_key: String, model: String, base_url: String) -> Result<Self, anyhow::Error> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| anyhow::anyhow!("failed to create HTTP client: {}", e))?;
        Ok(Self {
            client,
            api_key,
            model,
            base_url,
        })
    }
}

#[async_trait]
#[allow(non_camel_case_types)]
impl model_provider for openrouter_provider {
    async fn call_model(
        &self,
        _model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        temperature: f32,
        max_tokens: u32,
        tools: Option<&str>,
    ) -> Result<(String, String), anyhow::Error> {
        let url = format!("{}/chat/completions", self.base_url);

        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(serde_json::json!({ "role": "system", "content": sys }));
        }
        messages.push(serde_json::json!({ "role": "user", "content": prompt }));

        let mut body_map = serde_json::Map::new();
        body_map.insert("model".to_string(), serde_json::json!(self.model));
        body_map.insert("messages".to_string(), serde_json::json!(messages));
        body_map.insert("temperature".to_string(), serde_json::json!(temperature));
        body_map.insert("max_tokens".to_string(), serde_json::json!(max_tokens));

        // Add tools if provided
        if let Some(tools_json) = tools {
            if let Ok(tools_val) = serde_json::from_str::<serde_json::Value>(tools_json) {
                body_map.insert("tools".to_string(), tools_val);
            }
        }

        let body = serde_json::Value::Object(body_map);

        let res = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://enclave.local")
            .header("X-Title", "Enclave")
            .json(&body)
            .send()
            .await?;

        let data: serde_json::Value = res.json().await?;

        // OpenRouter uses OpenAI-compatible response format
        let choices = data["choices"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("openrouter response: choices is not an array"))?;

        if choices.is_empty() {
            return Err(anyhow::anyhow!(
                "openrouter response: choices array is empty"
            ));
        }

        let content = choices[0]["message"]["content"]
            .as_str()
            .ok_or_else(|| {
                anyhow::anyhow!("openrouter response: message content is missing or not a string")
            })?
            .to_string();

        Ok((content.clone(), content))
    }

    async fn call_model_streaming(
        &self,
        _model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        temperature: f32,
        max_tokens: u32,
        tools: Option<&str>,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>, anyhow::Error> {
        let url = format!("{}/chat/completions", self.base_url);

        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(serde_json::json!({ "role": "system", "content": sys }));
        }
        messages.push(serde_json::json!({ "role": "user", "content": prompt }));

        let mut body_map = serde_json::Map::new();
        body_map.insert("model".to_string(), serde_json::json!(self.model));
        body_map.insert("messages".to_string(), serde_json::json!(messages));
        body_map.insert("temperature".to_string(), serde_json::json!(temperature));
        body_map.insert("max_tokens".to_string(), serde_json::json!(max_tokens));
        body_map.insert("stream".to_string(), serde_json::json!(true));

        if let Some(tools_json) = tools {
            if let Ok(tools_val) = serde_json::from_str::<serde_json::Value>(tools_json) {
                body_map.insert("tools".to_string(), tools_val);
            }
        }

        let body = serde_json::Value::Object(body_map);

        let client = self.client.clone();
        let api_key = self.api_key.clone();

        let (tx, rx) = tokio::sync::mpsc::channel(100);

        tokio::spawn(async move {
            let res = match client
                .post(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("HTTP-Referer", "https://enclave.local")
                .header("X-Title", "Enclave")
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx
                        .send(StreamChunk::Error(format!("request failed: {}", e)))
                        .await;
                    return;
                }
            };

            if !res.status().is_success() {
                let body_text = res.text().await.unwrap_or_default();
                let _ = tx
                    .send(StreamChunk::Error(format!("API error: {}", body_text)))
                    .await;
                return;
            }

            // True SSE streaming - parse the stream line by line
            let mut buffer = Vec::new();
            let mut stream = res.bytes_stream();

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        for &byte in &bytes {
                            if byte == b'\n' {
                                let line = String::from_utf8_lossy(&buffer).to_string();
                                buffer.clear();

                                if line.starts_with("data:") {
                                    let data_str = line.trim_start_matches("data:").trim();
                                    if data_str == "[DONE]" {
                                        let _ = tx.send(StreamChunk::Done).await;
                                        return;
                                    }
                                    if let Ok(data) =
                                        serde_json::from_str::<serde_json::Value>(data_str)
                                    {
                                        // OpenAI-compatible chunk format (OpenRouter uses this)
                                        if let Some(choices) = data["choices"].as_array() {
                                            for choice in choices {
                                                if let Some(delta) = choice
                                                    .get("delta")
                                                    .or_else(|| choice.get("message"))
                                                {
                                                    // Text content
                                                    if let Some(content) = delta.get("content") {
                                                        if let Some(text) = content.as_str() {
                                                            let _ = tx
                                                                .send(StreamChunk::TextDelta(
                                                                    text.to_string(),
                                                                ))
                                                                .await;
                                                        }
                                                    }
                                                    // Tool calls
                                                    if let Some(tool_calls) = delta
                                                        .get("tool_calls")
                                                        .and_then(|t| t.as_array())
                                                    {
                                                        for tool_call in tool_calls {
                                                            if let Some(func) =
                                                                tool_call.get("function")
                                                            {
                                                                let name = func
                                                                    .get("name")
                                                                    .and_then(|n| n.as_str())
                                                                    .unwrap_or("")
                                                                    .to_string();
                                                                let arguments = func
                                                                    .get("arguments")
                                                                    .and_then(|a| a.as_str())
                                                                    .unwrap_or("");

                                                                if !name.is_empty()
                                                                    && !arguments.is_empty()
                                                                {
                                                                    if let Ok(args_val) =
                                                                        serde_json::from_str::<
                                                                            serde_json::Value,
                                                                        >(
                                                                            arguments
                                                                        )
                                                                    {
                                                                        let _ = tx.send(StreamChunk::ToolUse {
                                                                            id: tool_call.get("id").and_then(|i| i.as_str()).unwrap_or("tool_call").to_string(),
                                                                            name,
                                                                            input: args_val,
                                                                        }).await;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                buffer.push(byte);
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(StreamChunk::Error(format!("stream error: {}", e)))
                            .await;
                        return;
                    }
                }
            }

            let _ = tx.send(StreamChunk::Done).await;
        });

        Ok(rx)
    }
}

/// Anthropic API provider with timeout
#[allow(dead_code)]
#[allow(non_camel_case_types)]
pub struct anthropic_provider {
    client: Client,
    api_key: String,
}

#[allow(dead_code)]
impl anthropic_provider {
    pub fn new(api_key: String) -> Result<Self, anyhow::Error> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| anyhow::anyhow!("failed to create HTTP client: {}", e))?;
        Ok(Self { client, api_key })
    }
}

#[async_trait]
#[allow(non_camel_case_types)]
impl model_provider for anthropic_provider {
    async fn call_model(
        &self,
        model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        temperature: f32,
        max_tokens: u32,
        tools: Option<&str>,
    ) -> Result<(String, String), anyhow::Error> {
        let mut body_map = serde_json::Map::new();
        body_map.insert("model".to_string(), serde_json::json!(model));
        body_map.insert("max_tokens".to_string(), serde_json::json!(max_tokens));
        body_map.insert("temperature".to_string(), serde_json::json!(temperature));
        body_map.insert(
            "system".to_string(),
            serde_json::json!(system_prompt.unwrap_or("")),
        );
        body_map.insert(
            "messages".to_string(),
            serde_json::json!([
                { "role": "user", "content": prompt }
            ]),
        );

        // Add tools if provided (Anthropic format)
        if let Some(tools_json) = tools {
            if let Ok(tools_val) = serde_json::from_str::<serde_json::Value>(tools_json) {
                body_map.insert("tools".to_string(), tools_val);
            }
        }

        let body = serde_json::Value::Object(body_map);

        let res = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?;

        let data: serde_json::Value = res.json().await?;

        // safe array access with bounds checking
        let content_arr = data["content"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("anthropic response: content is not an array"))?;

        if content_arr.is_empty() {
            return Err(anyhow::anyhow!(
                "anthropic response: content array is empty"
            ));
        }

        let content = content_arr[0]["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("anthropic response: text is missing or not a string"))?
            .to_string();

        Ok((content.clone(), content))
    }

    async fn call_model_streaming(
        &self,
        model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        temperature: f32,
        max_tokens: u32,
        tools: Option<&str>,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamChunk>, anyhow::Error> {
        let mut body_map = serde_json::Map::new();
        body_map.insert("model".to_string(), serde_json::json!(model));
        body_map.insert("max_tokens".to_string(), serde_json::json!(max_tokens));
        body_map.insert("temperature".to_string(), serde_json::json!(temperature));
        body_map.insert(
            "system".to_string(),
            serde_json::json!(system_prompt.unwrap_or("")),
        );
        body_map.insert(
            "messages".to_string(),
            serde_json::json!([
                { "role": "user", "content": prompt }
            ]),
        );
        body_map.insert("stream".to_string(), serde_json::json!(true));

        if let Some(tools_json) = tools {
            if let Ok(tools_val) = serde_json::from_str::<serde_json::Value>(tools_json) {
                body_map.insert("tools".to_string(), tools_val);
            }
        }

        let body = serde_json::Value::Object(body_map);

        let client = self.client.clone();
        let api_key = self.api_key.clone();

        let (tx, rx) = tokio::sync::mpsc::channel(100);

        tokio::spawn(async move {
            let res = match client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx
                        .send(StreamChunk::Error(format!("request failed: {}", e)))
                        .await;
                    return;
                }
            };

            if !res.status().is_success() {
                let body_text = res.text().await.unwrap_or_default();
                let _ = tx
                    .send(StreamChunk::Error(format!("API error: {}", body_text)))
                    .await;
                return;
            }

            // True SSE streaming - parse the stream line by line
            let mut buffer = Vec::new();
            let mut current_tool_name: Option<String> = None;
            let mut current_tool_input = String::new();
            let mut stream = res.bytes_stream();

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        for &byte in &bytes {
                            if byte == b'\n' {
                                let line = String::from_utf8_lossy(&buffer).to_string();
                                buffer.clear();

                                if line.starts_with("data:") {
                                    let data_str = line.trim_start_matches("data:").trim();
                                    if data_str == "[DONE]" {
                                        let _ = tx.send(StreamChunk::Done).await;
                                        return;
                                    }
                                    if let Ok(data) =
                                        serde_json::from_str::<serde_json::Value>(data_str)
                                    {
                                        let event_type = data["type"].as_str().unwrap_or("");
                                        match event_type {
                                            "content_block_start" => {
                                                if let Some(block) =
                                                    data["content_block"].as_object()
                                                {
                                                    if block.get("type").and_then(|v| v.as_str())
                                                        == Some("tool_use")
                                                    {
                                                        current_tool_name = block["name"]
                                                            .as_str()
                                                            .map(|s| s.to_string());
                                                        current_tool_input.clear();
                                                    }
                                                }
                                            }
                                            "content_block_delta" => {
                                                if let Some(delta) = data["delta"].as_object() {
                                                    if delta.get("type").and_then(|v| v.as_str())
                                                        == Some("input_json_delta")
                                                    {
                                                        if let Some(partial) =
                                                            delta["partial_json"].as_str()
                                                        {
                                                            current_tool_input.push_str(partial);
                                                            let _ = tx
                                                                .send(StreamChunk::ToolInputDelta(
                                                                    partial.to_string(),
                                                                ))
                                                                .await;
                                                        }
                                                    } else if delta
                                                        .get("type")
                                                        .and_then(|v| v.as_str())
                                                        == Some("text_delta")
                                                    {
                                                        if let Some(text) = delta["text"].as_str() {
                                                            let _ = tx
                                                                .send(StreamChunk::TextDelta(
                                                                    text.to_string(),
                                                                ))
                                                                .await;
                                                        }
                                                    }
                                                }
                                            }
                                            "content_block_stop" => {
                                                if let Some(name) = current_tool_name.take() {
                                                    let input: serde_json::Value =
                                                        if current_tool_input.is_empty() {
                                                            serde_json::Value::Object(
                                                                serde_json::Map::new(),
                                                            )
                                                        } else {
                                                            serde_json::from_str(
                                                                &current_tool_input,
                                                            )
                                                            .unwrap_or(serde_json::Value::Object(
                                                                serde_json::Map::new(),
                                                            ))
                                                        };
                                                    let _ = tx
                                                        .send(StreamChunk::ToolUse {
                                                            id: format!(
                                                                "tool_{}",
                                                                chrono::Utc::now()
                                                                    .timestamp_millis()
                                                            ),
                                                            name,
                                                            input,
                                                        })
                                                        .await;
                                                }
                                                current_tool_input.clear();
                                            }
                                            "message_delta" => {
                                                if let Some(delta) = data["delta"].as_object() {
                                                    if delta.get("type").and_then(|v| v.as_str())
                                                        == Some("text_delta")
                                                    {
                                                        if let Some(text) = delta["text"].as_str() {
                                                            let _ = tx
                                                                .send(StreamChunk::TextDelta(
                                                                    text.to_string(),
                                                                ))
                                                                .await;
                                                        }
                                                    }
                                                }
                                                if let Some(usage) = data["usage"].as_object() {
                                                    let input_tokens =
                                                        usage["input_tokens"].as_u64().unwrap_or(0);
                                                    let output_tokens = usage["output_tokens"]
                                                        .as_u64()
                                                        .unwrap_or(0);
                                                    let _ = tx
                                                        .send(StreamChunk::Usage {
                                                            input_tokens,
                                                            output_tokens,
                                                        })
                                                        .await;
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            } else {
                                buffer.push(byte);
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(StreamChunk::Error(format!("stream error: {}", e)))
                            .await;
                        return;
                    }
                }
            }

            let _ = tx.send(StreamChunk::Done).await;
        });

        Ok(rx)
    }
}

/// Provider factory for creating providers dynamically based on configuration
pub mod factory {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    pub enum ProviderType {
        Cli,
        OpenAI,
        Anthropic,
        MiniMax,
        OpenRouter,
    }

    impl From<&str> for ProviderType {
        fn from(s: &str) -> Self {
            match s.to_lowercase().as_str() {
                "openai" | "gpt" => ProviderType::OpenAI,
                "anthropic" | "claude" => ProviderType::Anthropic,
                "minimax" => ProviderType::MiniMax,
                "openrouter" => ProviderType::OpenRouter,
                _ => ProviderType::Cli,
            }
        }
    }

    /// Create a provider based on the binary/config string
    #[allow(clippy::too_many_arguments)]
    pub fn create_provider(
        binary_or_config: &str,
        workspace_dir: std::path::PathBuf,
        minimax_key: Option<String>,
        openai_key: Option<String>,
        anthropic_key: Option<String>,
        openrouter_key: Option<String>,
        minimax_model: Option<String>,
        minimax_base_url: Option<String>,
        openrouter_model: Option<String>,
        openrouter_base_url: Option<String>,
        is_autonomous: bool,
    ) -> Arc<dyn model_provider> {
        let provider_type = ProviderType::from(binary_or_config);

        match provider_type {
            ProviderType::Cli => Arc::new(
                cli_provider::new(binary_or_config.to_string(), workspace_dir)
                    .with_autonomous(is_autonomous),
            ),
            ProviderType::OpenAI => {
                if let Some(key) = openai_key {
                    match openai_provider::new(key) {
                        Ok(provider) => Arc::new(provider),
                        Err(e) => {
                            tracing::error!("failed to create OpenAI provider: {}", e);
                            Arc::new(cli_provider::new("gpt-cli".to_string(), workspace_dir))
                        }
                    }
                } else {
                    tracing::warn!(
                        "OpenAI provider requested but no API key provided, falling back to CLI"
                    );
                    Arc::new(cli_provider::new("gpt-cli".to_string(), workspace_dir))
                }
            }
            ProviderType::Anthropic => {
                if let Some(key) = anthropic_key {
                    match anthropic_provider::new(key) {
                        Ok(p) => Arc::new(p),
                        Err(e) => {
                            tracing::warn!(
                                "Failed to create Anthropic provider: {}, falling back to CLI",
                                e
                            );
                            Arc::new(cli_provider::new("claude".to_string(), workspace_dir))
                        }
                    }
                } else {
                    tracing::warn!(
                        "Anthropic provider requested but no API key provided, falling back to CLI"
                    );
                    Arc::new(cli_provider::new("claude".to_string(), workspace_dir))
                }
            }
            ProviderType::MiniMax => {
                if let Some(key) = minimax_key {
                    let model = minimax_model.unwrap_or_else(|| "MiniMax-M2.5".to_string());
                    let base_url = minimax_base_url
                        .unwrap_or_else(|| "https://api.minimax.io/anthropic".to_string());
                    match minimax_provider::new(key, model, base_url) {
                        Ok(provider) => Arc::new(provider),
                        Err(e) => {
                            tracing::error!("failed to create MiniMax provider: {}", e);
                            Arc::new(cli_provider::new("minimax-cli".to_string(), workspace_dir))
                        }
                    }
                } else {
                    tracing::warn!(
                        "MiniMax provider requested but no API key provided, falling back to CLI"
                    );
                    Arc::new(cli_provider::new("minimax-cli".to_string(), workspace_dir))
                }
            }
            ProviderType::OpenRouter => {
                if let Some(key) = openrouter_key {
                    let model = openrouter_model
                        .unwrap_or_else(|| "anthropic/claude-3.5-sonnet".to_string());
                    let base_url = openrouter_base_url
                        .unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());
                    match openrouter_provider::new(key, model, base_url) {
                        Ok(provider) => Arc::new(provider),
                        Err(e) => {
                            tracing::error!("failed to create OpenRouter provider: {}", e);
                            Arc::new(cli_provider::new(
                                "openrouter-cli".to_string(),
                                workspace_dir,
                            ))
                        }
                    }
                } else {
                    tracing::warn!("OpenRouter provider requested but no API key provided, falling back to CLI");
                    Arc::new(cli_provider::new(
                        "openrouter-cli".to_string(),
                        workspace_dir,
                    ))
                }
            }
        }
    }
}
