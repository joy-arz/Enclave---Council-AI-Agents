use axum::{
    extract::{Query, State, Path},
    response::{Sse, Json},
};
use axum::response::sse::{Event, KeepAlive};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::mpsc;
use futures::stream::{self, Stream};
use std::convert::Infallible;
use uuid::Uuid;
use std::process::Command;

use crate::core::{orchestrator, agent_response};

use crate::utils::config_mod::config;
use crate::agents::{roles, judge::judge_agent};
use crate::core::providers_mod::{cli_provider, model_provider};
use crate::api::sessions_mod::session_store;
use crate::utils::logger_mod::session_logger;

#[derive(Deserialize)]
#[allow(non_camel_case_types)]
pub struct enclave_params {
    pub query: String,
    pub rounds: Option<usize>,
    pub session_id: Option<String>,
    pub autonomous: Option<bool>,
    pub workspace_dir: Option<String>,
    // Binary overrides
    pub strategist_binary: Option<String>,
    pub critic_binary: Option<String>,
    pub optimizer_binary: Option<String>,
    pub maintainer_binary: Option<String>,
    pub judge_binary: Option<String>,
}

pub async fn browse_workspace() -> Json<Option<String>> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("osascript")
            .arg("-e")
            .arg("POSIX path of (choose folder with prompt \"Select Workspace Directory\")")
            .output();

        if let Ok(res) = output {
            if res.status.success() {
                let path = String::from_utf8_lossy(&res.stdout).trim().to_string();
                if !path.is_empty() {
                    return Json(Some(path));
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // powershell script to open a folder browser dialog
        let script = "Add-Type -AssemblyName System.Windows.Forms; $f = New-Object System.Windows.Forms.FolderBrowserDialog; if ($f.ShowDialog() -eq 'OK') { $f.SelectedPath }";
        let output = Command::new("powershell")
            .arg("-Command")
            .arg(script)
            .output();

        if let Ok(res) = output {
            if res.status.success() {
                let path = String::from_utf8_lossy(&res.stdout).trim().to_string();
                if !path.is_empty() {
                    return Json(Some(path));
                }
            }
        }
    }

    Json(None)
}

#[derive(Deserialize)]
#[allow(non_camel_case_types)]
pub struct test_cli_params {
    pub command: String,
    pub workspace_dir: Option<String>,
}

pub async fn test_cli(
    State((config_inst, _)): State<(Arc<config>, Arc<session_store>)>,
    Json(params): Json<test_cli_params>,
) -> Json<serde_json::Value> {
    let ws = params.workspace_dir.map(std::path::PathBuf::from).unwrap_or_else(|| config_inst.workspace_dir.clone());
    let provider = cli_provider::new(params.command, ws);
    
    match provider.call_model("test", "ping", Some("respond with 'pong' if you are working"), 0.7, 10).await {
        Ok(_) => Json(serde_json::json!({"status": "success"})),
        Err(e) => Json(serde_json::json!({"status": "error", "message": e.to_string()})),
    }
}

pub async fn handle_enclave(
    Query(params): Query<enclave_params>,
    State((config_inst, session_store_inst)): State<(Arc<config>, Arc<session_store>)>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    tracing::info!("Enclave convening for query: {}", params.query);
    let (tx, rx) = mpsc::channel(100);
    let has_session = params.session_id.is_some();
    let session_id = params.session_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let autonomous = params.autonomous.unwrap_or(config_inst.autonomous_mode);
    let ws = params.workspace_dir.map(std::path::PathBuf::from).unwrap_or_else(|| config_inst.workspace_dir.clone());

    tracing::info!("Session ID: {}, Workspace: {:?}", session_id, ws);

    // Load previous session history if session_id was provided
    let prev_history = if has_session {
        session_store_inst.get_history(&session_id).await
    } else {
        vec![]
    };

    // Initialize logger for this session
    let logger = Arc::new(session_logger::new(ws.clone()));

    // Resolve binaries (UI override > config)
    let s_bin = params.strategist_binary.unwrap_or_else(|| config_inst.strategist_binary.clone());
    let c_bin = params.critic_binary.unwrap_or_else(|| config_inst.critic_binary.clone());
    let o_bin = params.optimizer_binary.unwrap_or_else(|| config_inst.optimizer_binary.clone());
    let ct_bin = params.maintainer_binary.unwrap_or_else(|| config_inst.contrarian_binary.clone());
    let j_bin = params.judge_binary.unwrap_or_else(|| config_inst.judge_binary.clone());

    // setup orchestrator using local cli binaries with workspace context
    let strategist_provider: Arc<dyn model_provider> = Arc::new(cli_provider::new(s_bin, ws.clone()).with_logger(logger.clone()).with_autonomous(autonomous));
    let critic_provider: Arc<dyn model_provider> = Arc::new(cli_provider::new(c_bin, ws.clone()).with_logger(logger.clone()).with_autonomous(autonomous));
    let optimizer_provider: Arc<dyn model_provider> = Arc::new(cli_provider::new(o_bin, ws.clone()).with_logger(logger.clone()).with_autonomous(autonomous));
    let maintainer_provider: Arc<dyn model_provider> = Arc::new(cli_provider::new(ct_bin, ws.clone()).with_logger(logger.clone()).with_autonomous(autonomous));
    let judge_provider: Arc<dyn model_provider> = Arc::new(cli_provider::new(j_bin, ws.clone()).with_logger(logger.clone()).with_autonomous(autonomous));

    let mut agents = vec![
        roles::strategist(strategist_provider, "cli", config_inst.default_temperature, config_inst.max_tokens_per_agent),
        roles::critic(critic_provider, "cli", config_inst.default_temperature, config_inst.max_tokens_per_agent),
        roles::optimizer(optimizer_provider, "cli", config_inst.default_temperature, config_inst.max_tokens_per_agent),
        roles::contrarian(maintainer_provider, "cli", config_inst.default_temperature, config_inst.max_tokens_per_agent),
    ];

    // enable autonomous mode if requested
    for agent in &mut agents {
        agent.set_autonomous(autonomous);
    }

    let mut judge = judge_agent::new(judge_provider, "cli", config_inst.default_temperature, 1000);
    judge.base.set_autonomous(autonomous);
    
    let mut orchestrator_inst = orchestrator::new(
        agents,
        judge,
        params.rounds.unwrap_or(config_inst.max_rounds),
        20,
        ws
    );
    orchestrator_inst.logger = logger.clone();

    let query = params.query;
    let store_clone = session_store_inst.clone();
    let sid_clone = session_id.clone();

    tokio::spawn(async move {
        // send session id as first message
        let _ = tx.send(Event::default().event("session_info").data(serde_json::json!({"session_id": sid_clone}).to_string())).await;

        // restore session history if available
        if !prev_history.is_empty() {
            let history_preview: Vec<String> = prev_history.iter().map(|m| format!("[{}]: {}", m.agent, &m.content[..m.content.len().min(50)])).collect();
            tracing::info!("Loading {} messages into session: {:?}", prev_history.len(), history_preview);
            orchestrator_inst.load_session_history(prev_history).await;
        }

        let logger_err = logger.clone();

        // store user query in session
        let user_query_response = agent_response {
            agent: "User".to_string(),
            content: query.clone(),
            terminal_output: String::new(),
            round: 0,
        };
        store_clone.add_message(&session_id, user_query_response).await;

        let _ = orchestrator_inst.run_council(&query, |resp| {
            let tx_clone = tx.clone();
            let store = store_clone.clone();
            let sid = sid_clone.clone();
            async move {
                if let Ok(json) = serde_json::to_string(&resp) {
                    if tx_clone.send(Event::default().data(json)).await.is_err() {
                        tracing::warn!("client disconnected, aborting enclave session.");
                        return Err(());
                    }
                }
                
                // store in session
                store.add_message(&sid, resp).await;
                Ok(())
            }
        }).await.inspect_err(|e| {
            let err_msg = e.to_string();
            tokio::spawn(async move {
                let _ = logger_err.log(&format!("fatal orchestrator error: {}", err_msg)).await;
            });
        });
    });

    let stream = stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|event| (Result::<Event, Infallible>::Ok(event), rx))
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
#[allow(non_camel_case_types)]
pub struct apply_change_params {
    pub path: String,
    pub content: String,
}

pub async fn apply_change(
    State((config_inst, _)): State<(Arc<config>, Arc<session_store>)>,
    Json(params): Json<apply_change_params>,
) -> Json<serde_json::Value> {
    // secure the path to prevent traversal attacks outside the workspace
    let ws = &config_inst.workspace_dir;
    let target_path = std::path::Path::new(&params.path);
    
    // basic sanitization: reject absolute paths and '..' components
    if target_path.is_absolute() || target_path.components().any(|c| c.as_os_str() == "..") {
        return Json(serde_json::json!({"status": "error", "message": "invalid path: path traversal detected"}));
    }

    let full_path = ws.join(target_path);

    // ensure the parent directory exists
    if let Some(parent) = full_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return Json(serde_json::json!({"status": "error", "message": format!("failed to create directories: {}", e)}));
        }
    }

    match tokio::fs::write(&full_path, &params.content).await {
        Ok(_) => Json(serde_json::json!({"status": "success"})),
        Err(e) => Json(serde_json::json!({"status": "error", "message": e.to_string()})),
    }
}

pub async fn get_session_history(
    Path(session_id): Path<String>,
    State((_, session_store_inst)): State<(Arc<config>, Arc<session_store>)>,
) -> Json<Vec<agent_response>> {
    Json(session_store_inst.get_history(&session_id).await)
}

pub async fn list_sessions(
    State((_, session_store_inst)): State<(Arc<config>, Arc<session_store>)>,
) -> Json<Vec<crate::api::sessions_mod::SessionSummary>> {
    Json(session_store_inst.list_sessions().await)
}

pub async fn delete_session(
    Path(session_id): Path<String>,
    State((_, session_store_inst)): State<(Arc<config>, Arc<session_store>)>,
) -> Json<serde_json::Value> {
    let deleted = session_store_inst.delete_session(&session_id).await;
    Json(serde_json::json!({"status": if deleted { "success" } else { "error" }, "deleted": deleted}))
}
