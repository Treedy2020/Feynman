//! Public metadata only: secret values never implement a public response contract.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SecretMetadata {
    pub id: String,
    pub name: String,
    pub version: i64,
    pub storage: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentProfile {
    pub name: String,
    pub argv: Vec<String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    /// Environment variable -> secret reference; file is materialized in verified tmpfs.
    #[serde(default)]
    pub secret_files: BTreeMap<String, String>,
    #[serde(default = "lease_seconds")]
    pub max_seconds: i64,
}
fn lease_seconds() -> i64 {
    1800
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceRecord {
    pub id: String,
    pub revision: i64,
    pub data: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub source_id: Option<String>,
    pub observation_sequence: Option<i64>,
    pub memory_id: Option<String>,
    pub memory_revision: Option<i64>,
    pub git_commit: Option<String>,
    pub run_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryInput {
    pub title: String,
    pub body: String,
    pub kind: String,
    #[serde(default = "agent_scope")]
    pub scope: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default = "open_state")]
    pub state: String,
}
fn agent_scope() -> String {
    "agent".into()
}
fn open_state() -> String {
    "open".into()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryItem {
    pub id: String,
    pub revision: i64,
    pub content: MemoryInput,
    pub author: String,
    pub confirmed: bool,
    pub pinned: bool,
    pub updated_at: DateTime<Utc>,
}
