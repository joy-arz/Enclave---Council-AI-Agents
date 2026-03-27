use async_trait::async_trait;
use reqwest::Client;
use tokio::process::Command;
use std::process::Stdio;
use std::path::PathBuf;
use crate::utils::logger_mod::session_logger;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

#[async_trait]
#[allow(non_camel_case_types)]
pub trait model_provider: Send + Sync {
    // returns (cleaned_content, full_terminal_output)
    async fn call_model(
        &self,
        model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        temperature: f32,
        max_tokens: u32,
    ) -> Result<(String, String), anyhow::Error>;
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
        Self { binary_path, workspace_dir, logger: None, is_autonomous: false }
    }

    pub fn with_logger(mut self, logger: Arc<session_logger>) -> Self {
        self.logger = Some(logger);
        self
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
            let has_autonomous_flag =
                base_cmd.contains("--full-auto") ||
                base_cmd.contains("-a never") ||
                base_cmd.contains("--dangerously-skip-permissions") ||
                base_cmd.contains("--yolo") ||
                base_cmd.contains(" -y"); // space-y flag

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

                    // Append appropriate autonomous flag based on binary name
                    if binary_name.contains("codex") {
                        format!("{} --full-auto", base_cmd)
                    } else if binary_name.contains("claude") {
                        format!("{} --dangerously-skip-permissions", base_cmd)
                    } else {
                        // qwen, gemini, opencode, and others default to --yolo
                        format!("{} --yolo", base_cmd)
                    }
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

        let mut stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("failed to open stdin"))?;
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
            let full_terminal = format!("=== STDOUT ===\n{}\n\n=== STDERR ===\n{}", stdout.trim(), stderr.trim());

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
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("failed to create HTTP client"),
            api_key,
        }
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
    ) -> Result<(String, String), anyhow::Error> {
        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(serde_json::json!({ "role": "system", "content": sys }));
        }
        messages.push(serde_json::json!({ "role": "user", "content": prompt }));

        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "temperature": temperature,
            "max_tokens": max_tokens
        });

        let res = self.client.post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await?;

        let data: serde_json::Value = res.json().await?;

        // safe array access with bounds checking
        let choices = data["choices"].as_array()
            .ok_or_else(|| anyhow::anyhow!("openai response: choices is not an array"))?;

        if choices.is_empty() {
            return Err(anyhow::anyhow!("openai response: choices array is empty"));
        }

        let content = choices[0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("openai response: message content is missing or not a string"))?
            .to_string();

        Ok((content.clone(), content))
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
    pub fn new(api_key: String, model: String, base_url: String) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "Authorization",
            format!("Bearer {}", api_key).parse().unwrap(),
        );
        headers.insert(
            "x-api-key",
            api_key.parse().unwrap(),
        );
        headers.insert(
            "anthropic-version",
            "2023-06-01".parse().unwrap(),
        );

        Self {
            client: Client::builder()
                .default_headers(headers)
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("failed to create HTTP client"),
            api_key,
            model,
            base_url,
        }
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
            tracing::warn!("minimax response: no text extracted, full response: {}", debug_info);
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
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": sys
                }));
            }
        }
        messages.push(serde_json::json!({
            "role": "user",
            "content": prompt
        }));

        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": temperature
        });

        // Debug: log the request
        tracing::debug!("minimax request to {}: {}", self.endpoint(), serde_json::to_string(&body).unwrap_or_default());

        let res = self.client
            .post(self.endpoint())
            .json(&body)
            .send()
            .await?;

        let status = res.status();
        let body_text = res.text().await.unwrap_or_default();
        tracing::debug!("minimax response status: {}, body: {}", status, body_text);

        if !status.is_success() {
            return Err(anyhow::anyhow!("minimax API error {}: {}", status, body_text));
        }

        let data: serde_json::Value = serde_json::from_str(&body_text)?;

        // Debug: log the full response
        tracing::debug!("minimax response: {}", serde_json::to_string(&data).unwrap_or_default());

        // MiniMax can return different content types: text, thinking, tool_use
        // We need to extract text content, handling thinking blocks
        let content = extract_minimax_text(&data)?;

        Ok((content.clone(), content))
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
    pub fn new(api_key: String, model: String, base_url: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("failed to create HTTP client"),
            api_key,
            model,
            base_url,
        }
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
    ) -> Result<(String, String), anyhow::Error> {
        let url = format!("{}/chat/completions", self.base_url);

        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(serde_json::json!({ "role": "system", "content": sys }));
        }
        messages.push(serde_json::json!({ "role": "user", "content": prompt }));

        let body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "temperature": temperature,
            "max_tokens": max_tokens
        });

        let res = self.client.post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://enclave.local")
            .header("X-Title", "Enclave")
            .json(&body)
            .send()
            .await?;

        let data: serde_json::Value = res.json().await?;

        // OpenRouter uses OpenAI-compatible response format
        let choices = data["choices"].as_array()
            .ok_or_else(|| anyhow::anyhow!("openrouter response: choices is not an array"))?;

        if choices.is_empty() {
            return Err(anyhow::anyhow!("openrouter response: choices array is empty"));
        }

        let content = choices[0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("openrouter response: message content is missing or not a string"))?
            .to_string();

        Ok((content.clone(), content))
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
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("failed to create HTTP client"),
            api_key,
        }
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
    ) -> Result<(String, String), anyhow::Error> {
        let body = serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "temperature": temperature,
            "system": system_prompt.unwrap_or(""),
            "messages": [
                { "role": "user", "content": prompt }
            ]
        });

        let res = self.client.post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?;

        let data: serde_json::Value = res.json().await?;

        // safe array access with bounds checking
        let content_arr = data["content"].as_array()
            .ok_or_else(|| anyhow::anyhow!("anthropic response: content is not an array"))?;

        if content_arr.is_empty() {
            return Err(anyhow::anyhow!("anthropic response: content array is empty"));
        }

        let content = content_arr[0]["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("anthropic response: text is missing or not a string"))?
            .to_string();

        Ok((content.clone(), content))
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
    pub fn create_provider(
        binary_or_config: &str,
        workspace_dir: std::path::PathBuf,
        api_key: Option<String>,
        model: Option<String>,
        base_url: Option<String>,
        is_autonomous: bool,
    ) -> Arc<dyn model_provider> {
        let provider_type = ProviderType::from(binary_or_config);

        match provider_type {
            ProviderType::Cli => {
                Arc::new(cli_provider::new(binary_or_config.to_string(), workspace_dir)
                    .with_autonomous(is_autonomous))
            }
            ProviderType::OpenAI => {
                if let Some(key) = api_key {
                    Arc::new(openai_provider::new(key))
                } else {
                    tracing::warn!("OpenAI provider requested but no API key provided, falling back to CLI");
                    Arc::new(cli_provider::new("gpt-cli".to_string(), workspace_dir))
                }
            }
            ProviderType::Anthropic => {
                if let Some(key) = api_key {
                    Arc::new(anthropic_provider::new(key))
                } else {
                    tracing::warn!("Anthropic provider requested but no API key provided, falling back to CLI");
                    Arc::new(cli_provider::new("claude".to_string(), workspace_dir))
                }
            }
            ProviderType::MiniMax => {
                if let Some(key) = api_key {
                    let model = model.unwrap_or_else(|| "MiniMax-M2.5".to_string());
                    let base_url = base_url.unwrap_or_else(|| "https://api.minimax.io/anthropic".to_string());
                    Arc::new(minimax_provider::new(key, model, base_url))
                } else {
                    tracing::warn!("MiniMax provider requested but no API key provided, falling back to CLI");
                    Arc::new(cli_provider::new("minimax-cli".to_string(), workspace_dir))
                }
            }
            ProviderType::OpenRouter => {
                if let Some(key) = api_key {
                    let model = model.unwrap_or_else(|| "anthropic/claude-3.5-sonnet".to_string());
                    let base_url = base_url.unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());
                    Arc::new(openrouter_provider::new(key, model, base_url))
                } else {
                    tracing::warn!("OpenRouter provider requested but no API key provided, falling back to CLI");
                    Arc::new(cli_provider::new("openrouter-cli".to_string(), workspace_dir))
                }
            }
        }
    }
}
