use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::session::DeployAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Success,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn label(&self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Success => "success",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub job_id: String,
    pub requested_by: i64,
    pub chat_id: i64,
    pub project_key: String,
    pub environment_key: String,
    pub branch: String,
    pub commit_hash: Option<String>,
    pub action: DeployAction,
    pub status: JobStatus,
    pub stage: String,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub log: Vec<String>,
}

impl Job {
    pub fn new(
        requested_by: i64,
        chat_id: i64,
        project_key: String,
        environment_key: String,
        branch: String,
        action: DeployAction,
    ) -> Self {
        Self {
            job_id: Uuid::new_v4().to_string(),
            requested_by,
            chat_id,
            project_key,
            environment_key,
            branch,
            commit_hash: None,
            action,
            status: JobStatus::Queued,
            stage: "queued".to_string(),
            error: None,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            log: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct JobStore {
    jobs: Arc<Mutex<HashMap<String, Job>>>,
}

impl JobStore {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn insert(&self, job: Job) {
        self.jobs.lock().await.insert(job.job_id.clone(), job);
    }

    pub async fn get(&self, job_id: &str) -> Option<Job> {
        self.jobs.lock().await.get(job_id).cloned()
    }

    pub async fn update(&self, job: Job) {
        self.jobs.lock().await.insert(job.job_id.clone(), job);
    }

    pub async fn recent_for_chat(&self, chat_id: i64, limit: usize) -> Vec<Job> {
        let mut jobs: Vec<Job> = self
            .jobs
            .lock()
            .await
            .values()
            .filter(|j| j.chat_id == chat_id)
            .cloned()
            .collect();
        jobs.sort_by_key(|j| Reverse(j.created_at));
        jobs.truncate(limit);
        jobs
    }

    pub async fn has_running_target(&self, project_key: &str, environment_key: &str) -> bool {
        self.jobs.lock().await.values().any(|j| {
            j.project_key == project_key
                && j.environment_key == environment_key
                && matches!(j.status, JobStatus::Queued | JobStatus::Running)
        })
    }
}
