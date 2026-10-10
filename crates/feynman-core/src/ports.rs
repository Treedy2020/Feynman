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
    fn check_agent_active(&self, _principal: &Principal, _project_id: &str) -> PlatformResult<()> {
        Ok(())
    }
    fn check_agent_route(
        &self,
        p: &Principal,
        project: &str,
        _agent: Option<&str>,
    ) -> PlatformResult<()> {
        self.check_agent_active(p, project)
    }
    fn check_agent_tool(
        &self,
        principal: &Principal,
        project_id: &str,
        tool: &str,
    ) -> PlatformResult<()> {
        self.check_agent_active(principal, project_id)?;
        if self.is_tool_granted(principal, project_id, tool)? {
            Ok(())
        } else {
            Err(PlatformError::ToolNotGranted(tool.into()))
        }
    }
    fn list_projects(&self, principal: &Principal) -> PlatformResult<Vec<Project>>;
    fn create_project(&self, principal: &Principal, name: &str) -> PlatformResult<Project>;
    fn create_project_configured(
        &self,
        p: &Principal,
        name: &str,
        without_agent: bool,
    ) -> PlatformResult<Project> {
        if without_agent {
            return Err(PlatformError::Conflict(
                "repository does not support a project without Agent".into(),
            ));
        }
        self.create_project(p, name)
    }
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
        task_id: Option<&str>,
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
    fn notifications(&self, _principal: &Principal, _after: i64, _limit: i64) -> PlatformResult<crate::NotificationFeed> {
        Err(crate::PlatformError::Runtime("notifications are not configured".into()))
    }
    fn read_notification(&self, _principal: &Principal, _id: &str) -> PlatformResult<()> {
        Err(crate::PlatformError::Runtime("notifications are not configured".into()))
    }
    fn stream_snapshot(&self, _principal: &Principal, _project: &str) -> PlatformResult<(ConversationSnapshot, Option<RunFeed>)> {
        Err(PlatformError::Runtime("consistent stream snapshots are not configured".into()))
    }
    /// Production stores must commit the message and its event together.
    fn append_assistant_delta(&self, principal: &Principal, run: &str, text: &str) -> PlatformResult<ChatEvent> {
        self.append_assistant_text(principal, run, text)?;
        self.append_run_event(principal, run, "agent_text_delta", serde_json::json!({"text":text}))
    }
}

#[async_trait]
pub trait AgentRuntime: Send + Sync {
    /// None means an explicit non-acceptance (ended turn / unsupported). Errors are ambiguous.
    async fn steer(
        &self,
        _project_id: &str,
        _thread_id: &str,
        _turn_id: &str,
        _message: &str,
    ) -> PlatformResult<Option<String>> {
        Ok(None)
    }
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
    async fn ensure_workspace(
        &self,
        principal: &Principal,
        project: &Project,
    ) -> PlatformResult<crate::PreparedWorkspace>;
}

#[async_trait]
pub trait IdentityProvider: Send + Sync {
    async fn authenticate(&self, authorization: Option<&str>) -> PlatformResult<Principal>;
}

pub fn opaque_id(prefix: &str) -> Id {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}
#[async_trait::async_trait]
pub trait AgentContextProvider: Send + Sync {
    async fn context(
        &self,
        principal: &crate::Principal,
        project: &str,
        agent: &str,
    ) -> crate::PlatformResult<String>;
}
