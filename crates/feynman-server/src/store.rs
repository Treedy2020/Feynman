use chrono::{DateTime, Utc};
use feynman_core::{
    AgentRun, AgentRunStatus, AgentSession, AgentSessionStatus, ChatEvent, Conversation,
    ConversationMessage, ConversationSnapshot, MessageRole, MessageStatus, PlatformError,
    PlatformRepository, PlatformResult, Principal, Project, RunFeed, SubmittedMessage, ToolGrant,
    opaque_id,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;

pub struct SqliteRepository {
    connection: Mutex<Connection>,
}

impl SqliteRepository {
    pub fn open(path: impl AsRef<Path>) -> PlatformResult<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| PlatformError::Storage(error.to_string()))?;
        }
        let connection = Connection::open(path).map_err(storage_error)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(storage_error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(storage_error)?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS tenants (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                email TEXT,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tenant_memberships (
                tenant_id TEXT NOT NULL REFERENCES tenants(id),
                user_id TEXT NOT NULL REFERENCES users(id),
                role TEXT NOT NULL,
                PRIMARY KEY (tenant_id, user_id)
            );
            CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL REFERENCES tenants(id),
                name TEXT NOT NULL,
                main_agent_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE (tenant_id, name)
            );
            CREATE INDEX IF NOT EXISTS projects_by_tenant ON projects(tenant_id, created_at);
            CREATE TABLE IF NOT EXISTS tool_grants (
                tenant_id TEXT NOT NULL REFERENCES tenants(id),
                project_id TEXT NOT NULL REFERENCES projects(id),
                tool_id TEXT NOT NULL,
                granted_by_user_id TEXT NOT NULL REFERENCES users(id),
                created_at TEXT NOT NULL,
                PRIMARY KEY (tenant_id, project_id, tool_id)
            );
            CREATE INDEX IF NOT EXISTS tool_grants_by_project ON tool_grants(tenant_id, project_id);
            CREATE TABLE IF NOT EXISTS agent_sessions (
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL REFERENCES tenants(id),
                project_id TEXT NOT NULL REFERENCES projects(id),
                runtime TEXT NOT NULL,
                status TEXT NOT NULL,
                last_heartbeat_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE (tenant_id, project_id)
            );
            CREATE TABLE IF NOT EXISTS conversations (
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL REFERENCES tenants(id),
                project_id TEXT NOT NULL REFERENCES projects(id),
                main_agent_id TEXT NOT NULL,
                runtime_thread_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE (tenant_id, project_id)
            );
            CREATE TABLE IF NOT EXISTS conversation_messages (
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                conversation_id TEXT NOT NULL REFERENCES conversations(id),
                run_id TEXT,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                completed_at TEXT
            );
            CREATE INDEX IF NOT EXISTS messages_by_conversation ON conversation_messages(tenant_id, conversation_id, created_at);
            CREATE TABLE IF NOT EXISTS agent_runs (
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                project_id TEXT NOT NULL REFERENCES projects(id),
                conversation_id TEXT NOT NULL REFERENCES conversations(id),
                agent_session_id TEXT NOT NULL REFERENCES agent_sessions(id),
                user_message_id TEXT NOT NULL,
                assistant_message_id TEXT NOT NULL,
                status TEXT NOT NULL,
                runtime_turn_id TEXT,
                created_at TEXT NOT NULL,
                started_at TEXT,
                completed_at TEXT,
                error TEXT
            );
            CREATE INDEX IF NOT EXISTS runs_by_project ON agent_runs(tenant_id, project_id, created_at);
            CREATE TABLE IF NOT EXISTS chat_events (
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                run_id TEXT NOT NULL REFERENCES agent_runs(id),
                sequence INTEGER NOT NULL,
                kind TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE (run_id, sequence)
            );
            INSERT OR IGNORE INTO tenants(id, name, created_at) VALUES ('local', 'Local Development', strftime('%Y-%m-%dT%H:%M:%fZ','now'));
            INSERT OR IGNORE INTO users(id, email, created_at) VALUES ('local-user', NULL, strftime('%Y-%m-%dT%H:%M:%fZ','now'));
            INSERT OR IGNORE INTO tenant_memberships(tenant_id, user_id, role) VALUES ('local', 'local-user', 'owner');
            ",
        ).map_err(storage_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn connection(&self) -> PlatformResult<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|error| PlatformError::Storage(error.to_string()))
    }

    pub fn recover_after_server_restart(&self) -> PlatformResult<usize> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let active_runs = {
            let mut statement = transaction
                .prepare("SELECT id, tenant_id FROM agent_runs WHERE status IN ('queued', 'running', 'waiting_approval')")
                .map_err(storage_error)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(storage_error)?;
            rows.map(|row| row.map_err(storage_error))
                .collect::<PlatformResult<Vec<_>>>()?
        };
        let now = Utc::now().to_rfc3339();
        for (run_id, tenant_id) in &active_runs {
            transaction.execute(
                "UPDATE agent_runs SET status = 'interrupted', completed_at = ?, error = 'Feynman server restarted during this Agent Run' WHERE id = ? AND tenant_id = ?",
                params![now, run_id, tenant_id],
            ).map_err(storage_error)?;
            transaction.execute(
                "UPDATE conversation_messages SET status = 'failed', completed_at = ? WHERE tenant_id = ? AND run_id = ? AND role = 'assistant'",
                params![now, tenant_id, run_id],
            ).map_err(storage_error)?;
            let sequence: i64 = transaction
                .query_row(
                    "SELECT COALESCE(MAX(sequence), 0) + 1 FROM chat_events WHERE run_id = ?",
                    [run_id],
                    |row| row.get(0),
                )
                .map_err(storage_error)?;
            transaction.execute(
                "INSERT INTO chat_events (id, tenant_id, run_id, sequence, kind, payload_json, created_at) VALUES (?, ?, ?, ?, 'run_interrupted', ?, ?)",
                params![opaque_id("evt"), tenant_id, run_id, sequence, "{\"reason\":\"server_restart\"}", now],
            ).map_err(storage_error)?;
        }
        transaction.execute(
            "UPDATE agent_sessions SET status = 'degraded', updated_at = ? WHERE status IN ('ready', 'provisioning')",
            [&now],
        ).map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(active_runs.len())
    }
}

