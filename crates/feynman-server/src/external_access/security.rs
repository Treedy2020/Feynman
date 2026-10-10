//! Internal composition seams. Public callers never receive a CredentialRef or secret.
use async_trait::async_trait;
use chrono::Utc;
use feynman_core::{
    CliProviderDescriptor, PlatformError, PlatformResult, Principal, ProjectCliBinding,
};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct TrustedCall<'a> {
    pub principal: &'a Principal,
    pub project_id: &'a str,
    pub agent_id: &'a str,
    pub tool: &'a str,
    pub binding: &'a ProjectCliBinding,
    pub connection_enabled: bool,
    pub granted: bool,
}
pub enum PolicyDecision {
    Allow,
    NeedsApproval,
    Deny,
}
pub trait AccessPolicy: Send + Sync {
    fn evaluate(&self, call: &TrustedCall<'_>) -> PolicyDecision;
}
pub struct ProjectAccessPolicy;
impl AccessPolicy for ProjectAccessPolicy {
    fn evaluate(&self, c: &TrustedCall<'_>) -> PolicyDecision {
        if c.principal.tenant_id.is_empty()
            || c.principal.user_id.is_empty()
            || c.agent_id.is_empty()
            || c.tool.is_empty()
            || c.project_id != c.binding.project_id
            || !c.connection_enabled
            || c.binding.expires_at <= Utc::now()
        {
            PolicyDecision::Deny
        } else if !c.granted {
            PolicyDecision::NeedsApproval
        } else {
            PolicyDecision::Allow
        }
    }
}
pub trait CredentialDeliveryStrategy: Send + Sync {
    fn authorize(&self, mode: &str, call: &TrustedCall<'_>) -> PlatformResult<()>;
}
pub struct ServerProxyDelivery;
impl CredentialDeliveryStrategy for ServerProxyDelivery {
    fn authorize(&self, mode: &str, c: &TrustedCall<'_>) -> PlatformResult<()> {
        if mode != "server_proxy"
            || !matches!(ProjectAccessPolicy.evaluate(c), PolicyDecision::Allow)
        {
            return Err(PlatformError::Conflict("this provider policy supports only authorized Server Proxy calls; arbitrary credential delivery is not available".into()));
        }
        Ok(())
    }
}
/// Not Serialize/Debug: an internal reference is neither public data nor bearer authority.
pub struct CredentialRef {
    pub(super) connection_id: String,
}
pub trait CredentialStore: Send + Sync {
    fn put(&self, reference: &CredentialRef, secret: &str) -> PlatformResult<()>;
    fn read(&self, reference: &CredentialRef) -> PlatformResult<String>;
}
pub trait AuditSink: Send + Sync {
    fn record(
        &self,
        p: &Principal,
        project: &str,
        action: &str,
        object: Option<&str>,
    ) -> PlatformResult<()>;
}
pub trait ExecutionPolicy: Send + Sync {
    fn validate(&self, args: &[String], env: &[(String, String)]) -> PlatformResult<()>;
}
pub struct FixedCliExecution;
impl ExecutionPolicy for FixedCliExecution {
    fn validate(&self, args: &[String], env: &[(String, String)]) -> PlatformResult<()> {
        let fixed_git = args.len() == 14
            && args[..13].iter().map(String::as_str).eq([
                "-c",
                "credential.helper=",
                "-c",
                "credential.useHttpPath=true",
                "-c",
                "http.followRedirects=false",
                "-c",
                "protocol.allow=never",
                "-c",
                "protocol.https.allow=always",
                "ls-remote",
                "--heads",
                "--tags",
            ]);
        let event = args.first().is_some_and(|s| s == "event")
            && args.get(1).is_some_and(|s| s == "+listen-im");
        if args.is_empty()
            || args.len() > 64
            || args.iter().any(|a| a.len() > 8192 || a.contains('\0'))
            || !(fixed_git || event || matches!(args[0].as_str(), "api" | "chat" | "aisearch"))
        {
            return Err(PlatformError::InvalidInput(
                "unsupported fixed CLI execution".into(),
            ));
        }
        const KEYS: &[&str] = &[
            "GITLAB_TOKEN",
            "GLAB_CONFIG_DIR",
            "GLAB_ENABLE_CI_AUTOLOGIN",
            "GLAB_CHECK_UPDATE",
            "GITLAB_HOST",
            "DWS_CONFIG_DIR",
            "DWS_KEYCHAIN_DIR",
            "DWS_DISABLE_KEYCHAIN",
            "GIT_ASKPASS",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_NOSYSTEM",
            "GIT_TERMINAL_PROMPT",
            "FEYNMAN_GIT_ASKPASS",
            "FEYNMAN_GIT_TOKEN",
            "FEYNMAN_GIT_EXPECTED_REMOTE",
        ];
        if env
            .iter()
            .any(|(k, v)| !KEYS.contains(&k.as_str()) || v.contains('\0'))
        {
            return Err(PlatformError::InvalidInput(
                "unsupported authenticated CLI environment".into(),
            ));
        }
        Ok(())
    }
}

/// Secret input is owner-only and never serialized. Device links/status remain public DTOs.
pub enum AuthIntent {
    Device { reason: String },
    OwnerToken { hostname: String, token: String },
}
#[async_trait]
pub trait AuthenticationStrategy: Send + Sync {
    async fn start(
        &self,
        p: &Principal,
        project: &str,
        intent: AuthIntent,
    ) -> PlatformResult<Value>;
    async fn inspect(&self, p: &Principal, project: &str, id: &str) -> PlatformResult<Value>;
    async fn cancel(&self, p: &Principal, project: &str, id: &str) -> PlatformResult<Value>;
    async fn refresh(&self, p: &Principal, project: &str, id: &str) -> PlatformResult<Value>;
}
pub struct DeviceAuthentication(
    pub Arc<super::providers::dws::authorization::DwsAuthorizationService>,
);
#[async_trait]
impl AuthenticationStrategy for DeviceAuthentication {
    async fn start(
        &self,
        p: &Principal,
        project: &str,
        intent: AuthIntent,
    ) -> PlatformResult<Value> {
        let AuthIntent::Device { reason } = intent else {
            return Err(PlatformError::InvalidInput(
                "device strategy requires reason, not secret input".into(),
            ));
        };
        serde_json::to_value(
            self.0
                .request(
                    p,
                    project,
                    super::providers::dws::authorization::AuthorizationRequest { reason },
                )
                .await?,
        )
        .map_err(|_| PlatformError::Runtime("authentication encoding failed".into()))
    }
    async fn inspect(&self, p: &Principal, project: &str, id: &str) -> PlatformResult<Value> {
        serde_json::to_value(self.0.get(p, project, id)?)
            .map_err(|_| PlatformError::Runtime("authentication encoding failed".into()))
    }
    async fn cancel(&self, p: &Principal, project: &str, id: &str) -> PlatformResult<Value> {
        serde_json::to_value(self.0.cancel(p, project, id).await?)
            .map_err(|_| PlatformError::Runtime("authentication encoding failed".into()))
    }
    async fn refresh(&self, p: &Principal, project: &str, id: &str) -> PlatformResult<Value> {
        serde_json::to_value(self.0.check(p, project, id).await?)
            .map_err(|_| PlatformError::Runtime("authentication encoding failed".into()))
    }
}
pub struct OwnerTokenAuthentication(pub super::providers::gitlab::GitlabProvider);
#[async_trait]
impl AuthenticationStrategy for OwnerTokenAuthentication {
    async fn start(
        &self,
        _p: &Principal,
        _project: &str,
        intent: AuthIntent,
    ) -> PlatformResult<Value> {
        use super::providers::ExternalAccessProvider;
        let AuthIntent::OwnerToken { hostname, token } = intent else {
            return Err(PlatformError::InvalidInput(
                "owner token strategy does not support device login".into(),
            ));
        };
        Ok(
            json!({"status":"verified","identity":self.0.verify(&hostname,&token).await?,"authentication":"owner_token_stdin","refresh_supported":false}),
        )
    }
    async fn inspect(&self, _p: &Principal, _project: &str, _id: &str) -> PlatformResult<Value> {
        Err(PlatformError::InvalidInput(
            "PAT has no persistent device attempt; inspect its Connection".into(),
        ))
    }
    async fn cancel(&self, p: &Principal, project: &str, id: &str) -> PlatformResult<Value> {
        self.inspect(p, project, id).await
    }
    async fn refresh(&self, p: &Principal, project: &str, id: &str) -> PlatformResult<Value> {
        self.inspect(p, project, id).await
    }
}
pub fn descriptor() -> Value {
    json!({"provider_interface":std::any::type_name::<CliProviderDescriptor>(),"strategies":["ExternalAccessProvider","AuthenticationStrategy","AccessPolicy","CredentialStore","CredentialDeliveryStrategy","ExternalToolAdapter","ExecutionPolicy","AuditSink"],"authentication":["dws_device","gitlab_owner_token","native_owner_import","slate_browser_pkce","drlee_native_browser"],"delivery":["server_proxy","fixed_sandbox_git_clone"],"approved_workload_delivery":["process_env","verified_tmpfs_file"],"leases":["usage_lease"],"secret_storage":["macos_keychain","explicit_private_file"],"unsupported":["provider_credential_lease","ssh_credential_issuance","service_account","encrypted_vault","strong_multitenancy"]})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proxy_policy_is_fail_closed_and_never_mislabels_long_keys_as_leases() {
        let p = Principal {
            tenant_id: "t".into(),
            user_id: "u".into(),
        };
        let b = ProjectCliBinding {
            project_id: "p".into(),
            provider: feynman_core::AccessProvider::Dws,
            connection_id: "c".into(),
            repository: None,
            resource_id: None,
            expires_at: Utc::now() + chrono::Duration::hours(1),
        };
        let mut c = TrustedCall {
            principal: &p,
            project_id: "p",
            agent_id: "a",
            tool: "read",
            binding: &b,
            connection_enabled: true,
            granted: true,
        };
        assert!(ServerProxyDelivery.authorize("server_proxy", &c).is_ok());
        for mode in ["env", "temporary_file", "credential_lease"] {
            assert!(ServerProxyDelivery.authorize(mode, &c).is_err())
        }
        c.granted = false;
        assert!(matches!(
            ProjectAccessPolicy.evaluate(&c),
            PolicyDecision::NeedsApproval
        ));
        c.connection_enabled = false;
        assert!(matches!(
            ProjectAccessPolicy.evaluate(&c),
            PolicyDecision::Deny
        ));
    }
}
