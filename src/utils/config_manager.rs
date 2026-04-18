use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[allow(non_camel_case_types)]
pub struct EnclaveConfig {
    pub minimax_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub openrouter_api_key: Option<String>,
    #[serde(default = "default_provider")]
    pub default_provider: String,
    #[serde(default = "default_autonomous")]
    pub autonomous_mode: bool,
    #[serde(default = "default_max_rounds")]
    pub max_rounds: usize,
}

fn default_provider() -> String {
    "minimax".to_string()
}

fn default_autonomous() -> bool {
    true
}

fn default_max_rounds() -> usize {
    7
}

#[derive(Serialize, Deserialize)]
#[allow(non_camel_case_types)]
pub struct MaskedConfig {
    pub has_minimax_key: bool,
    pub has_openai_key: bool,
    pub has_anthropic_key: bool,
    pub has_openrouter_key: bool,
    pub default_provider: String,
    pub autonomous_mode: bool,
    pub max_rounds: usize,
}

#[derive(Serialize, Deserialize)]
#[allow(non_camel_case_types)]
pub struct ConfigUpdate {
    pub minimax_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub openrouter_api_key: Option<String>,
    pub default_provider: Option<String>,
    pub autonomous_mode: Option<bool>,
    pub max_rounds: Option<usize>,
}

#[derive(Serialize)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<String>,
}

pub struct ConfigManager {
    config_path: PathBuf,
}

impl ConfigManager {
    pub fn new(workspace_dir: &Path) -> Self {
        Self {
            config_path: workspace_dir.join(".enclave_config.json"),
        }
    }

    pub fn load(&self) -> Result<EnclaveConfig, String> {
        if !self.config_path.exists() {
            return Ok(EnclaveConfig {
                minimax_api_key: None,
                openai_api_key: None,
                anthropic_api_key: None,
                openrouter_api_key: None,
                default_provider: default_provider(),
                autonomous_mode: default_autonomous(),
                max_rounds: default_max_rounds(),
            });
        }

        let content = std::fs::read_to_string(&self.config_path)
            .map_err(|e| format!("Failed to read config file: {}", e))?;

        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse config file: {}", e))
    }

    pub fn save(&self, config: &EnclaveConfig) -> Result<(), String> {
        let content = serde_json::to_string_pretty(config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        std::fs::write(&self.config_path, content)
            .map_err(|e| format!("Failed to write config file: {}", e))?;

        Ok(())
    }

    pub fn mask(&self, config: &EnclaveConfig) -> MaskedConfig {
        MaskedConfig {
            has_minimax_key: config.minimax_api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false),
            has_openai_key: config.openai_api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false),
            has_anthropic_key: config.anthropic_api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false),
            has_openrouter_key: config.openrouter_api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false),
            default_provider: config.default_provider.clone(),
            autonomous_mode: config.autonomous_mode,
            max_rounds: config.max_rounds,
        }
    }

    pub fn validate(config: &ConfigUpdate) -> ValidationResult {
        let mut errors = Vec::new();

        if let Some(ref key) = config.minimax_api_key {
            if !key.is_empty() && !Self::is_valid_api_key_format(key) {
                errors.push("minimax_api_key has invalid format".to_string());
            }
        }

        if let Some(ref key) = config.openai_api_key {
            if !key.is_empty() && !Self::is_valid_api_key_format(key) {
                errors.push("openai_api_key has invalid format".to_string());
            }
        }

        if let Some(ref key) = config.anthropic_api_key {
            if !key.is_empty() && !Self::is_valid_api_key_format(key) {
                errors.push("anthropic_api_key has invalid format".to_string());
            }
        }

        if let Some(ref key) = config.openrouter_api_key {
            if !key.is_empty() && !Self::is_valid_api_key_format(key) {
                errors.push("openrouter_api_key has invalid format".to_string());
            }
        }

        if let Some(ref provider) = config.default_provider {
            let valid_providers = ["minimax", "openai", "anthropic", "openrouter"];
            if !valid_providers.contains(&provider.as_str()) {
                errors.push(format!("default_provider must be one of: {}", valid_providers.join(", ")));
            }
        }

        if let Some(rounds) = config.max_rounds {
            if !(1..=50).contains(&rounds) {
                errors.push("max_rounds must be between 1 and 50".to_string());
            }
        }

        ValidationResult {
            valid: errors.is_empty(),
            errors,
        }
    }

    fn is_valid_api_key_format(key: &str) -> bool {
        if key.len() < 10 {
            return false;
        }
        key.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_config() {
        let config = EnclaveConfig {
            minimax_api_key: Some("test-key-123".to_string()),
            openai_api_key: Some("sk-abc123".to_string()),
            anthropic_api_key: None,
            openrouter_api_key: Some("sk-or-key".to_string()),
            default_provider: "minimax".to_string(),
            autonomous_mode: true,
            max_rounds: 7,
        };

        let manager = ConfigManager::new(&PathBuf::from("."));
        let masked = manager.mask(&config);

        assert!(masked.has_minimax_key);
        assert!(masked.has_openai_key);
        assert!(!masked.has_anthropic_key);
        assert!(masked.has_openrouter_key);
        assert_eq!(masked.default_provider, "minimax");
    }

    #[test]
    fn test_validate_valid_config() {
        let config = ConfigUpdate {
            minimax_api_key: Some("valid-key-123".to_string()),
            openai_api_key: None,
            anthropic_api_key: None,
            openrouter_api_key: None,
            default_provider: Some("minimax".to_string()),
            autonomous_mode: Some(true),
            max_rounds: Some(7),
        };

        let result = ConfigManager::validate(&config);
        assert!(result.valid);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_validate_invalid_provider() {
        let config = ConfigUpdate {
            minimax_api_key: None,
            openai_api_key: None,
            anthropic_api_key: None,
            openrouter_api_key: None,
            default_provider: Some("invalid".to_string()),
            autonomous_mode: None,
            max_rounds: None,
        };

        let result = ConfigManager::validate(&config);
        assert!(!result.valid);
        assert!(!result.errors.is_empty());
    }
}
