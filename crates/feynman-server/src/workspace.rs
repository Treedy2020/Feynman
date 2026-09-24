use async_trait::async_trait;
use feynman_core::{PlatformError, PlatformResult, Project, WorkspaceProvider};
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
    async fn ensure_workspace(&self, project: &Project) -> PlatformResult<String> {
        let path = self.root.join(&project.tenant_id).join(&project.id);
        tokio::fs::create_dir_all(&path).await.map_err(|error| {
            PlatformError::Runtime(format!("create persistent local workspace: {error}"))
        })?;
        let absolute_path = tokio::fs::canonicalize(&path).await.map_err(|error| {
            PlatformError::Runtime(format!("resolve persistent local workspace: {error}"))
        })?;
        Ok(absolute_path.to_string_lossy().into_owned())
    }
}
