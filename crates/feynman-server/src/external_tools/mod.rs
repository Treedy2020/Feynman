mod mcp;
mod opensandbox;

use feynman_core::{ExternalToolAdapter, ExternalToolService, PlatformRepository, PlatformResult};
use std::sync::Arc;

pub use crate::external_access::providers::dws::search::DwsEnterpriseSearchAdapter;
use crate::external_access::{providers::dws::access::DwsAccessAdapter, proxy::AccessProxyAdapter};

pub fn service(
    repository: Arc<dyn PlatformRepository>,
) -> PlatformResult<Arc<ExternalToolService>> {
    service_at(repository, std::env::var("FEYNMAN_ACCESS_SERVER_URL").ok())
}

pub fn service_at(
    repository: Arc<dyn PlatformRepository>,
    server_url: Option<String>,
) -> PlatformResult<Arc<ExternalToolService>> {
    let mut adapters: Vec<Arc<dyn ExternalToolAdapter>> = vec![
        Arc::new(DwsEnterpriseSearchAdapter::with_access_server(
            server_url.clone(),
        )),
        Arc::new(DwsAccessAdapter::new(server_url.clone())),
        Arc::new(
            crate::external_access::providers::dws::messages::MessagesAdapter {
                server: server_url.clone(),
            },
        ),
        Arc::new(AccessProxyAdapter { server: server_url }),
    ];
    if let Some(adapter) = opensandbox::OpenSandboxSmokeAdapter::from_env()? {
        adapters.push(Arc::new(adapter));
    }
    Ok(Arc::new(ExternalToolService::new(repository, adapters)?))
}

pub async fn run_stdio_mcp_server() -> anyhow::Result<()> {
    mcp::run_stdio().await
}
