use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use teloxide::types::MessageId;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStep {
    SelectEnvironment,
    SelectProject,
    SelectBranch,
    WaitingManualBranch,
    SelectAction,
    Confirm,
    DoubleConfirm,
    FastPresetList,
    FastPresetManageList,
    FastPresetManageOne,
    FastPresetCreateName,
    FastPresetEditField,
    FastPresetDeleteConfirm,
    Done,
}

impl SessionStep {
    pub fn next_after_confirm() -> Self {
        SessionStep::Done
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, SessionStep::Done)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployAction {
    BuildOnly,
    BackupAndDeploy,
}

impl DeployAction {
    pub fn label(&self) -> &'static str {
        match self {
            DeployAction::BuildOnly => "🧱 Build only",
            DeployAction::BackupAndDeploy => "🚀 Backup + Deploy IIS",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    pub owner_user_id: i64,
    pub chat_id: i64,
    pub message_id: Option<MessageId>,
    pub step: SessionStep,
    pub environment_key: Option<String>,
    pub project_key: Option<String>,
    pub branch: Option<String>,
    pub commit_hash: Option<String>,
    pub action: Option<DeployAction>,
    pub fast_preset_id: Option<String>,
    pub fast_preset_name: Option<String>,
    pub fast_preset_editing: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl Session {
    pub fn new(owner_user_id: i64, chat_id: i64) -> Self {
        let now = Utc::now();
        Self {
            session_id: Uuid::new_v4().to_string(),
            owner_user_id,
            chat_id,
            message_id: None,
            step: SessionStep::SelectEnvironment,
            environment_key: None,
            project_key: None,
            branch: None,
            commit_hash: None,
            action: None,
            fast_preset_id: None,
            fast_preset_name: None,
            fast_preset_editing: false,
            created_at: now,
            expires_at: now + Duration::minutes(10),
        }
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    pub fn refresh_timeout(&mut self) {
        self.expires_at = Utc::now() + Duration::minutes(10);
    }

    pub fn set_step(&mut self, step: SessionStep) {
        self.step = step;
        self.refresh_timeout();
    }
}

#[derive(Clone)]
pub struct SessionStore {
    sessions: Arc<Mutex<HashMap<String, Session>>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn create(&self, owner_user_id: i64, chat_id: i64) -> Session {
        let session = Session::new(owner_user_id, chat_id);
        let id = session.session_id.clone();
        self.sessions.lock().await.insert(id, session.clone());
        session
    }

    pub async fn get(&self, session_id: &str) -> Option<Session> {
        let sessions = self.sessions.lock().await;
        sessions.get(session_id).cloned()
    }

    pub async fn update(&self, session: Session) {
        let id = session.session_id.clone();
        self.sessions.lock().await.insert(id, session);
    }

    pub async fn remove(&self, session_id: &str) {
        self.sessions.lock().await.remove(session_id);
    }

    pub async fn find_by_chat_and_user(&self, chat_id: i64, user_id: i64) -> Option<Session> {
        let sessions = self.sessions.lock().await;
        sessions
            .values()
            .find(|s| s.chat_id == chat_id && s.owner_user_id == user_id && !s.is_expired())
            .cloned()
    }

    pub async fn find_active_for_chat(&self, chat_id: i64) -> Option<Session> {
        let sessions = self.sessions.lock().await;
        sessions
            .values()
            .find(|s| s.chat_id == chat_id && !s.is_expired())
            .cloned()
    }

    pub async fn cleanup_expired(&self) {
        let mut sessions = self.sessions.lock().await;
        sessions.retain(|_, s| !s.is_expired());
    }
}
