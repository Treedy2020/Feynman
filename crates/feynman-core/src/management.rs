use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Runtime transport is private implementation metadata, never an authorization token.
#[derive(Clone, Debug)]
pub struct NativeExecutionEnvironment {
    pub id: String,
    pub exec_server_url: String,
    pub cwd: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    Local,
    Opensandbox,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectWorkspace {
    pub project_id: String,
    pub provider: WorkspaceKind,
    pub state: String,
    pub cwd: String,
    pub environment_id: String,
    pub sandbox_id: Option<String>,
    pub volume_name: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct PreparedWorkspace {
    pub local_cwd: String,
    pub environment: Option<NativeExecutionEnvironment>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectTask {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub prompt: String,
    pub status: String,
    pub schedule_id: Option<String>,
    pub run_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectSchedule {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub prompt: String,
    pub interval_seconds: Option<i64>,
    pub next_due_at: DateTime<Utc>,
    pub enabled: bool,
    pub last_task_id: Option<String>,
    #[serde(default)]
    pub last_inbox_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default = "crate::queue")]
    pub delivery: String,
    #[serde(default)]
    pub collect_for_seconds: Option<i64>,
    #[serde(default)]
    pub exhausted: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTaskRequest {
    pub title: String,
    pub prompt: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateScheduleRequest {
    pub title: String,
    pub prompt: String,
    pub interval_seconds: Option<i64>,
    pub at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub enabled: bool,
    pub agent_id: Option<String>,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default = "crate::queue")]
    pub delivery: String,
    pub collect_for_seconds: Option<i64>,
}
