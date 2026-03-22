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

        // Parse command and determine autonomous mode flag based on AI CLI
        // Each CLI has its own flag for auto-approval/yolo mode:
        // - qwen: -y or --yolo
        // - gemini: --yolo or -y
        // - codex: --full-auto or -a never
        // - claude: --dangerously-skip-permissions
        // - opencode: --yolo or --dangerously-skip-permissions
        let final_cmd = if self.is_autonomous {
            let base_cmd = self.binary_path.trim();
            let has_autonomous_flag =
                base_cmd.contains("--full-auto") ||
                base_cmd.contains("-a never") ||
                base_cmd.contains("--dangerously-skip-permissions") ||
                base_cmd.contains("--yolo") ||
                base_cmd.contains(" -y") || // space-y to avoid matching "qy" or similar
                base_cmd.ends_with(" -y"); // also catch trailing -y

            if has_autonomous_flag {
                base_cmd.to_string()
            } else if base_cmd.contains("codex") {
                format!("{} --full-auto", base_cmd)
            } else if base_cmd.contains("claude") {
                format!("{} --dangerously-skip-permissions", base_cmd)
            } else {
                // qwen, gemini, opencode and others use -y/--yolo
                format!("{} --yolo", base_cmd)
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
            // simple heuristic to clean output (removing common CLI debug noise)
            // preserves natural line breaks and formatting
            let cleaned = if stdout.contains("```") {
                stdout.trim().to_string()
            } else {
                stdout.split("\n\n")
                    .max_by_key(|s| s.len())
                    .unwrap_or(&stdout)
                    .trim()
                    .to_string()
            };

            // full terminal output for logs (filter out massive prompts only)
            let full_terminal = format!("{}\n\n[... CLI OUTPUT ...]\n\n{}", stdout, stderr);

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