impl PlatformRepository for SqliteRepository {
    fn list_projects(&self, principal: &Principal) -> PlatformResult<Vec<Project>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT id, tenant_id, name, main_agent_id, created_at, updated_at FROM projects WHERE tenant_id = ? ORDER BY created_at")
            .map_err(storage_error)?;
        let rows = statement
            .query_map([&principal.tenant_id], project_from_row)
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(storage_error)).collect()
    }

    fn create_project(&self, principal: &Principal, name: &str) -> PlatformResult<Project> {
        let name = name.trim();
        if name.is_empty() {
            return Err(PlatformError::InvalidInput(
                "project name cannot be empty".into(),
            ));
        }
        let connection = self.connection()?;
        let now = Utc::now();
        let project = Project {
            id: opaque_id("prj"),
            tenant_id: principal.tenant_id.clone(),
            name: name.to_owned(),
            main_agent_id: opaque_id("agt"),
            created_at: now,
            updated_at: now,
        };
        connection.execute(
            "INSERT INTO projects (id, tenant_id, name, main_agent_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
            params![project.id, project.tenant_id, project.name, project.main_agent_id, project.created_at.to_rfc3339(), project.updated_at.to_rfc3339()],
        ).map_err(map_constraint_error)?;
        Ok(project)
    }

    fn get_project(&self, principal: &Principal, project_id: &str) -> PlatformResult<Project> {
        let connection = self.connection()?;
        connection.query_row(
            "SELECT id, tenant_id, name, main_agent_id, created_at, updated_at FROM projects WHERE tenant_id = ? AND id = ?",
            params![principal.tenant_id, project_id],
            project_from_row,
        ).optional().map_err(storage_error)?.ok_or(PlatformError::NotFound)
    }

    fn list_tool_grants(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<Vec<ToolGrant>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT tenant_id, project_id, tool_id, granted_by_user_id, created_at FROM tool_grants WHERE tenant_id = ? AND project_id = ? ORDER BY tool_id")
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![principal.tenant_id, project_id],
                tool_grant_from_row,
            )
            .map_err(storage_error)?;
        rows.map(|row| row.map_err(storage_error)).collect()
    }

    fn is_tool_granted(
        &self,
        principal: &Principal,
        project_id: &str,
        tool_id: &str,
    ) -> PlatformResult<bool> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM tool_grants WHERE tenant_id = ? AND project_id = ? AND tool_id = ?)",
                params![principal.tenant_id, project_id, tool_id],
                |row| row.get(0),
            )
            .map_err(storage_error)
    }

    fn grant_tool(
        &self,
        principal: &Principal,
        project_id: &str,
        tool_id: &str,
    ) -> PlatformResult<ToolGrant> {
        self.get_project(principal, project_id)?;
        let connection = self.connection()?;
        let now = Utc::now();
        connection
            .execute(
                "INSERT OR IGNORE INTO tool_grants (tenant_id, project_id, tool_id, granted_by_user_id, created_at) VALUES (?, ?, ?, ?, ?)",
                params![principal.tenant_id, project_id, tool_id, principal.user_id, now.to_rfc3339()],
            )
            .map_err(storage_error)?;
        connection
            .query_row(
                "SELECT tenant_id, project_id, tool_id, granted_by_user_id, created_at FROM tool_grants WHERE tenant_id = ? AND project_id = ? AND tool_id = ?",
                params![principal.tenant_id, project_id, tool_id],
                tool_grant_from_row,
            )
            .map_err(storage_error)
    }

    fn revoke_tool(
        &self,
        principal: &Principal,
        project_id: &str,
        tool_id: &str,
    ) -> PlatformResult<bool> {
        self.get_project(principal, project_id)?;
        let connection = self.connection()?;
        connection
            .execute(
                "DELETE FROM tool_grants WHERE tenant_id = ? AND project_id = ? AND tool_id = ?",
                params![principal.tenant_id, project_id, tool_id],
            )
            .map(|changed| changed > 0)
            .map_err(storage_error)
    }

    fn get_or_create_agent_session(
        &self,
        principal: &Principal,
        project: &Project,
        runtime: &str,
    ) -> PlatformResult<AgentSession> {
        let connection = self.connection()?;
        if let Some(session) = connection.query_row(
            "SELECT id, tenant_id, project_id, runtime, status, last_heartbeat_at, created_at, updated_at FROM agent_sessions WHERE tenant_id = ? AND project_id = ?",
            params![principal.tenant_id, project.id],
            session_from_row,
        ).optional().map_err(storage_error)? {
            return Ok(session);
        }
        let now = Utc::now();
        let session = AgentSession {
            id: opaque_id("ses"),
            tenant_id: principal.tenant_id.clone(),
            project_id: project.id.clone(),
            runtime: runtime.to_owned(),
            status: AgentSessionStatus::Provisioning,
            last_heartbeat_at: None,
            created_at: now,
            updated_at: now,
        };
        connection.execute(
            "INSERT INTO agent_sessions (id, tenant_id, project_id, runtime, status, last_heartbeat_at, created_at, updated_at) VALUES (?, ?, ?, ?, ?, NULL, ?, ?)",
            params![session.id, session.tenant_id, session.project_id, session.runtime, session_status_str(&session.status), now.to_rfc3339(), now.to_rfc3339()],
        ).map_err(storage_error)?;
        Ok(session)
    }

    fn mark_agent_session_ready(
        &self,
        principal: &Principal,
        session_id: &str,
    ) -> PlatformResult<()> {
        let connection = self.connection()?;
        let changed = connection.execute(
            "UPDATE agent_sessions SET status = 'ready', updated_at = ? WHERE tenant_id = ? AND id = ?",
            params![Utc::now().to_rfc3339(), principal.tenant_id, session_id],
        ).map_err(storage_error)?;
        if changed == 0 {
            return Err(PlatformError::NotFound);
        }
        Ok(())
    }

    fn record_agent_session_heartbeat(
        &self,
        principal: &Principal,
        session_id: &str,
    ) -> PlatformResult<()> {
        let connection = self.connection()?;
        let now = Utc::now().to_rfc3339();
        let changed = connection.execute(
            "UPDATE agent_sessions SET last_heartbeat_at = ?, updated_at = ? WHERE tenant_id = ? AND id = ?",
            params![now, now, principal.tenant_id, session_id],
        ).map_err(storage_error)?;
        if changed == 0 {
            return Err(PlatformError::NotFound);
        }
        Ok(())
    }

    fn get_or_create_conversation(
        &self,
        principal: &Principal,
        project: &Project,
    ) -> PlatformResult<Conversation> {
        let connection = self.connection()?;
        if let Some(conversation) = connection.query_row(
            "SELECT id, tenant_id, project_id, main_agent_id, runtime_thread_id, created_at, updated_at FROM conversations WHERE tenant_id = ? AND project_id = ?",
            params![principal.tenant_id, project.id],
            conversation_from_row,
        ).optional().map_err(storage_error)? {
            return Ok(conversation);
        }
        let now = Utc::now();
        let conversation = Conversation {
            id: opaque_id("cnv"),
            tenant_id: principal.tenant_id.clone(),
            project_id: project.id.clone(),
            main_agent_id: project.main_agent_id.clone(),
            runtime_thread_id: None,
            created_at: now,
            updated_at: now,
        };
        connection.execute(
            "INSERT INTO conversations (id, tenant_id, project_id, main_agent_id, runtime_thread_id, created_at, updated_at) VALUES (?, ?, ?, ?, NULL, ?, ?)",
            params![conversation.id, conversation.tenant_id, conversation.project_id, conversation.main_agent_id, now.to_rfc3339(), now.to_rfc3339()],
        ).map_err(storage_error)?;
        Ok(conversation)
    }

    fn conversation_snapshot(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<ConversationSnapshot> {
        let project = self.get_project(principal, project_id)?;
        let conversation = self.get_or_create_conversation(principal, &project)?;
        let connection = self.connection()?;
        let mut messages_stmt = connection.prepare(
            "SELECT id, tenant_id, conversation_id, run_id, role, content, status, created_at, completed_at FROM conversation_messages WHERE tenant_id = ? AND conversation_id = ? ORDER BY rowid",
        ).map_err(storage_error)?;
        let messages = messages_stmt
            .query_map(
                params![principal.tenant_id, conversation.id],
                message_from_row,
            )
            .map_err(storage_error)?
            .map(|row| row.map_err(storage_error))
            .collect::<PlatformResult<Vec<_>>>()?;
        let mut runs_stmt = connection.prepare(
            "SELECT id, tenant_id, project_id, conversation_id, agent_session_id, user_message_id, assistant_message_id, status, runtime_turn_id, created_at, started_at, completed_at, error FROM agent_runs WHERE tenant_id = ? AND conversation_id = ? ORDER BY rowid",
        ).map_err(storage_error)?;
        let runs = runs_stmt
            .query_map(params![principal.tenant_id, conversation.id], run_from_row)
            .map_err(storage_error)?
            .map(|row| row.map_err(storage_error))
            .collect::<PlatformResult<Vec<_>>>()?;
        let active_run = runs
            .iter()
            .rev()
            .find(|run| {
                matches!(
                    run.status,
                    AgentRunStatus::Queued
                        | AgentRunStatus::Running
                        | AgentRunStatus::WaitingApproval
                )
            })
            .cloned();
        Ok(ConversationSnapshot {
            conversation,
            messages,
            runs,
            active_run,
        })
    }

    fn submit_message(
        &self,
        principal: &Principal,
        project: &Project,
        conversation: &Conversation,
        session: &AgentSession,
        content: &str,
    ) -> PlatformResult<SubmittedMessage> {
        let content = content.trim();
        if content.is_empty() {
            return Err(PlatformError::InvalidInput(
                "message cannot be empty".into(),
            ));
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let active: Option<String> = transaction.query_row(
            "SELECT id FROM agent_runs WHERE tenant_id = ? AND project_id = ? AND status IN ('queued', 'running', 'waiting_approval') LIMIT 1",
            params![principal.tenant_id, project.id],
            |row| row.get(0),
        ).optional().map_err(storage_error)?;
        if active.is_some() {
            return Err(PlatformError::RunAlreadyActive);
        }
        let now = Utc::now();
        let user_message_id = opaque_id("msg");
        let assistant_message_id = opaque_id("msg");
        let run_id = opaque_id("run");
        transaction.execute(
            "INSERT INTO conversation_messages (id, tenant_id, conversation_id, run_id, role, content, status, created_at, completed_at) VALUES (?, ?, ?, NULL, 'user', ?, 'completed', ?, ?)",
            params![user_message_id, principal.tenant_id, conversation.id, content, now.to_rfc3339(), now.to_rfc3339()],
        ).map_err(storage_error)?;
        transaction.execute(
            "INSERT INTO conversation_messages (id, tenant_id, conversation_id, run_id, role, content, status, created_at, completed_at) VALUES (?, ?, ?, ?, 'assistant', '', 'streaming', ?, NULL)",
            params![assistant_message_id, principal.tenant_id, conversation.id, run_id, now.to_rfc3339()],
        ).map_err(storage_error)?;
        transaction.execute(
            "INSERT INTO agent_runs (id, tenant_id, project_id, conversation_id, agent_session_id, user_message_id, assistant_message_id, status, runtime_turn_id, created_at, started_at, completed_at, error) VALUES (?, ?, ?, ?, ?, ?, ?, 'queued', NULL, ?, NULL, NULL, NULL)",
            params![run_id, principal.tenant_id, project.id, conversation.id, session.id, user_message_id, assistant_message_id, now.to_rfc3339()],
        ).map_err(storage_error)?;
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ? WHERE tenant_id = ? AND id = ?",
                params![now.to_rfc3339(), principal.tenant_id, conversation.id],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(SubmittedMessage {
            conversation: Conversation {
                updated_at: now,
                ..conversation.clone()
            },
            user_message: ConversationMessage {
                id: user_message_id.clone(),
                tenant_id: principal.tenant_id.clone(),
                conversation_id: conversation.id.clone(),
                run_id: None,
                role: MessageRole::User,
                content: content.to_owned(),
                status: MessageStatus::Completed,
                created_at: now,
                completed_at: Some(now),
            },
            run: AgentRun {
                id: run_id,
                tenant_id: principal.tenant_id.clone(),
                project_id: project.id.clone(),
                conversation_id: conversation.id.clone(),
                agent_session_id: session.id.clone(),
                user_message_id,
                assistant_message_id,
                status: AgentRunStatus::Queued,
                runtime_turn_id: None,
                created_at: now,
                started_at: None,
                completed_at: None,
                error: None,
            },
        })
    }

    fn set_conversation_thread(
        &self,
        principal: &Principal,
        conversation_id: &str,
        thread_id: &str,
    ) -> PlatformResult<()> {
        let connection = self.connection()?;
        let changed = connection.execute(
            "UPDATE conversations SET runtime_thread_id = ?, updated_at = ? WHERE tenant_id = ? AND id = ?",
            params![thread_id, Utc::now().to_rfc3339(), principal.tenant_id, conversation_id],
        ).map_err(storage_error)?;
        if changed == 0 {
            return Err(PlatformError::NotFound);
        }
        Ok(())
    }

    fn mark_run_started(
        &self,
        principal: &Principal,
        run_id: &str,
        turn_id: &str,
    ) -> PlatformResult<()> {
        let connection = self.connection()?;
        let now = Utc::now().to_rfc3339();
        let changed = connection.execute(
            "UPDATE agent_runs SET status = 'running', runtime_turn_id = ?, started_at = ? WHERE tenant_id = ? AND id = ? AND status = 'queued'",
            params![turn_id, now, principal.tenant_id, run_id],
        ).map_err(storage_error)?;
        if changed == 0 {
            return Err(PlatformError::NotFound);
        }
        Ok(())
    }

    fn mark_run_waiting_approval(&self, principal: &Principal, run_id: &str) -> PlatformResult<()> {
        let connection = self.connection()?;
        let changed = connection.execute(
            "UPDATE agent_runs SET status = 'waiting_approval' WHERE tenant_id = ? AND id = ? AND status = 'running'",
            params![principal.tenant_id, run_id],
        ).map_err(storage_error)?;
        if changed == 0 {
            return Err(PlatformError::NotFound);
        }
        Ok(())
    }

    fn mark_run_resumed(&self, principal: &Principal, run_id: &str) -> PlatformResult<()> {
        let connection = self.connection()?;
        let changed = connection.execute(
            "UPDATE agent_runs SET status = 'running' WHERE tenant_id = ? AND id = ? AND status = 'waiting_approval'",
            params![principal.tenant_id, run_id],
        ).map_err(storage_error)?;
        if changed == 0 {
            return Err(PlatformError::NotFound);
        }
        Ok(())
    }

    fn append_run_event(
        &self,
        principal: &Principal,
        run_id: &str,
        kind: &str,
        payload: Value,
    ) -> PlatformResult<ChatEvent> {
        let connection = self.connection()?;
        let run_exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_runs WHERE tenant_id = ? AND id = ?)",
                params![principal.tenant_id, run_id],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        if !run_exists {
            return Err(PlatformError::NotFound);
        }
        let next: i64 = connection
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) + 1 FROM chat_events WHERE run_id = ?",
                [run_id],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        let event = ChatEvent {
            id: opaque_id("evt"),
            tenant_id: principal.tenant_id.clone(),
            run_id: run_id.to_owned(),
            sequence: next,
            kind: kind.to_owned(),
            payload,
            created_at: Utc::now(),
        };
        connection.execute(
            "INSERT INTO chat_events (id, tenant_id, run_id, sequence, kind, payload_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![event.id, event.tenant_id, event.run_id, event.sequence, event.kind, serde_json::to_string(&event.payload).map_err(storage_error)?, event.created_at.to_rfc3339()],
        ).map_err(storage_error)?;
        Ok(event)
    }

    fn append_assistant_text(
        &self,
        principal: &Principal,
        run_id: &str,
        text: &str,
    ) -> PlatformResult<()> {
        let connection = self.connection()?;
        let changed = connection.execute(
            "UPDATE conversation_messages SET content = content || ? WHERE tenant_id = ? AND run_id = ? AND role = 'assistant'",
            params![text, principal.tenant_id, run_id],
        ).map_err(storage_error)?;
        if changed == 0 {
            return Err(PlatformError::NotFound);
        }
        Ok(())
    }

    fn finish_run(
        &self,
        principal: &Principal,
        run_id: &str,
        status: AgentRunStatus,
        error: Option<&str>,
    ) -> PlatformResult<()> {
        let connection = self.connection()?;
        let now = Utc::now().to_rfc3339();
        let status_text = run_status_str(&status);
        let changed = connection.execute(
            "UPDATE agent_runs SET status = ?, completed_at = ?, error = ? WHERE tenant_id = ? AND id = ? AND status IN ('queued', 'running', 'waiting_approval')",
            params![status_text, now, error, principal.tenant_id, run_id],
        ).map_err(storage_error)?;
        if changed == 0 {
            return Err(PlatformError::NotFound);
        }
        let message_status = if matches!(status, AgentRunStatus::Completed) {
            "completed"
        } else {
            "failed"
        };
        connection.execute(
            "UPDATE conversation_messages SET status = ?, completed_at = ? WHERE tenant_id = ? AND run_id = ? AND role = 'assistant'",
            params![message_status, now, principal.tenant_id, run_id],
        ).map_err(storage_error)?;
        Ok(())
    }

    fn get_run_feed(
        &self,
        principal: &Principal,
        project_id: &str,
        run_id: &str,
        after_sequence: i64,
    ) -> PlatformResult<RunFeed> {
        let connection = self.connection()?;
        let run = connection.query_row(
            "SELECT id, tenant_id, project_id, conversation_id, agent_session_id, user_message_id, assistant_message_id, status, runtime_turn_id, created_at, started_at, completed_at, error FROM agent_runs WHERE tenant_id = ? AND project_id = ? AND id = ?",
            params![principal.tenant_id, project_id, run_id],
            run_from_row,
        ).optional().map_err(storage_error)?.ok_or(PlatformError::NotFound)?;
        let mut statement = connection.prepare(
            "SELECT id, tenant_id, run_id, sequence, kind, payload_json, created_at FROM chat_events WHERE tenant_id = ? AND run_id = ? AND sequence > ? ORDER BY sequence",
        ).map_err(storage_error)?;
        let events = statement
            .query_map(
                params![principal.tenant_id, run_id, after_sequence],
                event_from_row,
            )
            .map_err(storage_error)?
            .map(|row| row.map_err(storage_error))
            .collect::<PlatformResult<Vec<_>>>()?;
        Ok(RunFeed { run, events })
    }
}

