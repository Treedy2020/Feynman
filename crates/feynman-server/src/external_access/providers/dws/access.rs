use async_trait::async_trait;
use chrono::Utc;
use feynman_core::{
    ExternalToolAdapter, ExternalToolContext, ExternalToolDefinition, ExternalToolResult,
    PlatformError, PlatformResult,
};
use serde::Deserialize;
use serde_json::{Value, json};

pub struct DwsAccessAdapter {
    server_url: Option<String>,
}

impl DwsAccessAdapter {
    pub fn new(server_url: Option<String>) -> Self {
        Self { server_url }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    operation: String,
    reason: Option<String>,
    authorization_id: Option<String>,
}

#[async_trait]
impl ExternalToolAdapter for DwsAccessAdapter {
    fn definitions(&self) -> Vec<ExternalToolDefinition> {
        vec![ExternalToolDefinition {
            id: crate::dws_authorization::TOOL_ID.into(), mcp_name: "feynman_dws_authorization".into(),
            name: "DWS device 授权".into(),
            description: "申请 DWS device 授权、查看认证链接和状态、取消等待或重新检查登录态。request 不阻塞等待用户操作；把 verification_uri 和 user_code 展示给用户，再用 status 检查。浏览器授权由用户完成。不会自动授权业务工具，不返回 token，不装载到远端沙盒。".into(),
            input_schema: json!({"type":"object", "properties": {
                "operation":{"type":"string","enum":["request","list","status","cancel","check"]},
                "reason":{"type":"string","minLength":1,"maxLength":512},
                "authorization_id":{"type":"string","maxLength":100}
            }, "required":["operation"], "additionalProperties":false}),
            read_only: false,
        }]
    }

    async fn invoke(
        &self,
        context: &ExternalToolContext,
        tool_id: &str,
        arguments: Value,
    ) -> PlatformResult<ExternalToolResult> {
        if tool_id != crate::dws_authorization::TOOL_ID {
            return Err(PlatformError::InvalidInput(
                "unknown authorization tool".into(),
            ));
        }
        let arguments: Arguments = serde_json::from_value(arguments)
            .map_err(|_| PlatformError::InvalidInput("invalid authorization arguments".into()))?;
        let base = self.server_url.as_ref().ok_or_else(|| {
            PlatformError::Runtime("Feynman authorization server is not configured".into())
        })?;
        let mut url = reqwest::Url::parse(base)
            .map_err(|_| PlatformError::Runtime("invalid authorization server".into()))?;
        if url.scheme() != "http"
            || !url.username().is_empty()
            || url.password().is_some()
            || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1"))
        {
            return Err(PlatformError::Runtime(
                "authorization server must be loopback HTTP".into(),
            ));
        }
        url.set_query(None);
        url.set_fragment(None);
        let path = format!("/v1/projects/{}/dws-authorizations", context.project_id);
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(40))
            .no_proxy()
            .build()
            .map_err(|_| {
                PlatformError::Runtime("authorization client initialization failed".into())
            })?;
        let id = || -> PlatformResult<&str> {
            let id = arguments.authorization_id.as_deref().ok_or_else(|| {
                PlatformError::InvalidInput("authorization_id is required".into())
            })?;
            if !id.starts_with("dws_auth_")
                || id.len() > 100
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(PlatformError::InvalidInput(
                    "invalid authorization_id".into(),
                ));
            }
            Ok(id)
        };
        let request = match arguments.operation.as_str() {
            "request" if arguments.authorization_id.is_none() => {
                let reason = arguments
                    .reason
                    .as_deref()
                    .filter(|value| !value.trim().is_empty() && value.chars().count() <= 512)
                    .ok_or_else(|| PlatformError::InvalidInput("request requires reason".into()))?;
                url.set_path(&path);
                client.post(url).json(&json!({"reason":reason}))
            }
            "list" if arguments.reason.is_none() && arguments.authorization_id.is_none() => {
                url.set_path(&path);
                client.get(url)
            }
            "status" | "cancel" | "check" if arguments.reason.is_none() => {
                url.set_path(&format!(
                    "{path}/{}{}",
                    id()?,
                    if arguments.operation == "check" {
                        "/check"
                    } else {
                        ""
                    }
                ));
                match arguments.operation.as_str() {
                    "cancel" => client.delete(url),
                    "check" => client.post(url).json(&json!({})),
                    _ => client.get(url),
                }
            }
            _ => {
                return Err(PlatformError::InvalidInput(
                    "invalid operation or incompatible arguments".into(),
                ));
            }
        };
        let response = request
            .send()
            .await
            .map_err(|_| PlatformError::Runtime("authorization server is unavailable".into()))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(PlatformError::NotFound);
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(PlatformError::ToolNotGranted(tool_id.into()));
        }
        if !status.is_success() {
            return Err(PlatformError::Runtime(
                "authorization request failed".into(),
            ));
        }
        let data = response
            .json()
            .await
            .map_err(|_| PlatformError::Runtime("invalid authorization response".into()))?;
        Ok(ExternalToolResult {
            source: tool_id.into(),
            observed_at: Utc::now(),
            data,
        })
    }
}
