use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;
use std::path::PathBuf;
use chrono::Local;
use serde::Serialize;
use regex::Regex;
use once_cell::sync::Lazy;

/// Patterns to redact from logs for security
static API_KEY_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(api[_-]?key|apikey|bearer|token|secret)["\s:=]+["']?[a-zA-Z0-9_-]{20,}["']?"#).unwrap()
});
static PASSWORD_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(password|passwd|pwd)["\s:=]+["']?[^\s"']{4,}["']?"#).unwrap()
});
static PRIVATE_KEY_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"-----BEGIN (RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----").unwrap()
});

/// Redact sensitive information from log messages
fn redact_sensitive_data(message: &str) -> String {
    let mut redacted = message.to_string();
    
    // Redact API keys and tokens
    redacted = API_KEY_PATTERN.replace_all(&redacted, "$1=[REDACTED]").to_string();
    
    // Redact passwords
    redacted = PASSWORD_PATTERN.replace_all(&redacted, "$1=[REDACTED]").to_string();
    
    // Redact private keys (replace entire block marker)
    if PRIVATE_KEY_PATTERN.is_match(&redacted) {
        redacted = redacted.replace("-----BEGIN PRIVATE KEY-----", "[PRIVATE KEY REDACTED]");
        redacted = redacted.replace("-----BEGIN RSA PRIVATE KEY-----", "[PRIVATE KEY REDACTED]");
        redacted = redacted.replace("-----BEGIN EC PRIVATE KEY-----", "[PRIVATE KEY REDACTED]");
        redacted = redacted.replace("-----BEGIN OPENSSH PRIVATE KEY-----", "[PRIVATE KEY REDACTED]");
    }
    
    redacted
}

/// JSONL event types for structured logging
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "data")]
#[allow(non_camel_case_types)]
pub enum LogEvent {
    session_start { timestamp: String, query: String },
    session_end { timestamp: String },
    round_start { round: usize },
    #[allow(dead_code)]
    round_end { round: usize },
    agent_message { timestamp: String, agent: String, round: usize, content: String },
    judge_decision { timestamp: String, decision: String, round: usize },
    max_rounds_reached { max_rounds: usize },
    #[allow(dead_code)]
    error { timestamp: String, error: String },
    #[allow(dead_code)]
    info { timestamp: String, message: String },
    // Context management events
    context_warning { message: String },
    context_compaction { phase: String, message: String, messages_summarized: usize },
    busy_state_changed { state: String },
}

#[allow(non_camel_case_types)]
pub struct session_logger {
    pub log_path: PathBuf,
    pub jsonl_path: PathBuf,
}

#[allow(non_camel_case_types)]
impl session_logger {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self {
            log_path: workspace_dir.join("last_session_log.md"),
            jsonl_path: workspace_dir.join("last_session_log.jsonl"),
        }
    }

    pub async fn clear(&self) -> tokio::io::Result<()> {
        File::create(&self.log_path).await?;
        File::create(&self.jsonl_path).await?;
        self.log_markdown("# enclave session log\n").await
    }

    /// Log in JSONL format for machine-readable output
    pub async fn log_event(&self, event: LogEvent) -> tokio::io::Result<()> {
        if let Some(parent) = self.jsonl_path.parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        let json = serde_json::to_string(&event).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e)
        })?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.jsonl_path)
            .await?;

        file.write_all(json.as_bytes()).await?;
        file.write_all(b"\n").await?;
        let _ = file.flush().await;

        Ok(())
    }

    /// Log a message in markdown format for human readability
    pub async fn log_markdown(&self, message: &str) -> tokio::io::Result<()> {
        if let Some(parent) = self.log_path.parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .await?;

        let entry = if message.starts_with("#") || message.starts_with("---") {
            format!("{}\n", message)
        } else {
            format!("[{}] {}\n", timestamp, message)
        };

        file.write_all(entry.as_bytes()).await?;
        file.sync_all().await.map_err(std::io::Error::other)?;

        Ok(())
    }

    /// Convenience method that logs both to markdown and stdout
    pub async fn log(&self, message: &str) -> tokio::io::Result<()> {
        self.log_markdown(message).await?;
        println!("[session log] {}", message);
        Ok(())
    }

    /// Log session start event
    pub async fn log_session_start(&self, query: &str) -> tokio::io::Result<()> {
        let timestamp = Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
        self.log_event(LogEvent::session_start {
            timestamp: timestamp.clone(),
            query: query.to_string(),
        }).await?;
        self.log_markdown(&format!("\n# enclave session log\n\n## Session Started\nQuery: {}\n", query)).await
    }

    /// Log session end event
    pub async fn log_session_end(&self) -> tokio::io::Result<()> {
        let timestamp = Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
        self.log_event(LogEvent::session_end {
            timestamp: timestamp.clone(),
        }).await?;
        self.log_markdown("\n## Session Ended\n").await
    }

    /// Log round start
    pub async fn log_round_start(&self, round: usize) -> tokio::io::Result<()> {
        self.log_event(LogEvent::round_start { round }).await?;
        self.log(&format!("--- round {} ---", round)).await
    }

    /// Log agent message (for web UI display)
    pub async fn log_agent_message(&self, agent: &str, round: usize, content: &str) -> tokio::io::Result<()> {
        let timestamp = Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
        self.log_event(LogEvent::agent_message {
            timestamp,
            agent: agent.to_string(),
            round,
            content: content.to_string(),
        }).await?;
        // Also log to markdown for human readability
        self.log_markdown(&format!("\n### {} (round {})\n\n{}\n", agent, round, content)).await
    }

    /// Log judge decision
    pub async fn log_judge_decision(&self, decision: &str, round: usize) -> tokio::io::Result<()> {
        let timestamp = Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
        self.log_event(LogEvent::judge_decision {
            timestamp,
            decision: decision.to_string(),
            round,
        }).await?;
        self.log(&format!("judge decision: {} (round {}/{})", decision, round, self.max_rounds_for_log())).await
    }

    /// Helper to get max_rounds from orchestrator context (approximate)
    fn max_rounds_for_log(&self) -> usize {
        7 // Default, will be set via log_judge_decision if needed
    }

    /// Log context warning (e.g., approaching token limit)
    pub async fn log_context_warning(&self, message: &str) -> tokio::io::Result<()> {
        self.log_event(LogEvent::context_warning {
            message: message.to_string(),
        }).await?;
        self.log(&format!("[context warning] {}", message)).await
    }

    /// Log context compaction (summarization)
    pub async fn log_context_compaction(&self, phase: &str, message: &str, messages_summarized: usize) -> tokio::io::Result<()> {
        self.log_event(LogEvent::context_compaction {
            phase: phase.to_string(),
            message: message.to_string(),
            messages_summarized,
        }).await?;
        self.log(&format!("[context compaction] {}: {} ({} messages summarized)", phase, message, messages_summarized)).await
    }

    /// Log busy state change
    pub async fn log_busy_state(&self, state: &str) -> tokio::io::Result<()> {
        self.log_event(LogEvent::busy_state_changed {
            state: state.to_string(),
        }).await
    }
}