fn tool_grant_from_row(row: &Row<'_>) -> rusqlite::Result<ToolGrant> {
    Ok(ToolGrant {
        tenant_id: row.get(0)?,
        project_id: row.get(1)?,
        tool_id: row.get(2)?,
        granted_by_user_id: row.get(3)?,
        created_at: parse_time(row.get::<_, String>(4)?)?,
    })
}

fn project_from_row(row: &Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        name: row.get(2)?,
        main_agent_id: row.get(3)?,
        created_at: parse_time(row.get::<_, String>(4)?)?,
        updated_at: parse_time(row.get::<_, String>(5)?)?,
    })
}

fn session_from_row(row: &Row<'_>) -> rusqlite::Result<AgentSession> {
    let heartbeat: Option<String> = row.get(5)?;
    Ok(AgentSession {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        project_id: row.get(2)?,
        runtime: row.get(3)?,
        status: parse_session_status(row.get::<_, String>(4)?.as_str())?,
        last_heartbeat_at: heartbeat.map(parse_time).transpose()?,
        created_at: parse_time(row.get::<_, String>(6)?)?,
        updated_at: parse_time(row.get::<_, String>(7)?)?,
    })
}

fn conversation_from_row(row: &Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        project_id: row.get(2)?,
        main_agent_id: row.get(3)?,
        runtime_thread_id: row.get(4)?,
        created_at: parse_time(row.get::<_, String>(5)?)?,
        updated_at: parse_time(row.get::<_, String>(6)?)?,
    })
}

