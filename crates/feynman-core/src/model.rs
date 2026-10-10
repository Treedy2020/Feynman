use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type Id = String;

/// Public authorization metadata only. Credentials and device codes are never serialized here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DwsAuthorization {
    pub id: Id,
    pub project_id: Id,
    pub reason: String,
    pub status: DwsAuthorizationStatus,
    pub verification_uri: Option<String>,
    pub verification_uri_complete: Option<String>,
    pub user_code: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub identity: Option<DwsIdentity>,
    pub error_code: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub checked_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DwsAuthorizationStatus {
    Preparing,
    WaitingForUser,
    Authorized,
    Expired,
    Denied,
    Failed,
    Cancelled,
    Interrupted,
    AuthenticationRequired,
}

impl DwsAuthorizationStatus {
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Preparing | Self::WaitingForUser)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DwsIdentity {
    pub corp_id: String,
    pub user_id: String,
    pub corp_name: Option<String>,
    pub user_name: Option<String>,
    pub token_expires_at: Option<DateTime<Utc>>,
    pub refresh_expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Principal {
    pub user_id: Id,
    pub tenant_id: Id,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub id: Id,
    pub tenant_id: Id,
    pub name: String,
    pub main_agent_id: Id,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSession {
    pub id: Id,
    pub tenant_id: Id,
    pub project_id: Id,
    pub runtime: String,
    pub status: AgentSessionStatus,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolGrant {
    pub tenant_id: Id,
    pub project_id: Id,
    pub tool_id: String,
    pub granted_by_user_id: Id,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectToolAccess {
    pub id: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub granted: bool,
    pub grant: Option<ToolGrant>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionStatus {
    Provisioning,
    Ready,
    Degraded,
    Stopped,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Conversation {
    pub id: Id,
    pub tenant_id: Id,
    pub project_id: Id,
    pub main_agent_id: Id,
    pub runtime_thread_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationMessage {
    pub id: Id,
    pub tenant_id: Id,
    pub conversation_id: Id,
    pub run_id: Option<Id>,
    pub role: MessageRole,
    pub content: String,
    pub status: MessageStatus,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Streaming,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRun {
    pub id: Id,
    pub tenant_id: Id,
    pub project_id: Id,
    pub conversation_id: Id,
    pub agent_session_id: Id,
    pub user_message_id: Id,
    pub assistant_message_id: Id,
    pub status: AgentRunStatus,
    pub runtime_turn_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    Queued,
    Running,
    WaitingApproval,
    Completed,
    Failed,
    Interrupted,
}

impl AgentRunStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Interrupted)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChatEvent {
    pub id: Id,
    pub tenant_id: Id,
    pub run_id: Id,
    pub sequence: i64,
    pub kind: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversationSnapshot {
    pub conversation: Conversation,
    pub messages: Vec<ConversationMessage>,
    pub runs: Vec<AgentRun>,
    pub active_run: Option<AgentRun>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubmittedMessage {
    pub conversation: Conversation,
    pub user_message: ConversationMessage,
    pub run: AgentRun,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunFeed {
    pub run: AgentRun,
    pub events: Vec<ChatEvent>,
}

#[derive(Clone, Debug)]
pub struct AgentTurnInput {
    pub principal: Principal,
    pub project: Project,
    pub conversation: Conversation,
    pub session: AgentSession,
    pub message: String,
    pub workspace_path: String,
    pub execution_environment: Option<crate::NativeExecutionEnvironment>,
    pub instructions: String,
}

#[derive(Clone, Debug)]
pub struct AgentTurnStarted {
    pub session_id: Id,
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Clone, Debug)]
pub enum AgentRuntimeEvent {
    TextDelta {
        text: String,
    },
    ToolActivity {
        status: String,
        tool: String,
        message: Option<String>,
    },
    ApprovalRequested {
        request_id: String,
        kind: String,
        summary: String,
    },
    Completed {
        status: AgentRunStatus,
        error: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    #[serde(default)]
    pub without_agent: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendMessageResponse {
    pub conversation: Conversation,
    pub user_message: ConversationMessage,
    pub run: AgentRun,
}

/// A durable attention record, not an Inbox input or an approval decision.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Notification {
    pub sequence: i64,
    pub id: String,
    pub project_id: String,
    pub project_name: String,
    pub run_id: String,
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub read: bool,
    pub resolved: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotificationFeed {
    pub items: Vec<Notification>,
    pub next_after: i64,
    pub head: i64,
    pub unread: i64,
    pub has_more: bool,
}
