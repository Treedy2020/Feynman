use async_trait::async_trait;
use feynman_core::{PlatformError, PlatformResult, PreparedWorkspace, Project, WorkspaceProvider};
use std::path::PathBuf;

pub struct LocalWorkspaceProvider {
    root: PathBuf,
}

impl LocalWorkspaceProvider {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

#[async_trait]
impl WorkspaceProvider for LocalWorkspaceProvider {
    async fn ensure_workspace(
        &self,
        _principal: &Principal,
        project: &Project,
    ) -> PlatformResult<PreparedWorkspace> {
        let path = self.root.join(&project.tenant_id).join(&project.id);
        tokio::fs::create_dir_all(&path).await.map_err(|error| {
            PlatformError::Runtime(format!("create persistent local workspace: {error}"))
        })?;
        let absolute_path = tokio::fs::canonicalize(&path).await.map_err(|error| {
            PlatformError::Runtime(format!("resolve persistent local workspace: {error}"))
        })?;
        Ok(PreparedWorkspace {
            local_cwd: absolute_path.to_string_lossy().into_owned(),
            environment: None,
        })
    }
}

mod opensandbox;

use crate::store::SqliteRepository;
use chrono::Utc;
use feynman_core::{PlatformRepository, Principal, ProjectWorkspace, WorkspaceKind};
use opensandbox::OpenSandboxProvider;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Project-scoped lifecycle and transport preparation; remote execution never falls back locally.
pub struct ProjectWorkspaceProvider {
    repository: Arc<SqliteRepository>,
    local: LocalWorkspaceProvider,
    remote: Option<OpenSandboxProvider>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl ProjectWorkspaceProvider {
    pub async fn clone_repository(
        &self,
        p: &Principal,
        w: &ProjectWorkspace,
        payload: serde_json::Value,
    ) -> PlatformResult<serde_json::Value> {
        let project = self.repository.get_project(p, &w.project_id)?;
        let lock = self.lock(&project).await;
        let _guard = lock.lock().await;
        if !self.get(p, &w.project_id)?.is_some_and(|current| {
            current.environment_id == w.environment_id
                && current.sandbox_id == w.sandbox_id
                && current.provider == w.provider
                && current.volume_name == w.volume_name
                && current.state == w.state
        }) {
            return Err(PlatformError::Conflict(
                "Workspace changed before clone".into(),
            ));
        }
        self.remote()?
            .fixed_control(p, w, include_str!("workspace/clone.py"), payload)
            .await
    }
    pub fn workload_instance(&self, w: &ProjectWorkspace) -> PlatformResult<String> {
        if let Some(id) = &w.sandbox_id {
            return Ok(id.clone());
        }
        let metadata = std::fs::metadata(&w.cwd)
            .map_err(|_| PlatformError::Runtime("local Workspace missing".into()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(format!(
                "local:{}:{}:{}",
                w.environment_id,
                metadata.dev(),
                metadata.ino()
            ))
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            Err(PlatformError::InvalidInput(
                "managed workloads require Unix".into(),
            ))
        }
    }
    pub async fn workload_control(
        &self,
        p: &Principal,
        w: &ProjectWorkspace,
        action: &str,
        mut payload: serde_json::Value,
        root: &std::path::Path,
    ) -> PlatformResult<serde_json::Value> {
        if w.provider == WorkspaceKind::Opensandbox {
            return self.remote()?.workload_control(p, w, action, payload).await;
        }
        payload["root"] = serde_json::json!(root);
        payload["workspace"] = serde_json::json!(w.cwd);
        crate::resources::run_control(None, action, payload).await
    }
    pub fn new(repository: Arc<SqliteRepository>, root: PathBuf) -> PlatformResult<Self> {
        Ok(Self {
            repository,
            local: LocalWorkspaceProvider::new(root),
            remote: OpenSandboxProvider::from_env()?,
            locks: Mutex::new(HashMap::new()),
        })
    }

    async fn lock(&self, project: &Project) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .await
            .entry(format!("{}:{}", project.tenant_id, project.id))
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn remote(&self) -> PlatformResult<&OpenSandboxProvider> {
        self.remote.as_ref().ok_or_else(|| {
            PlatformError::InvalidInput(
                "OpenSandbox is not configured; set FEYNMAN_OPENSANDBOX_CONFIG on the local server"
                    .into(),
            )
        })
    }

    pub fn get(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<Option<ProjectWorkspace>> {
        self.repository.workspace(principal, project_id)
    }

    pub async fn create(
        &self,
        principal: &Principal,
        project_id: &str,
        kind: WorkspaceKind,
    ) -> PlatformResult<ProjectWorkspace> {
        let project = self.repository.get_project(principal, project_id)?;
        let lock = self.lock(&project).await;
        let _guard = lock.lock().await;
        self.assert_idle(principal, project_id)?;
        let mut workspace = if let Some(existing) = self.get(principal, project_id)? {
            if existing.provider != kind {
                return Err(PlatformError::Conflict("project already has a different Workspace; use a new Project rather than silently migrate files".into()));
            }
            existing
        } else {
            let local = self.local.ensure_workspace(principal, &project).await?;
            let remote = kind == WorkspaceKind::Opensandbox;
            let record = ProjectWorkspace {
                project_id: project.id.clone(),
                provider: kind,
                state: if remote { "provisioning" } else { "ready" }.into(),
                cwd: if remote {
                    "/workspace".into()
                } else {
                    local.local_cwd
                },
                environment_id: format!("feynman-{}", project.id),
                sandbox_id: None,
                volume_name: remote.then(|| format!("feynman-{}", project.id.replace('_', "-"))),
                expires_at: None,
                updated_at: Utc::now(),
            };
            // Validate configuration before persisting a remote binding.
            if remote {
                self.remote()?;
            }
            self.repository.save_workspace(principal, &record)?;
            record
        };
        if workspace.provider == WorkspaceKind::Opensandbox {
            self.remote()?
                .create_or_reconcile(principal, &mut workspace)
                .await?;
            self.repository.save_workspace(principal, &workspace)?;
        }
        Ok(workspace)
    }

    fn assert_idle(&self, principal: &Principal, project_id: &str) -> PlatformResult<()> {
        // Snapshot lazily creates the one conversation/session-independent thread record.
        if self
            .repository
            .conversation_snapshot(principal, project_id)?
            .active_run
            .is_some()
        {
            return Err(PlatformError::RunAlreadyActive);
        }
        Ok(())
    }

    pub async fn action(
        &self,
        principal: &Principal,
        project_id: &str,
        action: &str,
    ) -> PlatformResult<ProjectWorkspace> {
        let project = self.repository.get_project(principal, project_id)?;
        let lock = self.lock(&project).await;
        let _guard = lock.lock().await;
        if !matches!(action, "refresh" | "renew") {
            self.assert_idle(principal, project_id)?;
        }
        let mut workspace = self
            .get(principal, project_id)?
            .ok_or(PlatformError::NotFound)?;
        if workspace.provider != WorkspaceKind::Opensandbox {
            return Err(PlatformError::InvalidInput(
                "lifecycle actions require an OpenSandbox Workspace".into(),
            ));
        }
        self.remote()?
            .action(principal, &mut workspace, action)
            .await?;
        self.repository.save_workspace(principal, &workspace)?;
        Ok(workspace)
    }

    pub async fn close(&self) {
        if let Some(remote) = &self.remote {
            remote.close().await;
        }
    }

    pub async fn seed(
        &self,
        principal: &Principal,
        project_id: &str,
        input: feynman_local::evaluation::SeedInput,
    ) -> PlatformResult<serde_json::Value> {
        input.validate().map_err(|_| {
            PlatformError::InvalidInput("invalid bounded baseline/fact snapshot".into())
        })?;
        let project = self.repository.get_project(principal, project_id)?;
        let lock = self.lock(&project).await;
        let _guard = lock.lock().await;
        self.assert_idle(principal, project_id)?;
        if !self
            .repository
            .conversation_snapshot(principal, project_id)?
            .runs
            .is_empty()
        {
            return Err(PlatformError::Conflict(
                "seed requires a new Project with no previous Agent Runs".into(),
            ));
        }
        let mut workspace = self
            .get(principal, project_id)?
            .ok_or(PlatformError::NotFound)?;
        let bytes = serde_json::to_vec(&input)
            .map_err(|_| PlatformError::InvalidInput("invalid seed".into()))?;
        if workspace.provider == WorkspaceKind::Opensandbox {
            let result = self
                .remote()?
                .seed(principal, &mut workspace, bytes)
                .await?;
            self.repository.save_workspace(principal, &workspace)?;
            return Ok(result);
        }
        let source =
            PathBuf::from(&workspace.cwd).join(format!(".seed-{}.json", uuid::Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        options
            .open(&source)
            .and_then(|mut f| f.write_all(&bytes))
            .map_err(|_| PlatformError::Storage("stage local baseline".into()))?;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            tokio::process::Command::new("python3")
                .args(["-c", include_str!("workspace/seed.py"), &workspace.cwd])
                .arg(&source)
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await;
        let _ = std::fs::remove_file(&source);
        let output = result
            .map_err(|_| PlatformError::Runtime("workspace seed timed out".into()))?
            .map_err(|_| PlatformError::Runtime("workspace bootstrap unavailable".into()))?;
        if !output.status.success() {
            return Err(PlatformError::Conflict(
                "workspace seed failed; existing files are never replaced".into(),
            ));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|_| PlatformError::Runtime("invalid bootstrap result".into()))
    }

    pub async fn renew_active(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<()> {
        let Some(remote) = &self.remote else {
            return Ok(());
        };
        if let Some(workspace) = self.get(principal, project_id)?
            && workspace.provider == WorkspaceKind::Opensandbox
            && workspace.state == "running"
            && remote.renewal_due(&workspace)
        {
            self.action(principal, project_id, "renew").await?;
        }
        Ok(())
    }
}

#[async_trait]
impl WorkspaceProvider for ProjectWorkspaceProvider {
    async fn ensure_workspace(
        &self,
        principal: &Principal,
        project: &Project,
    ) -> PlatformResult<PreparedWorkspace> {
        let lock = self.lock(project).await;
        let _guard = lock.lock().await;
        let local = self.local.ensure_workspace(principal, project).await?;
        let Some(mut workspace) = self.get(principal, &project.id)? else {
            self.repository.save_workspace(
                principal,
                &ProjectWorkspace {
                    project_id: project.id.clone(),
                    provider: WorkspaceKind::Local,
                    state: "ready".into(),
                    cwd: local.local_cwd.clone(),
                    environment_id: format!("feynman-{}", project.id),
                    sandbox_id: None,
                    volume_name: None,
                    expires_at: None,
                    updated_at: Utc::now(),
                },
            )?;
            return Ok(local);
        };
        if workspace.provider == WorkspaceKind::Local {
            return Ok(local);
        }
        let environment = self.remote()?.prepare(principal, &mut workspace).await?;
        self.repository.save_workspace(principal, &workspace)?;
        Ok(PreparedWorkspace {
            local_cwd: local.local_cwd,
            environment: Some(environment),
        })
    }
}