fn message_from_row(row: &Row<'_>) -> rusqlite::Result<ConversationMessage> {
    let completed: Option<String> = row.get(8)?;
    Ok(ConversationMessage {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        conversation_id: row.get(2)?,
        run_id: row.get(3)?,
        role: parse_role(row.get::<_, String>(4)?.as_str())?,
        content: row.get(5)?,
        status: parse_message_status(row.get::<_, String>(6)?.as_str())?,
        created_at: parse_time(row.get::<_, String>(7)?)?,
        completed_at: completed.map(parse_time).transpose()?,
    })
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<AgentRun> {
    let started: Option<String> = row.get(10)?;
    let completed: Option<String> = row.get(11)?;
    Ok(AgentRun {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        project_id: row.get(2)?,
        conversation_id: row.get(3)?,
        agent_session_id: row.get(4)?,
        user_message_id: row.get(5)?,
        assistant_message_id: row.get(6)?,
        status: parse_run_status(row.get::<_, String>(7)?.as_str())?,
        runtime_turn_id: row.get(8)?,
        created_at: parse_time(row.get::<_, String>(9)?)?,
        started_at: started.map(parse_time).transpose()?,
        completed_at: completed.map(parse_time).transpose()?,
        error: row.get(12)?,
    })
}

fn event_from_row(row: &Row<'_>) -> rusqlite::Result<ChatEvent> {
    let payload: String = row.get(5)?;
    Ok(ChatEvent {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        run_id: row.get(2)?,
        sequence: row.get(3)?,
        kind: row.get(4)?,
        payload: serde_json::from_str(&payload).map_err(to_sql_conversion_error)?,
        created_at: parse_time(row.get::<_, String>(6)?)?,
    })
}

fn parse_time(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(to_sql_conversion_error)
}

fn parse_session_status(value: &str) -> rusqlite::Result<AgentSessionStatus> {
    match value {
        "provisioning" => Ok(AgentSessionStatus::Provisioning),
        "ready" => Ok(AgentSessionStatus::Ready),
        "degraded" => Ok(AgentSessionStatus::Degraded),
        "stopped" => Ok(AgentSessionStatus::Stopped),
        _ => Err(to_sql_conversion_error("unknown Agent Session status")),
    }
}
fn parse_role(value: &str) -> rusqlite::Result<MessageRole> {
    match value {
        "user" => Ok(MessageRole::User),
        "assistant" => Ok(MessageRole::Assistant),
        "system" => Ok(MessageRole::System),
        _ => Err(to_sql_conversion_error("unknown message role")),
    }
}
fn parse_message_status(value: &str) -> rusqlite::Result<MessageStatus> {
    match value {
        "streaming" => Ok(MessageStatus::Streaming),
        "completed" => Ok(MessageStatus::Completed),
        "failed" => Ok(MessageStatus::Failed),
        _ => Err(to_sql_conversion_error("unknown message status")),
    }
}
fn parse_run_status(value: &str) -> rusqlite::Result<AgentRunStatus> {
    match value {
        "queued" => Ok(AgentRunStatus::Queued),
        "running" => Ok(AgentRunStatus::Running),
        "waiting_approval" => Ok(AgentRunStatus::WaitingApproval),
        "completed" => Ok(AgentRunStatus::Completed),
        "failed" => Ok(AgentRunStatus::Failed),
        "interrupted" => Ok(AgentRunStatus::Interrupted),
        _ => Err(to_sql_conversion_error("unknown Agent Run status")),
    }
}
fn session_status_str(value: &AgentSessionStatus) -> &'static str {
    match value {
        AgentSessionStatus::Provisioning => "provisioning",
        AgentSessionStatus::Ready => "ready",
        AgentSessionStatus::Degraded => "degraded",
        AgentSessionStatus::Stopped => "stopped",
    }
}
fn run_status_str(value: &AgentRunStatus) -> &'static str {
    match value {
        AgentRunStatus::Queued => "queued",
        AgentRunStatus::Running => "running",
        AgentRunStatus::WaitingApproval => "waiting_approval",
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::Failed => "failed",
        AgentRunStatus::Interrupted => "interrupted",
    }
}
fn to_sql_conversion_error(error: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::other(error.to_string())),
    )
}
fn storage_error(error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Storage(error.to_string())
}
fn map_constraint_error(error: rusqlite::Error) -> PlatformError {
    if matches!(error, rusqlite::Error::SqliteFailure(ref code, _) if code.code == rusqlite::ErrorCode::ConstraintViolation)
    {
        PlatformError::Conflict("project with this name already exists in this tenant".into())
    } else {
        storage_error(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temporary_database() -> PathBuf {
        std::env::temp_dir().join(format!("feynman-test-{}.sqlite3", uuid::Uuid::new_v4()))
    }

    #[test]
    fn projects_and_conversation_records_are_scoped_to_the_authenticated_tenant() {
        let path = temporary_database();
        let repository = SqliteRepository::open(&path).unwrap();
        let local = Principal {
            user_id: "local-user".into(),
            tenant_id: "local".into(),
        };
        let other = Principal {
            user_id: "other-user".into(),
            tenant_id: "other-tenant".into(),
        };
        let project = repository
            .create_project(&local, "CLI architecture")
            .unwrap();
        assert_eq!(repository.list_projects(&local).unwrap().len(), 1);
        assert!(matches!(
            repository.get_project(&other, &project.id),
            Err(PlatformError::NotFound)
        ));
        assert!(repository.list_projects(&other).unwrap().is_empty());

        let session = repository
            .get_or_create_agent_session(&local, &project, "codex")
            .unwrap();
        let conversation = repository
            .get_or_create_conversation(&local, &project)
            .unwrap();
        let submitted = repository
            .submit_message(&local, &project, &conversation, &session, "hello")
            .unwrap();
        repository
            .mark_run_started(&local, &submitted.run.id, "turn-1")
            .unwrap();
        repository
            .append_assistant_text(&local, &submitted.run.id, "hello from agent")
            .unwrap();
        repository
            .append_run_event(
                &local,
                &submitted.run.id,
                "agent_text_delta",
                serde_json::json!({"text":"hello from agent"}),
            )
            .unwrap();
        repository
            .finish_run(&local, &submitted.run.id, AgentRunStatus::Completed, None)
            .unwrap();
        let snapshot = repository
            .conversation_snapshot(&local, &project.id)
            .unwrap();
        assert_eq!(snapshot.messages.len(), 2);
        assert_eq!(snapshot.messages[1].content, "hello from agent");
        assert_eq!(snapshot.runs[0].status, AgentRunStatus::Completed);
        assert_eq!(
            repository
                .get_run_feed(&local, &project.id, &submitted.run.id, 0)
                .unwrap()
                .events
                .len(),
            1
        );
        drop(repository);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn project_tool_grants_are_persisted_idempotently_and_tenant_scoped() {
        let path = temporary_database();
        let repository = SqliteRepository::open(&path).unwrap();
        let local = Principal {
            user_id: "local-user".into(),
            tenant_id: "local".into(),
        };
        let other_tenant = Principal {
            user_id: "other-user".into(),
            tenant_id: "other-tenant".into(),
        };
        let project = repository.create_project(&local, "DWS grant").unwrap();

        let first = repository
            .grant_tool(&local, &project.id, "dws.aisearch.enterprise")
            .unwrap();
        let repeated = repository
            .grant_tool(&local, &project.id, "dws.aisearch.enterprise")
            .unwrap();
        assert_eq!(first, repeated);
        assert_eq!(
            repository.list_tool_grants(&local, &project.id).unwrap(),
            vec![first]
        );
        assert!(
            repository
                .is_tool_granted(&local, &project.id, "dws.aisearch.enterprise")
                .unwrap()
        );
        assert!(
            !repository
                .is_tool_granted(&other_tenant, &project.id, "dws.aisearch.enterprise")
                .unwrap()
        );

        assert!(
            repository
                .revoke_tool(&local, &project.id, "dws.aisearch.enterprise")
                .unwrap()
        );
        assert!(
            !repository
                .revoke_tool(&local, &project.id, "dws.aisearch.enterprise")
                .unwrap()
        );
        assert!(
            !repository
                .is_tool_granted(&local, &project.id, "dws.aisearch.enterprise")
                .unwrap()
        );

        drop(repository);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_project_accepts_only_one_active_run() {
        let path = temporary_database();
        let repository = SqliteRepository::open(&path).unwrap();
        let principal = Principal {
            user_id: "local-user".into(),
            tenant_id: "local".into(),
        };
        let project = repository
            .create_project(&principal, "single active run")
            .unwrap();
        let session = repository
            .get_or_create_agent_session(&principal, &project, "codex")
            .unwrap();
        let conversation = repository
            .get_or_create_conversation(&principal, &project)
            .unwrap();
        repository
            .submit_message(&principal, &project, &conversation, &session, "first")
            .unwrap();
        assert!(matches!(
            repository.submit_message(&principal, &project, &conversation, &session, "second"),
            Err(PlatformError::RunAlreadyActive)
        ));
        drop(repository);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn server_restart_closes_active_runs_but_preserves_the_conversation_thread() {
        let path = temporary_database();
        let repository = SqliteRepository::open(&path).unwrap();
        let principal = Principal {
            user_id: "local-user".into(),
            tenant_id: "local".into(),
        };
        let project = repository
            .create_project(&principal, "restart recovery")
            .unwrap();
        let session = repository
            .get_or_create_agent_session(&principal, &project, "codex")
            .unwrap();
        repository
            .mark_agent_session_ready(&principal, &session.id)
            .unwrap();
        let conversation = repository
            .get_or_create_conversation(&principal, &project)
            .unwrap();
        let submitted = repository
            .submit_message(
                &principal,
                &project,
                &conversation,
                &session,
                "continue later",
            )
            .unwrap();
        repository
            .set_conversation_thread(&principal, &conversation.id, "thread-persisted")
            .unwrap();

        assert_eq!(repository.recover_after_server_restart().unwrap(), 1);
        let snapshot = repository
            .conversation_snapshot(&principal, &project.id)
            .unwrap();
        assert_eq!(snapshot.runs[0].status, AgentRunStatus::Interrupted);
        assert_eq!(
            snapshot.conversation.runtime_thread_id.as_deref(),
            Some("thread-persisted")
        );
        let feed = repository
            .get_run_feed(&principal, &project.id, &submitted.run.id, 0)
            .unwrap();
        assert_eq!(feed.events.len(), 1);
        assert_eq!(feed.events[0].kind, "run_interrupted");
        drop(repository);
        let _ = std::fs::remove_file(path);
    }
}
