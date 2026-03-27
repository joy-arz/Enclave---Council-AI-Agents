use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize, Clone, Debug)]
#[allow(non_camel_case_types)]
pub struct config {
    #[allow(dead_code)]
    #[serde(default = "default_debug")]
    pub debug: bool,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_host")]
    pub host: String,

    // workspace settings
    #[serde(default = "default_workspace")]
    pub workspace_dir: PathBuf,

    // API keys for direct LLM providers
    #[allow(dead_code)]
    pub minimax_api_key: Option<String>,
    #[allow(dead_code)]
    pub openai_api_key: Option<String>,
    #[allow(dead_code)]
    pub anthropic_api_key: Option<String>,
    #[allow(dead_code)]
    pub openrouter_api_key: Option<String>,

    // MiniMax specific settings
    #[serde(default = "default_minimax_model")]
    pub minimax_model: String,
    #[serde(default = "default_minimax_base_url")]
    pub minimax_base_url: String,

    // OpenRouter specific settings (kept for future use)
    #[allow(dead_code)]
    #[serde(default = "default_openrouter_model")]
    pub openrouter_model: String,
    #[allow(dead_code)]
    #[serde(default = "default_openrouter_base_url")]
    pub openrouter_base_url: String,

    // cli binary mapping
    #[serde(default = "default_gemini_binary")]
    pub strategist_binary: String,
    #[serde(default = "default_qwen_binary")]
    pub critic_binary: String,
    #[serde(default = "default_gemini_binary")]
    pub optimizer_binary: String,
    #[serde(default = "default_qwen_binary")]
    pub contrarian_binary: String,
    #[serde(default = "default_gemini_binary")]
    pub judge_binary: String,

    // autonomous mode configuration
    #[serde(default = "default_true")]
    pub autonomous_mode: bool,

    // session defaults
    #[serde(default = "default_max_rounds")]
    pub max_rounds: usize,
    #[serde(default = "default_max_tokens")]
    pub max_tokens_per_agent: u32,
    #[serde(default = "default_temperature")]
    pub default_temperature: f32,
}

fn default_debug() -> bool { true }
fn default_port() -> u16 { 8000 }
fn default_host() -> String { "127.0.0.1".to_string() }
fn default_max_rounds() -> usize { 7 }
fn default_max_tokens() -> u32 { 500 }
fn default_temperature() -> f32 { 0.7 }
fn default_true() -> bool { true }
fn default_gemini_binary() -> String { "gemini-cli".to_string() }
fn default_qwen_binary() -> String { "qwen-cli".to_string() }
fn default_workspace() -> PathBuf { std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")) }
fn default_minimax_model() -> String { "MiniMax-Text-01".to_string() }
fn default_minimax_base_url() -> String { "https://api.minimax.io/anthropic".to_string() }
fn default_openrouter_model() -> String { "anthropic/claude-3.5-sonnet".to_string() }
fn default_openrouter_base_url() -> String { "https://openrouter.ai/api/v1".to_string() }

#[allow(non_camel_case_types)]
impl config {
    pub fn from_env() -> Result<Self, envy::Error> {
        dotenvy::dotenv().ok();
        envy::from_env::<config>()
    }
}
