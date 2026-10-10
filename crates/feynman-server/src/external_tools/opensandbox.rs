//! Diagnostic file bridge, not a production WorkspaceProvider or lifecycle API.
//! Trusted operator config binds exactly one Tenant/Project to one sandbox.
use async_trait::async_trait;
use chrono::Utc;
use feynman_core::{
    ExternalToolAdapter, ExternalToolContext, ExternalToolDefinition, ExternalToolResult,
    PlatformError, PlatformResult,
};
use reqwest::{Client, Url, header::HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

const TOOL_ID: &str = "sandbox.files.smoke";
const ROOT: &str = "/workspace/smoke";
const FILES: [&str; 2] = ["note.txt", "probe.json"];
const MAX_BYTES: usize = 65_536;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Connection {
    base_url: String,
    api_key: String,
    tenant_id: String,
    project_id: String,
    sandbox_id: String,
}

pub struct OpenSandboxSmokeAdapter {
    connection: Connection,
    client: Client,
    base_url: Url,
    api_key: HeaderValue,
}

impl OpenSandboxSmokeAdapter {
    pub fn from_env() -> PlatformResult<Option<Self>> {
        let Some(path) = std::env::var_os("FEYNMAN_OPENSANDBOX_SMOKE_CONFIG") else {
            return Ok(None);
        };
        let invalid =
            || PlatformError::InvalidInput("invalid private OpenSandbox smoke config".into());
        let file = std::fs::File::open(path).map_err(|_| invalid())?;
        let metadata = file.metadata().map_err(|_| invalid())?;
        if !metadata.is_file() || metadata.len() > 16_384 {
            return Err(invalid());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(invalid());
            }
        }
        let connection: Connection = serde_json::from_reader(file).map_err(|_| invalid())?;
        let base_url = Url::parse(&connection.base_url).map_err(|_| invalid())?;
        // This smoke uses a private SSH tunnel only. No caller-controlled hosts,
        // credential-bearing URLs, redirects, or environment HTTP proxies.
        if base_url.scheme() != "http"
            || base_url.host_str() != Some("127.0.0.1")
            || base_url.port().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.path() != "/"
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || connection.tenant_id.is_empty()
            || connection.project_id.is_empty()
            || connection.api_key.is_empty()
            || uuid::Uuid::parse_str(&connection.sandbox_id).is_err()
        {
            return Err(invalid());
        }
        let mut api_key = HeaderValue::from_str(&connection.api_key).map_err(|_| invalid())?;
        api_key.set_sensitive(true);
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| unavailable())?;
        Ok(Some(Self {
            connection,
            client,
            base_url,
            api_key,
        }))
    }

    async fn get(&self, route: &str, query: &[(&str, &str)]) -> PlatformResult<Vec<u8>> {
        let mut url = self
            .base_url
            .join(&format!(
                "v1/sandboxes/{}/proxy/44772/{route}",
                self.connection.sandbox_id
            ))
            .map_err(|_| unavailable())?;
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        let mut response = self
            .client
            .get(url)
            .header("OPEN-SANDBOX-API-KEY", self.api_key.clone())
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|size| size > MAX_BYTES as u64)
        {
            return Err(unavailable());
        }
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if data.len() + chunk.len() > MAX_BYTES {
                return Err(unavailable());
            }
            data.extend_from_slice(&chunk);
        }
        Ok(data)
    }
}

fn unavailable() -> PlatformError {
    // Never forward upstream bodies, tokens, URLs, or transport diagnostics.
    PlatformError::Runtime("OpenSandbox smoke file request failed".into())
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Arguments {
    List,
    Read { file: SmokeFile },
}

#[derive(Deserialize)]
enum SmokeFile {
    #[serde(rename = "probe.json")]
    Probe,
    #[serde(rename = "note.txt")]
    Note,
}

#[async_trait]
impl ExternalToolAdapter for OpenSandboxSmokeAdapter {
    fn definitions(&self) -> Vec<ExternalToolDefinition> {
        vec![ExternalToolDefinition {
            read_only: true,
            id: TOOL_ID.into(),
            mcp_name: "sandbox_smoke_files".into(),
            name: "OpenSandbox 测试文件读取".into(),
            description: "只读查看本项目已绑定的远程测试沙盒 /workspace/smoke，列出文件或读取 probe.json、note.txt；不能执行命令或选择其他沙盒、路径。".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "operation": {"type": "string", "enum": ["list", "read"]},
                    "file": {"type": "string", "enum": FILES, "description": "read 时必填；list 时不传"}
                },
                "required": ["operation"],
                "additionalProperties": false
            }),
        }]
    }

    async fn invoke(
        &self,
        context: &ExternalToolContext,
        tool_id: &str,
        arguments: Value,
    ) -> PlatformResult<ExternalToolResult> {
        if context.principal.tenant_id != self.connection.tenant_id
            || context.project_id != self.connection.project_id
        {
            return Err(PlatformError::NotFound);
        }
        if tool_id != TOOL_ID {
            return Err(PlatformError::InvalidInput(
                "unsupported sandbox tool".into(),
            ));
        }
        let arguments: Arguments = serde_json::from_value(arguments).map_err(|_| {
            PlatformError::InvalidInput(
                "use list or read with an allowlisted smoke filename".into(),
            )
        })?;
        let mut data = match arguments {
            Arguments::List => {
                let bytes = self
                    .get("files/search", &[("path", ROOT), ("pattern", "*")])
                    .await?;
                let items: Vec<Value> =
                    serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
                let mut files = FILES
                    .into_iter()
                    .filter(|name| {
                        let path = format!("{ROOT}/{name}");
                        items
                            .iter()
                            .any(|item| item["path"].as_str() == Some(path.as_str()))
                    })
                    .collect::<Vec<_>>();
                files.sort_unstable();
                json!({"operation": "list", "path": ROOT, "files": files})
            }
            Arguments::Read { file } => {
                let name = match file {
                    SmokeFile::Probe => "probe.json",
                    SmokeFile::Note => "note.txt",
                };
                let path = format!("{ROOT}/{name}");
                let bytes = self
                    .get("files/download", &[("path", path.as_str())])
                    .await?;
                let content = String::from_utf8(bytes).map_err(|_| unavailable())?;
                json!({"operation": "read", "path": path, "content": content})
            }
        };
        data["sandbox_id"] = json!(self.connection.sandbox_id);
        Ok(ExternalToolResult {
            source: TOOL_ID.into(),
            observed_at: Utc::now(),
            data,
        })
    }
}
