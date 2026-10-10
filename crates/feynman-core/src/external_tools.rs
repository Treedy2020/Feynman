use crate::{
    PlatformError, PlatformRepository, PlatformResult, Principal, ProjectToolAccess, ToolGrant,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct ExternalToolDefinition {
    pub id: String,
    pub mcp_name: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
}

#[derive(Clone, Debug)]
pub struct ExternalToolContext {
    pub principal: Principal,
    pub project_id: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ExternalToolResult {
    pub source: String,
    pub observed_at: DateTime<Utc>,
    pub data: Value,
}

#[async_trait]
pub trait ExternalToolAdapter: Send + Sync {
    fn definitions(&self) -> Vec<ExternalToolDefinition>;

    async fn invoke(
        &self,
        context: &ExternalToolContext,
        tool_id: &str,
        arguments: Value,
    ) -> PlatformResult<ExternalToolResult>;
}

pub struct ExternalToolService {
    repository: Arc<dyn PlatformRepository>,
    adapters: Vec<Arc<dyn ExternalToolAdapter>>,
    definitions: Vec<ExternalToolDefinition>,
}

impl ExternalToolService {
    pub fn new(
        repository: Arc<dyn PlatformRepository>,
        adapters: Vec<Arc<dyn ExternalToolAdapter>>,
    ) -> PlatformResult<Self> {
        let mut definitions = Vec::new();
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for adapter in &adapters {
            for definition in adapter.definitions() {
                if definition.id.trim().is_empty()
                    || definition.mcp_name.trim().is_empty()
                    || !ids.insert(definition.id.clone())
                    || !names.insert(definition.mcp_name.clone())
                {
                    return Err(PlatformError::InvalidInput(
                        "external tool definitions must have unique non-empty IDs and MCP names"
                            .into(),
                    ));
                }
                definitions.push(definition);
            }
        }
        Ok(Self {
            repository,
            adapters,
            definitions,
        })
    }

    pub fn list_for_project(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<Vec<ProjectToolAccess>> {
        self.repository.get_project(principal, project_id)?;
        let grants = self.repository.list_tool_grants(principal, project_id)?;
        Ok(self
            .definitions
            .iter()
            .map(|definition| {
                let grant = grants
                    .iter()
                    .find(|grant| grant.tool_id == definition.id)
                    .cloned();
                ProjectToolAccess {
                    id: definition.id.clone(),
                    name: definition.name.clone(),
                    description: definition.description.clone(),
                    input_schema: definition.input_schema.clone(),
                    granted: grant.is_some(),
                    grant,
                }
            })
            .collect())
    }

    pub fn grant(
        &self,
        principal: &Principal,
        project_id: &str,
        tool_id: &str,
    ) -> PlatformResult<ToolGrant> {
        self.repository.get_project(principal, project_id)?;
        self.ensure_known_tool(tool_id)?;
        self.repository.grant_tool(principal, project_id, tool_id)
    }

    pub fn revoke(
        &self,
        principal: &Principal,
        project_id: &str,
        tool_id: &str,
    ) -> PlatformResult<bool> {
        self.repository.get_project(principal, project_id)?;
        self.ensure_known_tool(tool_id)?;
        self.repository.revoke_tool(principal, project_id, tool_id)
    }

    pub async fn invoke(
        &self,
        principal: &Principal,
        project_id: &str,
        tool_id: &str,
        arguments: Value,
    ) -> PlatformResult<ExternalToolResult> {
        self.repository.get_project(principal, project_id)?;
        self.ensure_known_tool(tool_id)?;
        self.repository
            .check_agent_tool(principal, project_id, tool_id)?;
        let context = ExternalToolContext {
            principal: principal.clone(),
            project_id: project_id.to_owned(),
        };
        let adapter = self
            .adapters
            .iter()
            .find(|adapter| adapter.definitions().iter().any(|item| item.id == tool_id))
            .ok_or_else(|| PlatformError::InvalidInput("unknown external tool".into()))?;
        adapter.invoke(&context, tool_id, arguments).await
    }

    pub fn definition_by_mcp_name(&self, name: &str) -> Option<&ExternalToolDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.mcp_name == name)
    }

    pub fn definitions(&self) -> &[ExternalToolDefinition] {
        &self.definitions
    }

    fn ensure_known_tool(&self, tool_id: &str) -> PlatformResult<()> {
        if self
            .definitions
            .iter()
            .any(|definition| definition.id == tool_id)
        {
            Ok(())
        } else {
            Err(PlatformError::InvalidInput(format!(
                "unknown external tool '{tool_id}'"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ExternalToolAdapter, ExternalToolDefinition, ExternalToolService};
    use crate::{
        ExternalToolContext, ExternalToolResult, PlatformError, PlatformRepository, PlatformResult,
        Principal, Project, ToolGrant,
    };
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    struct FakeRepository {
        granted: Mutex<bool>,
    }

    impl PlatformRepository for FakeRepository {
        fn list_projects(&self, _: &Principal) -> PlatformResult<Vec<Project>> {
            Ok(vec![])
        }
        fn create_project(&self, _: &Principal, _: &str) -> PlatformResult<Project> {
            unreachable!()
        }
        fn get_project(&self, principal: &Principal, id: &str) -> PlatformResult<Project> {
            if principal.tenant_id != "tenant-a" || id != "project-a" {
                return Err(PlatformError::NotFound);
            }
            Ok(Project {
                id: id.into(),
                tenant_id: principal.tenant_id.clone(),
                name: "A".into(),
                main_agent_id: "agent-a".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })
        }
        fn list_tool_grants(&self, _: &Principal, _: &str) -> PlatformResult<Vec<ToolGrant>> {
            Ok(vec![])
        }
        fn is_tool_granted(&self, _: &Principal, _: &str, _: &str) -> PlatformResult<bool> {
            Ok(*self.granted.lock().unwrap())
        }
        fn grant_tool(
            &self,
            principal: &Principal,
            project: &str,
            tool: &str,
        ) -> PlatformResult<ToolGrant> {
            *self.granted.lock().unwrap() = true;
            Ok(ToolGrant {
                tenant_id: principal.tenant_id.clone(),
                project_id: project.into(),
                tool_id: tool.into(),
                granted_by_user_id: principal.user_id.clone(),
                created_at: chrono::Utc::now(),
            })
        }
        fn revoke_tool(&self, _: &Principal, _: &str, _: &str) -> PlatformResult<bool> {
            *self.granted.lock().unwrap() = false;
            Ok(true)
        }
        fn get_or_create_agent_session(
            &self,
            _: &Principal,
            _: &Project,
            _: &str,
        ) -> PlatformResult<crate::AgentSession> {
            unreachable!()
        }
        fn mark_agent_session_ready(&self, _: &Principal, _: &str) -> PlatformResult<()> {
            unreachable!()
        }
        fn record_agent_session_heartbeat(&self, _: &Principal, _: &str) -> PlatformResult<()> {
            unreachable!()
        }
        fn get_or_create_conversation(
            &self,
            _: &Principal,
            _: &Project,
        ) -> PlatformResult<crate::Conversation> {
            unreachable!()
        }
        fn conversation_snapshot(
            &self,
            _: &Principal,
            _: &str,
        ) -> PlatformResult<crate::ConversationSnapshot> {
            unreachable!()
        }
        fn submit_message(
            &self,
            _: &Principal,
            _: &Project,
            _: &crate::Conversation,
            _: &crate::AgentSession,
            _: &str,
            _: Option<&str>,
        ) -> PlatformResult<crate::SubmittedMessage> {
            unreachable!()
        }
        fn set_conversation_thread(&self, _: &Principal, _: &str, _: &str) -> PlatformResult<()> {
            unreachable!()
        }
        fn mark_run_started(&self, _: &Principal, _: &str, _: &str) -> PlatformResult<()> {
            unreachable!()
        }
        fn mark_run_waiting_approval(&self, _: &Principal, _: &str) -> PlatformResult<()> {
            unreachable!()
        }
        fn mark_run_resumed(&self, _: &Principal, _: &str) -> PlatformResult<()> {
            unreachable!()
        }
        fn append_run_event(
            &self,
            _: &Principal,
            _: &str,
            _: &str,
            _: Value,
        ) -> PlatformResult<crate::ChatEvent> {
            unreachable!()
        }
        fn append_assistant_text(&self, _: &Principal, _: &str, _: &str) -> PlatformResult<()> {
            unreachable!()
        }
        fn finish_run(
            &self,
            _: &Principal,
            _: &str,
            _: crate::AgentRunStatus,
            _: Option<&str>,
        ) -> PlatformResult<()> {
            unreachable!()
        }
        fn get_run_feed(
            &self,
            _: &Principal,
            _: &str,
            _: &str,
            _: i64,
        ) -> PlatformResult<crate::RunFeed> {
            unreachable!()
        }
    }

    struct FakeAdapter {
        invoked: Mutex<bool>,
    }

    #[async_trait]
    impl ExternalToolAdapter for FakeAdapter {
        fn definitions(&self) -> Vec<ExternalToolDefinition> {
            vec![ExternalToolDefinition {
                read_only: true,
                id: "dws.aisearch.enterprise".into(),
                mcp_name: "dws_search_enterprise".into(),
                name: "DWS enterprise search".into(),
                description: "Read-only search".into(),
                input_schema: json!({"type":"object"}),
            }]
        }
        async fn invoke(
            &self,
            _: &ExternalToolContext,
            _: &str,
            _: Value,
        ) -> PlatformResult<ExternalToolResult> {
            *self.invoked.lock().unwrap() = true;
            Ok(ExternalToolResult {
                source: "fake".into(),
                observed_at: chrono::Utc::now(),
                data: json!({"ok":true}),
            })
        }
    }

    #[tokio::test]
    async fn a_tool_invocation_requires_a_project_scoped_grant() {
        let repository = Arc::new(FakeRepository {
            granted: Mutex::new(false),
        });
        let adapter = Arc::new(FakeAdapter {
            invoked: Mutex::new(false),
        });
        let service = ExternalToolService::new(repository.clone(), vec![adapter.clone()]).unwrap();
        let principal = Principal {
            user_id: "user-a".into(),
            tenant_id: "tenant-a".into(),
        };

        let denied = service
            .invoke(
                &principal,
                "project-a",
                "dws.aisearch.enterprise",
                json!({}),
            )
            .await;
        assert!(matches!(denied, Err(PlatformError::ToolNotGranted(_))));
        assert!(!*adapter.invoked.lock().unwrap());

        service
            .grant(&principal, "project-a", "dws.aisearch.enterprise")
            .unwrap();
        let result = service
            .invoke(
                &principal,
                "project-a",
                "dws.aisearch.enterprise",
                json!({}),
            )
            .await
            .unwrap();
        assert_eq!(result.data, json!({"ok":true}));
        assert!(*adapter.invoked.lock().unwrap());
    }
}
