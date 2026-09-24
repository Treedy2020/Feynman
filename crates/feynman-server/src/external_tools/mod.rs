mod dws;
mod mcp;

use feynman_core::{ExternalToolAdapter, ExternalToolService, PlatformRepository, PlatformResult};
use std::sync::Arc;

pub use dws::DwsEnterpriseSearchAdapter;

pub fn service(
    repository: Arc<dyn PlatformRepository>,
) -> PlatformResult<Arc<ExternalToolService>> {
    Ok(Arc::new(ExternalToolService::new(
        repository,
        vec![Arc::new(DwsEnterpriseSearchAdapter::default()) as Arc<dyn ExternalToolAdapter>],
    )?))
}

pub async fn run_stdio_mcp_server() -> anyhow::Result<()> {
    mcp::run_stdio().await
}
