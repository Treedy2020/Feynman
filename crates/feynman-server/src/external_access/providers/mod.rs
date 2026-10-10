pub mod dws;
pub mod git;
pub mod gitlab;
pub mod native;

use async_trait::async_trait;
use feynman_core::{CliProviderDescriptor, PlatformResult};

/// Authentication-specific CLI behaviour; project policy belongs to the Broker.
#[async_trait]
pub trait ExternalAccessProvider: Send + Sync {
    fn descriptor(&self) -> CliProviderDescriptor;
    async fn verify(&self, hostname: &str, credential: &str) -> PlatformResult<String>;
}
pub use ExternalAccessProvider as AuthenticatedCliProvider;
