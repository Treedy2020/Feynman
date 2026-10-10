//! Durable identities and immutable observation partitions. No credentials in public DTOs.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectAgent {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub state: String,
    pub is_default: bool,
    /// None is the migrated legacy policy. New identities start with no capabilities.
    pub capabilities: Option<Vec<String>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInput {
    pub name: String,
    #[serde(default = "active")]
    pub state: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub capabilities: Vec<String>,
}
fn active() -> String {
    "active".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObservationSource {
    pub id: String,
    pub project_id: String,
    pub agent_id: String,
    pub name: String,
    pub kind: String,
    pub connection_id: Option<String>,
    pub arguments: Value,
    pub enabled: bool,
    pub poll_seconds: i64,
    pub cursor: DateTime<Utc>,
    pub next_poll_at: DateTime<Utc>,
    pub coverage: String,
    #[serde(default)]
    pub listener_state: String,
    #[serde(default)]
    pub listener_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceInput {
    pub agent_id: Option<String>,
    pub name: String,
    pub kind: String,
    pub connection_id: Option<String>,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "poll_seconds")]
    pub poll_seconds: i64,
    pub start: Option<DateTime<Utc>>,
}
fn poll_seconds() -> i64 {
    60
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationInput {
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub body: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub sequence: i64,
    pub source_id: String,
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub body: Value,
}

/// Read-only channel projection. No connection credentials or provider arguments.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObservationChannel {
    pub source_id: String,
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub listener_state: String,
    pub coverage: String,
    pub poll_seconds: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationFeedQuery {
    pub source_id: Option<String>,
    #[serde(default)]
    pub after: i64,
    #[serde(default = "feed_limit")]
    pub limit: i64,
    /// Return the currently authorized high watermark without reading old bodies.
    #[serde(default)]
    pub head: bool,
}
fn feed_limit() -> i64 {
    100
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObservationFeedPage {
    pub records: Vec<Observation>,
    pub next_after: i64,
    pub has_more: bool,
    pub channels: Vec<ObservationChannel>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InboxMessage {
    pub id: String,
    pub project_id: String,
    pub agent_id: String,
    pub schedule_id: Option<String>,
    pub prompt: String,
    pub delivery: String,
    pub state: String,
    pub run_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxInput {
    pub agent_id: Option<String>,
    pub content: String,
    #[serde(default = "queue")]
    pub delivery: String,
}
pub fn queue() -> String {
    "queue".into()
}
