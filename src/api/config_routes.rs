use axum::{
    extract::State,
    Json,
};

use crate::utils::config_manager::{ConfigUpdate, MaskedConfig, ValidationResult};
use crate::api::AppState;

pub async fn get_config(
    State(state): State<AppState>,
) -> Json<MaskedConfig> {
    match state.config_manager.load() {
        Ok(cfg) => Json(state.config_manager.mask(&cfg)),
        Err(_) => {
            Json(MaskedConfig {
                has_minimax_key: false,
                has_openai_key: false,
                has_anthropic_key: false,
                has_openrouter_key: false,
                default_provider: "minimax".to_string(),
                autonomous_mode: true,
                max_rounds: 7,
            })
        }
    }
}

pub async fn update_config(
    State(state): State<AppState>,
    Json(params): Json<ConfigUpdate>,
) -> Json<serde_json::Value> {
    let validation = crate::utils::config_manager::ConfigManager::validate(&params);
    if !validation.valid {
        return Json(serde_json::json!({
            "status": "error",
            "message": "Validation failed",
            "errors": validation.errors
        }));
    }

    let mut current = match state.config_manager.load() {
        Ok(cfg) => cfg,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to load current config: {}", e)
            }));
        }
    };

    if let Some(key) = params.minimax_api_key {
        current.minimax_api_key = Some(key).filter(|k| !k.is_empty());
    }
    if let Some(key) = params.openai_api_key {
        current.openai_api_key = Some(key).filter(|k| !k.is_empty());
    }
    if let Some(key) = params.anthropic_api_key {
        current.anthropic_api_key = Some(key).filter(|k| !k.is_empty());
    }
    if let Some(key) = params.openrouter_api_key {
        current.openrouter_api_key = Some(key).filter(|k| !k.is_empty());
    }
    if let Some(provider) = params.default_provider {
        current.default_provider = provider;
    }
    if let Some(autonomous) = params.autonomous_mode {
        current.autonomous_mode = autonomous;
    }
    if let Some(rounds) = params.max_rounds {
        current.max_rounds = rounds;
    }

    match state.config_manager.save(&current) {
        Ok(_) => Json(serde_json::json!({
            "status": "success",
            "config": state.config_manager.mask(&current)
        })),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "message": format!("Failed to save config: {}", e)
        })),
    }
}

pub async fn validate_config(
    Json(params): Json<ConfigUpdate>,
) -> Json<ValidationResult> {
    Json(crate::utils::config_manager::ConfigManager::validate(&params))
}
