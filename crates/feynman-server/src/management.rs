use crate::{store::SqliteRepository, workspace::ProjectWorkspaceProvider};
use chrono::Utc;
use feynman_core::{
    CreateScheduleRequest, CreateTaskRequest, PlatformError, PlatformResult, PlatformService,
    Principal, ProjectSchedule, ProjectTask, ProjectWorkspace, WorkspaceKind,
};
use serde_json::{Value, json};
use std::sync::Arc;
mod automation;
mod events;
mod feed;

/// Local manager: one durable Main Agent, one execution binding and one work queue per Project.
pub struct ProjectManager {
    repository: Arc<SqliteRepository>,
    platform: Arc<PlatformService>,
    workspace: Arc<ProjectWorkspaceProvider>,
    access: Option<Arc<crate::external_access::ExternalAccessBroker>>,
    event_jobs: tokio::sync::Mutex<std::collections::HashMap<String, events::ListenerJob>>,
}

impl ProjectManager {
    pub async fn seed_workspace(
        &self,
        principal: &feynman_core::Principal,
        project: &str,
        input: feynman_local::evaluation::SeedInput,
    ) -> feynman_core::PlatformResult<serde_json::Value> {
        self.workspace.seed(principal, project, input).await
    }
    pub fn new(
        repository: Arc<SqliteRepository>,
        platform: Arc<PlatformService>,
        workspace: Arc<ProjectWorkspaceProvider>,
    ) -> Self {
        Self {
            repository,
            platform,
            workspace,
            access: None,
            event_jobs: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
    pub fn with_access(
        mut self,
        access: Arc<crate::external_access::ExternalAccessBroker>,
    ) -> Self {
        self.access = Some(access);
        self
    }

    pub fn status(&self, principal: &Principal, project_id: &str) -> PlatformResult<Value> {
        let project = self.platform.get_project(principal, project_id)?;
        let conversation = if project.main_agent_id.is_empty() {
            None
        } else {
            Some(self.platform.get_conversation(principal, project_id)?)
        };
        let tasks = self.repository.list_tasks(principal, project_id)?;
        let schedules = self.repository.list_schedules(principal, project_id)?;
        let next = schedules
            .iter()
            .filter(|s| s.enabled && s.source_ids.is_empty())
            .map(|s| s.next_due_at)
            .min();
        Ok(
            json!({"project":project,"main_agent":conversation.as_ref().map(|c|json!({"id":project.main_agent_id,"role":"main","conversation_id":c.conversation.id,
            "runtime_thread_id":c.conversation.runtime_thread_id,"active_run":c.active_run,"latest_run":c.runs.last()})),
            "agents":self.repository.agents(principal,project_id)?,"inbox":self.repository.inbox(principal,project_id)?,"sources":self.repository.sources(principal,project_id)?,
            "workspace":self.workspace.get(principal, project_id)?,
            "tasks":{"queued":tasks.iter().filter(|t| t.status == "queued").count(),"running":tasks.iter().filter(|t| t.status == "running").count(),"total":tasks.len()},
            "schedules":{"enabled":schedules.iter().filter(|s| s.enabled).count(),"next_due_at":next},
            "reviewer":{"enabled":false,"required":false}}),
        )
    }

    pub fn overview(&self, principal: &Principal) -> PlatformResult<Value> {
        let projects = self
            .platform
            .list_projects(principal)?
            .iter()
            .map(|p| self.status(principal, &p.id))
            .collect::<PlatformResult<Vec<_>>>()?;
        Ok(
            json!({"mode":"local-first","projects":projects,"scheduler":{"requires_server_running":true,"missed_occurrences":"coalesce","overlap":"one_active_run_per_project"}}),
        )
    }

    pub fn tasks(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<Vec<ProjectTask>> {
        self.repository.list_tasks(principal, project_id)
    }
    pub fn task(
        &self,
        principal: &Principal,
        project_id: &str,
        id: &str,
    ) -> PlatformResult<ProjectTask> {
        self.repository.task(principal, project_id, id)
    }
    pub fn create_task(
        &self,
        principal: &Principal,
        project_id: &str,
        request: CreateTaskRequest,
    ) -> PlatformResult<ProjectTask> {
        self.repository.create_task(principal, project_id, request)
    }
    pub fn cancel_task(
        &self,
        principal: &Principal,
        project_id: &str,
        id: &str,
    ) -> PlatformResult<ProjectTask> {
        self.repository.cancel_task(principal, project_id, id)
    }
    pub fn schedules(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<Vec<ProjectSchedule>> {
        self.repository.list_schedules(principal, project_id)
    }
    pub fn create_schedule(
        &self,
        principal: &Principal,
        project_id: &str,
        request: CreateScheduleRequest,
    ) -> PlatformResult<ProjectSchedule> {
        if request.enabled {
            for id in &request.source_ids {
                self.source_permission(
                    principal,
                    &self.repository.source(principal, project_id, id)?,
                )?;
            }
        }
        self.repository
            .create_schedule(principal, project_id, request)
    }
    pub fn enable_schedule(
        &self,
        principal: &Principal,
        project_id: &str,
        id: &str,
        enabled: bool,
    ) -> PlatformResult<ProjectSchedule> {
        if enabled {
            let schedule = self
                .schedules(principal, project_id)?
                .into_iter()
                .find(|s| s.id == id)
                .ok_or(PlatformError::NotFound)?;
            for source_id in &schedule.source_ids {
                self.source_permission(
                    principal,
                    &self.repository.source(principal, project_id, source_id)?,
                )?;
            }
        }
        self.repository
            .set_schedule_enabled(principal, project_id, id, enabled)
    }
    pub fn workspace(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<Option<ProjectWorkspace>> {
        self.workspace.get(principal, project_id)
    }
    pub async fn create_workspace(
        &self,
        principal: &Principal,
        project_id: &str,
        provider: WorkspaceKind,
    ) -> PlatformResult<ProjectWorkspace> {
        self.workspace.create(principal, project_id, provider).await
    }
    pub async fn workspace_action(
        &self,
        principal: &Principal,
        project_id: &str,
        action: &str,
    ) -> PlatformResult<ProjectWorkspace> {
        self.workspace.action(principal, project_id, action).await
    }

    pub async fn tick(&self) -> PlatformResult<()> {
        let owner = self.platform.authenticate(None).await?;
        let mut allowed = std::collections::HashSet::new();
        for project in self.platform.list_projects(&owner)? {
            for source in self.sources(&owner, &project.id)? {
                if source.enabled && self.source_permission(&owner, &source).is_ok() {
                    allowed.insert(source.id);
                }
            }
        }
        self.repository
            .enqueue_authorized_schedule_inputs(Utc::now(), &allowed)?;
        self.dispatch_inbox().await?;
        let mut jobs = tokio::task::JoinSet::new();
        for (principal, task) in self.repository.queued_tasks()? {
            if self
                .workspace
                .get(&principal, &task.project_id)?
                .is_some_and(|w| w.state == "paused" || w.state == "pausing")
            {
                continue;
            }
            let platform = self.platform.clone();
            jobs.spawn(async move {
                match platform.send_task(&principal, &task).await {
                    Ok(_)
                    | Err(PlatformError::RunAlreadyActive)
                    | Err(PlatformError::Conflict(_)) => {}
                    // Failed starts already have a persisted terminal Run/task. Never blindly replay side effects.
                    Err(error) => {
                        tracing::warn!(%error, task_id = task.id, "project task dispatch failed")
                    }
                }
            });
        }
        while let Some(result) = jobs.join_next().await {
            result.map_err(|error| {
                PlatformError::Runtime(format!("task dispatcher stopped: {error}"))
            })?;
        }
        Ok(())
    }

    pub fn start_scheduler(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            let mut maintenance = tokio::time::interval(std::time::Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut collectors = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        this.sync_events().await;
                        if collectors.is_empty() {let manager=this.clone();collectors.spawn(async move{manager.poll_sources().await});}
                        if let Err(error) = this.tick().await { tracing::error!(%error, "project scheduler tick failed"); }
                    },
                    result=collectors.join_next(),if !collectors.is_empty()=>if let Some(Err(error))=result {tracing::warn!(%error,"source collector stopped")},
                    _ = maintenance.tick() => {
                        // Local dev identity is the sole authenticated owner. A remote
                        // identity adapter must supply an explicit background principal.
                        if let Ok(principal) = this.platform.authenticate(None).await
                            && let Ok(projects) = this.platform.list_projects(&principal) {
                            for project in projects {
                                if this.platform.get_conversation(&principal, &project.id).is_ok_and(|s| s.active_run.is_some())
                                    && let Err(error) = this.workspace.renew_active(&principal, &project.id).await { tracing::warn!(%error, project_id = project.id, "active Workspace renewal failed"); }
                            }
                        }
                    }
                }
            }
        })
    }
}
