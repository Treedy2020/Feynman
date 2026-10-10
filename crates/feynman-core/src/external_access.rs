//! External Access public metadata. Never serialize credentials here.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccessProvider {
    Dws,
    Gitlab,
    Git,
    Slate,
    Drlee,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CliProviderDescriptor {
    pub provider: AccessProvider,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub summary: String,
    /// Implemented managed Observation Source kinds, not every readable tool.
    #[serde(default)]
    pub source_kinds: Vec<String>,
    pub executable: String,
    pub authentication: Vec<String>,
    pub capabilities: Vec<String>,
    pub delivery: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExternalConnection {
    pub id: String,
    pub tenant_id: String,
    pub owner_user_id: String,
    pub provider: AccessProvider,
    pub hostname: Option<String>,
    pub identity: String,
    pub created_at: DateTime<Utc>,
    pub disabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectCliBinding {
    pub project_id: String,
    pub provider: AccessProvider,
    pub connection_id: String,
    pub repository: Option<String>,
    #[serde(default)]
    pub resource_id: Option<String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExternalAccessRequest {
    pub id: String,
    pub project_id: String,
    pub provider: AccessProvider,
    pub capability: String,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}
