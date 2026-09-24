use crate::{
    AgentRuntimeEvent, AgentSession, AgentTurnInput, AgentTurnStarted, ChatEvent, Conversation,
    ConversationSnapshot, Id, Principal, Project, RunFeed, SubmittedMessage, ToolGrant,
};
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;

pub type PlatformResult<T> = Result<T, PlatformError>;

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("not found")]
    NotFound,
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("a run is already active for this project")]
    RunAlreadyActive,
    #[error("unauthorized")]
    Unauthorized,
    #[error("project has not been granted external tool: {0}")]
    ToolNotGranted(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("runtime error: {0}")]
    Runtime(String),
}

pub trait PlatformRepository: Send + Sync {
    fn list_projects(&self, principal: &Principal) -> PlatformResult<Vec<Project>>;
    fn create_project(&self, principal: &Principal, name: &str) -> PlatformResult<Project>;
    fn get_project(&self, principal: &Principal, project_id: &str) -> PlatformResult<Project>;
    fn list_tool_grants(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<Vec<ToolGrant>>;
    fn is_tool_granted(
        &self,
        principal: &Principal,
        project_id: &str,
        tool_id: &str,
    ) -> PlatformResult<bool>;
    fn grant_tool(
        &self,
        principal: &Principal,
        project_id: &str,
        tool_id: &str,
    ) -> PlatformResult<ToolGrant>;
    fn revoke_tool(
        &self,
        principal: &Principal,
        project_id: &str,
        tool_id: &str,
    ) -> PlatformResult<bool>;
    fn get_or_create_agent_session(
        &self,
        principal: &Principal,
        project: &Project,
        runtime: &str,
    ) -> PlatformResult<AgentSession>;
    fn mark_agent_session_ready(
        &self,
        principal: &Principal,
        session_id: &str,
    ) -> PlatformResult<()>;
    fn record_agent_session_heartbeat(
        &self,
        principal: &Principal,
        session_id: &str,
    ) -> PlatformResult<()>;
    fn get_or_create_conversation(
        &self,
        principal: &Principal,
        project: &Project,
    ) -> PlatformResult<Conversation>;
    fn conversation_snapshot(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<ConversationSnapshot>;
    fn submit_message(
        &self,
        principal: &Principal,
        project: &Project,
        conversation: &Conversation,
        session: &AgentSession,
        content: &str,
    ) -> PlatformResult<SubmittedMessage>;
    fn set_conversation_thread(
        &self,
        principal: &Principal,
        conversation_id: &str,
        thread_id: &str,
    ) -> PlatformResult<()>;
    fn mark_run_started(
        &self,
        principal: &Principal,
        run_id: &str,
        turn_id: &str,
    ) -> PlatformResult<()>;
    fn mark_run_waiting_approval(&self, principal: &Principal, run_id: &str) -> PlatformResult<()>;
    fn mark_run_resumed(&self, principal: &Principal, run_id: &str) -> PlatformResult<()>;
    fn append_run_event(
        &self,
        principal: &Principal,
        run_id: &str,
        kind: &str,
        payload: Value,
    ) -> PlatformResult<ChatEvent>;
    fn append_assistant_text(
        &self,
        principal: &Principal,
        run_id: &str,
        text: &str,
    ) -> PlatformResult<()>;
    fn finish_run(
        &self,
        principal: &Principal,
        run_id: &str,
        status: crate::AgentRunStatus,
        error: Option<&str>,
    ) -> PlatformResult<()>;
    fn get_run_feed(
        &self,
        principal: &Principal,
        project_id: &str,
        run_id: &str,
        after_sequence: i64,
    ) -> PlatformResult<RunFeed>;
}

#[async_trait]
pub trait AgentRuntime: Send + Sync {
    async fn start_turn(
        &self,
        input: AgentTurnInput,
    ) -> PlatformResult<(AgentTurnStarted, mpsc::Receiver<AgentRuntimeEvent>)>;
    async fn respond_to_approval(
        &self,
        project_id: &str,
        request_id: &str,
        decision: &str,
    ) -> PlatformResult<()>;
    async fn close(&self) -> PlatformResult<()>;
}

#[async_trait]
pub trait WorkspaceProvider: Send + Sync {
    async fn ensure_workspace(&self, project: &Project) -> PlatformResult<String>;
}

#[async_trait]
pub trait IdentityProvider: Send + Sync {
    async fn authenticate(&self, authorization: Option<&str>) -> PlatformResult<Principal>;
}

#[async_trait]
pub trait AgentWakeupScheduler: Send + Sync {
    async fn schedule_wakeup(&self, project_id: &str, session_id: &str) -> PlatformResult<()>;
}

pub fn opaque_id(prefix: &str) -> Id {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}
