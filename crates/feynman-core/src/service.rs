use crate::{
    AgentRunStatus, AgentRuntime, AgentRuntimeEvent, AgentTurnInput, AgentTurnStarted,
    ConversationSnapshot, CreateProjectRequest, ExternalToolService, IdentityProvider,
    PlatformError, PlatformRepository, PlatformResult, Principal, Project, ProjectToolAccess,
    RunFeed, SendMessageResponse, ToolGrant, WorkspaceProvider,
};
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct PlatformService {
    repository: Arc<dyn PlatformRepository>,
    runtime: Arc<dyn AgentRuntime>,
    workspace_provider: Arc<dyn WorkspaceProvider>,
    identity_provider: Arc<dyn IdentityProvider>,
    external_tools: Arc<ExternalToolService>,
    event_tx: broadcast::Sender<crate::ChatEvent>,
}

impl PlatformService {
    pub fn new(
        repository: Arc<dyn PlatformRepository>,
        runtime: Arc<dyn AgentRuntime>,
        workspace_provider: Arc<dyn WorkspaceProvider>,
        identity_provider: Arc<dyn IdentityProvider>,
        external_tools: Arc<ExternalToolService>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(512);
        Self {
            repository,
            runtime,
            workspace_provider,
            identity_provider,
            external_tools,
            event_tx,
        }
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<crate::ChatEvent> {
        self.event_tx.subscribe()
    }

    pub async fn authenticate(&self, authorization: Option<&str>) -> PlatformResult<Principal> {
        self.identity_provider.authenticate(authorization).await
    }

    pub fn list_projects(&self, principal: &Principal) -> PlatformResult<Vec<Project>> {
        self.repository.list_projects(principal)
    }

    pub fn create_project(
        &self,
        principal: &Principal,
        request: CreateProjectRequest,
    ) -> PlatformResult<Project> {
        self.repository.create_project(principal, &request.name)
    }

    pub fn get_project(&self, principal: &Principal, project_id: &str) -> PlatformResult<Project> {
        self.repository.get_project(principal, project_id)
    }

    pub fn list_project_tools(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<Vec<ProjectToolAccess>> {
        self.external_tools.list_for_project(principal, project_id)
    }

    pub fn grant_project_tool(
        &self,
        principal: &Principal,
        project_id: &str,
        tool_id: &str,
    ) -> PlatformResult<ToolGrant> {
        self.external_tools.grant(principal, project_id, tool_id)
    }

    pub fn revoke_project_tool(
        &self,
        principal: &Principal,
        project_id: &str,
        tool_id: &str,
    ) -> PlatformResult<bool> {
        self.external_tools.revoke(principal, project_id, tool_id)
    }

    pub fn get_conversation(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<ConversationSnapshot> {
        self.repository.conversation_snapshot(principal, project_id)
    }

    pub async fn send_message(
        &self,
        principal: &Principal,
        project_id: &str,
        content: &str,
    ) -> PlatformResult<SendMessageResponse> {
        let project = self.repository.get_project(principal, project_id)?;
        let session = self
            .repository
            .get_or_create_agent_session(principal, &project, "codex")?;
        let conversation = self
            .repository
            .get_or_create_conversation(principal, &project)?;
        let submitted = self.repository.submit_message(
            principal,
            &project,
            &conversation,
            &session,
            content,
        )?;
        let workspace_path = match self.workspace_provider.ensure_workspace(&project).await {
            Ok(path) => path,
            Err(error) => {
                self.repository.finish_run(
                    principal,
                    &submitted.run.id,
                    AgentRunStatus::Failed,
                    Some(&error.to_string()),
                )?;
                let event = self.repository.append_run_event(
                    principal,
                    &submitted.run.id,
                    "run_failed",
                    serde_json::json!({"error":error.to_string()}),
                )?;
                let _ = self.event_tx.send(event);
                return Err(error);
            }
        };
        let input = AgentTurnInput {
            principal: principal.clone(),
            project: project.clone(),
            conversation: submitted.conversation.clone(),
            session: session.clone(),
            message: content.trim().to_owned(),
            workspace_path,
            instructions: format!(
                "You are the Main Agent for Feynman project '{}'. Keep project knowledge and actions within this project. Distinguish observed facts, hypotheses, and unknowns.\nProject ID: {}",
                project.name, project.id
            ),
        };
        let (started, events) = match self.runtime.start_turn(input).await {
            Ok(started) => started,
            Err(error) => {
                self.repository.finish_run(
                    principal,
                    &submitted.run.id,
                    AgentRunStatus::Failed,
                    Some(&error.to_string()),
                )?;
                let event = self.repository.append_run_event(
                    principal,
                    &submitted.run.id,
                    "run_failed",
                    serde_json::json!({"error": error.to_string()}),
                )?;
                let _ = self.event_tx.send(event);
                return Err(error);
            }
        };
        self.mark_started_and_emit(
            principal,
            &submitted.run.id,
            &submitted.conversation.id,
            &started,
        )?;
        let this = self.clone();
        let actor = principal.clone();
        let run_id = submitted.run.id.clone();
        let conversation_id = submitted.conversation.id.clone();
        tokio::spawn(async move {
            this.consume_turn(actor, run_id, conversation_id, started, events)
                .await;
        });
        Ok(SendMessageResponse {
            conversation: submitted.conversation,
            user_message: submitted.user_message,
            run: submitted.run,
        })
    }

    pub fn get_run_feed(
        &self,
        principal: &Principal,
        project_id: &str,
        run_id: &str,
        after_sequence: i64,
    ) -> PlatformResult<RunFeed> {
        if after_sequence < 0 {
            return Err(PlatformError::InvalidInput(
                "after must be non-negative".into(),
            ));
        }
        self.repository
            .get_run_feed(principal, project_id, run_id, after_sequence)
    }

    pub async fn respond_to_approval(
        &self,
        principal: &Principal,
        project_id: &str,
        run_id: &str,
        request_id: &str,
        decision: &str,
    ) -> PlatformResult<()> {
        let project = self.repository.get_project(principal, project_id)?;
        self.repository
            .get_run_feed(principal, project_id, run_id, 0)?;
        if !matches!(
            decision,
            "accept" | "acceptForSession" | "decline" | "cancel"
        ) {
            return Err(PlatformError::InvalidInput(
                "unsupported approval decision".into(),
            ));
        }
        self.runtime
            .respond_to_approval(&project.id, request_id, decision)
            .await?;
        self.repository.mark_run_resumed(principal, run_id)?;
        self.emit(
            principal,
            run_id,
            "approval_resolved",
            serde_json::json!({"requestId": request_id, "decision": decision}),
        )
    }

    async fn consume_turn(
        &self,
        principal: Principal,
        run_id: String,
        conversation_id: String,
        started: AgentTurnStarted,
        mut events: tokio::sync::mpsc::Receiver<AgentRuntimeEvent>,
    ) {
        while let Some(event) = events.recv().await {
            let result = match event {
                AgentRuntimeEvent::TextDelta { text } => {
                    if let Err(error) = self
                        .repository
                        .append_assistant_text(&principal, &run_id, &text)
                    {
                        Err(error)
                    } else {
                        self.emit(
                            &principal,
                            &run_id,
                            "agent_text_delta",
                            serde_json::json!({"text": text}),
                        )
                    }
                }
                AgentRuntimeEvent::ToolActivity {
                    status,
                    tool,
                    message,
                } => self.emit(
                    &principal,
                    &run_id,
                    "tool_activity",
                    serde_json::json!({"status": status, "tool": tool, "message": message}),
                ),
                AgentRuntimeEvent::ApprovalRequested {
                    request_id,
                    kind,
                    summary,
                } => {
                    if let Err(error) = self
                        .repository
                        .mark_run_waiting_approval(&principal, &run_id)
                    {
                        Err(error)
                    } else {
                        self.emit(
                            &principal,
                            &run_id,
                            "approval_requested",
                            serde_json::json!({"requestId": request_id, "kind": kind, "summary": summary}),
                        )
                    }
                }
                AgentRuntimeEvent::Completed { status, error } => {
                    let terminal = if status.is_terminal() {
                        status
                    } else {
                        AgentRunStatus::Completed
                    };
                    let result = self.repository.finish_run(
                        &principal,
                        &run_id,
                        terminal.clone(),
                        error.as_deref(),
                    );
                    if result.is_ok() {
                        let kind = match terminal {
                            AgentRunStatus::Completed => "run_completed",
                            AgentRunStatus::Interrupted => "run_interrupted",
                            _ => "run_failed",
                        };
                        let payload = error.map_or_else(
                            || serde_json::json!({}),
                            |message| serde_json::json!({"error": message}),
                        );
                        if let Err(error) = self.emit(&principal, &run_id, kind, payload) {
                            tracing::error!(%error, "could not persist terminal Agent Run event");
                        }
                    } else if let Err(error) = result {
                        tracing::error!(%error, "could not finish Agent Run");
                    }
                    return;
                }
            };
            if let Err(error) = result {
                tracing::error!(%error, %run_id, "could not persist Agent event");
            }
        }
        let error = "Agent Runtime event stream closed before completion";
        let _ =
            self.repository
                .finish_run(&principal, &run_id, AgentRunStatus::Failed, Some(error));
        let _ = self.emit(
            &principal,
            &run_id,
            "run_failed",
            serde_json::json!({"error": error}),
        );
        let _ = conversation_id;
        let _ = started;
    }

    fn mark_started_and_emit(
        &self,
        principal: &Principal,
        run_id: &str,
        conversation_id: &str,
        started: &AgentTurnStarted,
    ) -> PlatformResult<()> {
        self.repository
            .mark_agent_session_ready(principal, &started.session_id)?;
        self.repository
            .set_conversation_thread(principal, conversation_id, &started.thread_id)?;
        self.repository
            .mark_run_started(principal, run_id, &started.turn_id)?;
        self.emit(
            principal,
            run_id,
            "run_started",
            serde_json::json!({"sessionId": started.session_id, "threadId": started.thread_id, "turnId": started.turn_id}),
        )?;
        Ok(())
    }

    fn emit(
        &self,
        principal: &Principal,
        run_id: &str,
        kind: &str,
        payload: serde_json::Value,
    ) -> PlatformResult<()> {
        let event = self
            .repository
            .append_run_event(principal, run_id, kind, payload)?;
        let _ = self.event_tx.send(event);
        Ok(())
    }
}
