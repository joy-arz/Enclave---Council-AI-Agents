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

        // Parse command and determine how to pass the prompt
        // Some CLIs like qwen expect `-p "prompt"` format, others expect stdin
        // We'll pass prompt via stdin for now (most common)

        let final_cmd = if self.is_autonomous
            && !self.binary_path.contains("--no-confirmation")
            && self.binary_path.contains("qwen")
        {
            format!("{} --no-confirmation", self.binary_path.trim())
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
        let mut full_terminal = format!("{}{}", stdout, stderr);

        // Filter out the massive prompts to prevent UI clutter
        full_terminal = full_terminal.replace(&full_prompt, "\n[... FULL PROMPT REDACTED FOR CLARITY ...]\n");
        full_terminal = full_terminal.replace(prompt, "\n[... HISTORY REDACTED FOR CLARITY ...]\n");

        if output.status.success() {
            // clean output - extract actual response, skipping debug noise
            // the actual response typically comes after all the debug/thinking blocks
            let lines: Vec<&str> = stdout.lines().collect();
            let debug_markers = [
                "Debug mode enabled",
                "Logging to:",
                "Warning:",
                "Tool \"write_file\" requires user approval",
                "To enable automatic tool execution",
                "Example:",
                "<thinking>",
                "</thinking>",
            ];

            // find the last non-debug line that has substantial content
            let mut cleaned_lines: Vec<&str> = Vec::new();
            let mut found_real_content = false;

            for line in lines.iter().rev() {
                let trimmed = line.trim();

                // skip empty lines at the end
                if trimmed.is_empty() && !found_real_content {
                    continue;
                }

                // skip debug markers
                let is_debug = debug_markers.iter().any(|m| trimmed.starts_with(m));
                if is_debug {
                    continue;
                }

                // skip lines that are just code fences
                if trimmed == "```" || trimmed == "```json" || trimmed == "```rust" || trimmed == "```" {
                    continue;
                }

                found_real_content = true;
                cleaned_lines.push(line);
            }

            // reverse back to correct order and join
            cleaned_lines.reverse();
            let cleaned = cleaned_lines.join("\n").trim().to_string();

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

#[allow(non_camel_case_types, dead_code)]
pub struct openai_provider {
    client: Client,
    api_key: String,
}

#[allow(non_camel_case_types, dead_code)]
impl openai_provider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
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
        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("failed to parse openai response"))?
            .to_string();
        
        Ok((content.clone(), content))
    }
}

#[allow(non_camel_case_types, dead_code)]
pub struct anthropic_provider {
    client: Client,
    api_key: String,
}

#[allow(non_camel_case_types, dead_code)]
impl anthropic_provider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
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
        let content = data["content"][0]["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("failed to parse anthropic response"))?
            .to_string();
        
        Ok((content.clone(), content))
    }
}
