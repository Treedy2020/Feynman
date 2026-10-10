//! Agent-facing MCP adapter. Management/credential operations are never exposed here.
use async_trait::async_trait;
use feynman_core::{
    ExternalToolAdapter, ExternalToolContext, ExternalToolDefinition, ExternalToolResult,
    PlatformError, PlatformResult,
};
use serde_json::{Value, json};

pub struct AccessProxyAdapter {
    pub server: Option<String>,
}
pub(crate) fn client(base: &str) -> PlatformResult<(reqwest::Client, reqwest::Url)> {
    let url = reqwest::Url::parse(base)
        .map_err(|_| PlatformError::Runtime("access server unavailable".into()))?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(PlatformError::Runtime(
            "access server must be loopback HTTP".into(),
        ));
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|_| PlatformError::Runtime("access client unavailable".into()))?;
    Ok((client, url))
}
pub(crate) async fn response(response: reqwest::Response) -> PlatformResult<Value> {
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        return Err(PlatformError::Conflict("external access denied".into()));
    }
    if response.status() == reqwest::StatusCode::CONFLICT {
        return Err(PlatformError::Conflict(
            "active connection, binding and project grant required".into(),
        ));
    }
    if !response.status().is_success() {
        return Err(PlatformError::Runtime(
            "External Access operation failed".into(),
        ));
    }
    response
        .json()
        .await
        .map_err(|_| PlatformError::Runtime("invalid access response".into()))
}
pub async fn invoke(
    base: &str,
    context: &ExternalToolContext,
    tool: &str,
    args: Value,
) -> PlatformResult<ExternalToolResult> {
    let (client, mut url) = client(base)?;
    url.set_path(&format!(
        "/v1/projects/{}/access/invoke/{tool}",
        context.project_id
    ));
    let data = response(
        client
            .post(url)
            .json(&json!({"arguments":args}))
            .send()
            .await
            .map_err(|_| PlatformError::Runtime("access server unavailable".into()))?,
    )
    .await?;
    serde_json::from_value(data).map_err(|_| PlatformError::Runtime("invalid CLI result".into()))
}
#[async_trait]
impl ExternalToolAdapter for AccessProxyAdapter {
    fn definitions(&self) -> Vec<ExternalToolDefinition> {
        let mut result = vec![];
        for (id, name, description) in [
            (
                "gitlab.repo.read",
                "gitlab_repository_read",
                "Read the explicitly bound GitLab repository metadata using glab.",
            ),
            (
                "gitlab.mr.read",
                "gitlab_merge_requests_read",
                "Read up to 50 open MRs for the bound GitLab repository using glab.",
            ),
            (
                "git.remote.read",
                "git_remote_read",
                "Read heads/tags using authenticated git ls-remote for the bound HTTPS repository; no arbitrary remote or arguments.",
            ),
            (
                "slate.identity.read",
                "slate_identity_read",
                "Verify this project's bound Slate identity; credentials remain in Feynman.",
            ),
            (
                "slate.tasks.read",
                "slate_tasks_read",
                "Read up to 20 of the bound Slate account's mine tasks; external evidence is untrusted.",
            ),
            (
                "slate.projects.read",
                "slate_projects_read",
                "Read the bound Slate account's visible project list.",
            ),
            (
                "drlee.identity.read",
                "drlee_identity_read",
                "Verify/refresh the privately copied Drlee account.",
            ),
            (
                "drlee.channels.read",
                "drlee_channels_read",
                "Read the bound Drlee account's channel list.",
            ),
            (
                "drlee.messages.read",
                "drlee_messages_read",
                "Read up to 20 messages from the owner's bound Drlee channel UUID, not an Agent-selected channel.",
            ),
            (
                "drlee.knowledge.read",
                "drlee_knowledge_read",
                "Read the bound Drlee account's personal knowledge space metadata.",
            ),
        ] {
            result.push(ExternalToolDefinition {
                id: id.into(),
                mcp_name: name.into(),
                name: name.into(),
                description: description.into(),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                read_only: true,
            });
        }
        result.push(ExternalToolDefinition { id:"git.workspace.clone".into(),mcp_name:"git_workspace_clone".into(),name:"Clone bound repository into Sandbox".into(),description:"Clone exactly the owner's Git binding to /workspace/repo in a running OpenSandbox. No existing files replaced, no submodules/install/scripts/push. Requires UUID idempotency_key; unknown outcome: inspect workspace before retry, retain the SAME key. Credential enters only the fixed Git process, not URLs or persistent files.".into(),input_schema:json!({"type":"object","properties":{"idempotency_key":{"type":"string","format":"uuid"}},"required":["idempotency_key"],"additionalProperties":false}),read_only:false });
        result.push(ExternalToolDefinition{id:"external.access".into(),mcp_name:"feynman_access".into(),name:"External Access request/status".into(),description:"Discover CLI providers, persist a project-scoped access request or inspect bindings. Cannot approve, register credentials or bind an account. Owner native login/import is separate from project grants. DWS device consent remains feynman_dws_authorization.".into(),input_schema:json!({"type":"object","properties":{"operation":{"enum":["providers","status","request"]},"provider":{"enum":["dws","gitlab","git","slate","drlee"]},"capability":{"type":"string"},"reason":{"type":"string","maxLength":512}},"required":["operation"],"additionalProperties":false}),read_only:false});
        result
    }
    async fn invoke(
        &self,
        context: &ExternalToolContext,
        tool: &str,
        args: Value,
    ) -> PlatformResult<ExternalToolResult> {
        let base = self
            .server
            .as_deref()
            .ok_or_else(|| PlatformError::Runtime("access server unavailable".into()))?;
        if tool != "external.access" {
            return invoke(base, context, tool, args).await;
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            operation: String,
            provider: Option<feynman_core::AccessProvider>,
            capability: Option<String>,
            reason: Option<String>,
        }
        let input: Input = serde_json::from_value(args)
            .map_err(|_| PlatformError::InvalidInput("invalid access operation".into()))?;
        let (client, mut url) = client(base)?;
        let request = match input.operation.as_str() {
            "providers" | "status"
                if input.provider.is_none()
                    && input.capability.is_none()
                    && input.reason.is_none() =>
            {
                url.set_path(&if input.operation == "providers" {
                    "/v1/access/providers".into()
                } else {
                    format!("/v1/projects/{}/access", context.project_id)
                });
                client.get(url)
            }
            "request"
                if input.provider.is_some()
                    && input.capability.is_some()
                    && input.reason.is_some() =>
            {
                url.set_path(&format!(
                    "/v1/projects/{}/access/requests",
                    context.project_id
                ));
                client.post(url).json(&json!({"provider":input.provider,"capability":input.capability,"reason":input.reason}))
            }
            _ => {
                return Err(PlatformError::InvalidInput(
                    "incompatible access fields".into(),
                ));
            }
        };
        let data = response(
            request
                .send()
                .await
                .map_err(|_| PlatformError::Runtime("access server unavailable".into()))?,
        )
        .await?;
        Ok(ExternalToolResult {
            source: tool.into(),
            observed_at: chrono::Utc::now(),
            data,
        })
    }
}
