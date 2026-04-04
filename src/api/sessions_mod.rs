use tokio::sync::Mutex;
use std::collections::HashMap;
use crate::core::agent_response;
use std::path::PathBuf;
use std::fs;
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

/// Session metadata for hierarchical session management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub workspace: PathBuf,
    pub model: String,
    pub status: SessionStatus,
    pub worktree_path: Option<PathBuf>,
    pub branch: Option<String>,
    pub parent_session_id: Option<String>,
    pub child_session_ids: Vec<String>,
    pub inherited_summary: Option<String>,
    pub spawn_reason: Option<String>,
    pub session_summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SessionStatus {
    Active,
    Completed,
    Failed,
    Archived,
}

impl Default for SessionStatus {
    fn default() -> Self {
        SessionStatus::Active
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub first_message: String,
    pub message_count: usize,
    pub parent_session_id: Option<String>,
    pub child_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichedSession {
    pub meta: SessionMeta,
    pub messages: Vec<agent_response>,
}

#[allow(non_camel_case_types)]
pub struct session_store {
    pub sessions: Mutex<HashMap<String, EnrichedSession>>,
    pub workspace_dir: PathBuf,
}

#[allow(non_camel_case_types)]
impl session_store {
    pub fn new(workspace_dir: PathBuf) -> Self {
        let mut store = Self {
            sessions: Mutex::new(HashMap::new()),
            workspace_dir,
        };
        store.load_from_disk();
        store
    }

    fn get_history_path(&self) -> PathBuf {
        self.workspace_dir.join(".enclave_history.json")
    }

    fn load_from_disk(&mut self) {
        let path = self.get_history_path();
        if path.exists() {
            match fs::read_to_string(path) {
                Ok(data) => {
                    match serde_json::from_str::<HashMap<String, EnrichedSession>>(&data) {
                        Ok(loaded_sessions) => {
                            *self.sessions.get_mut() = loaded_sessions;
                        }
                        Err(e) => {
                            eprintln!("Warning: failed to parse session history: {}", e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Warning: failed to read session history file: {}", e);
                }
            }
        }
    }

    /// Create a new session
    pub async fn create_session(
        &self,
        session_id: String,
        model: String,
        parent_session_id: Option<String>,
        spawn_reason: Option<String>,
    ) -> SessionMeta {
        let now = Utc::now();
        let meta = SessionMeta {
            session_id: session_id.clone(),
            created_at: now,
            updated_at: now,
            workspace: self.workspace_dir.clone(),
            model,
            status: SessionStatus::Active,
            worktree_path: None,
            branch: None,
            parent_session_id: parent_session_id.clone(),
            child_session_ids: Vec::new(),
            inherited_summary: None,
            spawn_reason,
            session_summary: None,
        };

        let session = EnrichedSession {
            meta: meta.clone(),
            messages: Vec::new(),
        };

        let mut sessions = self.sessions.lock().await;
        sessions.insert(session_id.clone(), session);

        // Update parent to include this child
        if let Some(parent_id) = parent_session_id {
            if let Some(parent) = sessions.get_mut(&parent_id) {
                parent.meta.child_session_ids.push(session_id.clone());
            }
        }

        drop(sessions);
        self.save_to_disk().await;

        meta
    }

    /// Add a message to a session
    pub async fn add_message(&self, session_id: &str, msg: agent_response) {
        let mut sessions = self.sessions.lock().await;

        if let Some(session) = sessions.get_mut(session_id) {
            session.messages.push(msg.clone());
            session.meta.updated_at = Utc::now();

            // Save while still holding the lock
            let data = serde_json::to_string_pretty(&*sessions)
                .unwrap_or_else(|e| {
                    eprintln!("Warning: failed to serialize session: {}", e);
                    String::new()
                });

            drop(sessions);

            if !data.is_empty() {
                if let Err(e) = tokio::fs::write(self.get_history_path(), data).await {
                    eprintln!("Warning: failed to persist session: {}", e);
                }
            }
        }
    }

    /// Update session metadata
    pub async fn update_session_meta(&self, session_id: &str, update: SessionMetaUpdate) {
        let mut sessions = self.sessions.lock().await;

        if let Some(session) = sessions.get_mut(session_id) {
            if let Some(status) = update.status {
                session.meta.status = status;
            }
            if let Some(worktree_path) = update.worktree_path {
                session.meta.worktree_path = Some(worktree_path);
            }
            if let Some(branch) = update.branch {
                session.meta.branch = Some(branch);
            }
            if let Some(summary) = update.session_summary {
                session.meta.session_summary = Some(summary);
            }
            if let Some(inherited) = update.inherited_summary {
                session.meta.inherited_summary = Some(inherited);
            }

            session.meta.updated_at = Utc::now();

            let data = match serde_json::to_string_pretty(&*sessions) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Warning: failed to serialize session: {}", e);
                    String::new()
                }
            };

            drop(sessions);

            if !data.is_empty() {
                if let Err(e) = tokio::fs::write(self.get_history_path(), data).await {
                    eprintln!("Warning: failed to persist session: {}", e);
                }
            }
        }
    }

    async fn save_to_disk(&self) {
        let sessions = self.sessions.lock().await;
        let data = serde_json::to_string_pretty(&*sessions)
            .unwrap_or_else(|e| {
                eprintln!("Warning: failed to serialize session: {}", e);
                String::new()
            });

        if !data.is_empty() {
            if let Err(e) = tokio::fs::write(self.get_history_path(), data).await {
                eprintln!("Warning: failed to persist session: {}", e);
            }
        }
    }

    pub async fn get_history(&self, session_id: &str) -> Vec<agent_response> {
        let sessions: tokio::sync::MutexGuard<'_, HashMap<String, EnrichedSession>> = self.sessions.lock().await;
        sessions.get(session_id).map(|s| s.messages.clone()).unwrap_or_default()
    }

    pub async fn get_session(&self, session_id: &str) -> Option<EnrichedSession> {
        let sessions: tokio::sync::MutexGuard<'_, HashMap<String, EnrichedSession>> = self.sessions.lock().await;
        sessions.get(session_id).cloned()
    }

    pub async fn get_child_sessions(&self, parent_id: &str) -> Vec<String> {
        let sessions: tokio::sync::MutexGuard<'_, HashMap<String, EnrichedSession>> = self.sessions.lock().await;
        sessions.get(parent_id)
            .map(|s| s.meta.child_session_ids.clone())
            .unwrap_or_default()
    }

    pub async fn list_sessions(&self) -> Vec<SessionSummary> {
        let sessions: tokio::sync::MutexGuard<'_, HashMap<String, EnrichedSession>> = self.sessions.lock().await;
        sessions.iter()
            .map(|(session_id, session)| {
                let first_message = session.messages.first()
                    .map(|m| {
                        if m.agent == "User" {
                            m.content.clone()
                        } else {
                            m.content.chars().take(100).collect::<String>()
                        }
                    })
                    .unwrap_or_default();

                SessionSummary {
                    session_id: session_id.clone(),
                    first_message,
                    message_count: session.messages.len(),
                    parent_session_id: session.meta.parent_session_id.clone(),
                    child_count: session.meta.child_session_ids.len(),
                }
            })
            .collect()
    }

    pub async fn delete_session(&self, session_id: &str) -> bool {
        let mut sessions = self.sessions.lock().await;

        // Get the session to find its parent (clone to avoid borrow issues)
        let parent_id = sessions.get(session_id)
            .and_then(|s| s.meta.parent_session_id.clone());

        // Remove this session from parent's child list
        if let Some(ref pid) = parent_id {
            if let Some(parent) = sessions.get_mut(pid) {
                parent.meta.child_session_ids.retain(|id| id != session_id);
            }
        }

        let removed = sessions.remove(session_id).is_some();

        if removed {
            let data = match serde_json::to_string_pretty(&*sessions) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Warning: failed to serialize session: {}", e);
                    String::new()
                }
            };

            drop(sessions);

            if !data.is_empty() {
                if let Err(e) = tokio::fs::write(self.get_history_path(), data).await {
                    eprintln!("Warning: failed to persist session after deletion: {}", e);
                }
            }
            true
        } else {
            false
        }
    }
}

#[derive(Default)]
pub struct SessionMetaUpdate {
    pub status: Option<SessionStatus>,
    pub worktree_path: Option<PathBuf>,
    pub branch: Option<String>,
    pub session_summary: Option<String>,
    pub inherited_summary: Option<String>,
}
