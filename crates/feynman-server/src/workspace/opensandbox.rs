use chrono::{DateTime, Utc};
use feynman_core::{
    NativeExecutionEnvironment, PlatformError, PlatformResult, Principal, ProjectWorkspace,
};
use reqwest::{Client, Method, Url, header::HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Trusted local operator configuration, never supplied in Project/Agent requests.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    base_url: String,
    api_key: String,
    image: String,
    executor_ssh_target: Option<String>,
    allow_outer_sandbox_only: bool,
    #[serde(default = "default_timeout")]
    timeout_seconds: i64,
}
fn default_timeout() -> i64 {
    1800
}

struct Tunnel {
    child: Child,
    address: String,
    url: String,
}
pub struct OpenSandboxProvider {
    config: Config,
    base: Url,
    key: HeaderValue,
    client: Client,
    tunnels: Mutex<HashMap<String, Tunnel>>,
}
fn unavailable() -> PlatformError {
    PlatformError::Runtime("OpenSandbox Workspace unavailable; no local execution fallback".into())
}

impl OpenSandboxProvider {
    pub async fn fixed_control(
        &self,
        p: &Principal,
        w: &ProjectWorkspace,
        code: &str,
        payload: Value,
    ) -> PlatformResult<Value> {
        let mut current = w.clone();
        self.inspect(p, &mut current)
            .await?
            .ok_or_else(unavailable)?;
        if current.sandbox_id != w.sandbox_id || current.state != "running" {
            return Err(unavailable());
        }
        crate::resources::run_container_control(
            self.config.executor_ssh_target.as_deref(),
            json!({
                "sandbox_id":w.sandbox_id,"volume":w.volume_name,"feynman-project":w.project_id,
                "feynman-tenant":p.tenant_id,"feynman-environment":w.environment_id,
                "action":"clone","code":code,"payload":payload,"control_timeout":70
            }),
        )
        .await
    }
    pub async fn workload_control(
        &self,
        p: &Principal,
        w: &ProjectWorkspace,
        action: &str,
        mut payload: Value,
    ) -> PlatformResult<Value> {
        let mut current = w.clone();
        self.inspect(p, &mut current)
            .await?
            .ok_or_else(unavailable)?;
        if current.sandbox_id != w.sandbox_id || current.state != "running" {
            return Err(unavailable());
        }
        payload["root"] = json!("/tmp/feynman-workloads");
        payload["workspace"] = json!("/workspace");
        let payload = crate::resources::prepare_payload(payload);
        crate::resources::run_container_control(self.config.executor_ssh_target.as_deref(),json!({"sandbox_id":w.sandbox_id,"volume":w.volume_name,"feynman-project":w.project_id,"feynman-tenant":p.tenant_id,"feynman-environment":w.environment_id,"action":action,"code":crate::resources::RUNNER,"payload":payload})).await
    }
    pub fn from_env() -> PlatformResult<Option<Self>> {
        let Some(path) = std::env::var_os("FEYNMAN_OPENSANDBOX_CONFIG") else {
            return Ok(None);
        };
        let invalid = || {
            PlatformError::InvalidInput("invalid private OpenSandbox config (loopback API, private file, pinned image, explicit outer-sandbox consent required)".into())
        };
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
        let config: Config = serde_json::from_reader(file).map_err(|_| invalid())?;
        let base = Url::parse(&config.base_url).map_err(|_| invalid())?;
        if base.scheme() != "http"
            || base.host_str() != Some("127.0.0.1")
            || base.port().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.path() != "/"
            || base.query().is_some()
            || base.fragment().is_some()
            || config.api_key.is_empty()
            || config.image.is_empty()
            || !config.allow_outer_sandbox_only
            || !(300..=86400).contains(&config.timeout_seconds)
            || config.executor_ssh_target.as_ref().is_some_and(|target| {
                target.starts_with('-')
                    || target.is_empty()
                    || !target
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"@.-_".contains(&b))
            })
        {
            return Err(invalid());
        }
        let mut key = HeaderValue::from_str(&config.api_key).map_err(|_| invalid())?;
        key.set_sensitive(true);
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| unavailable())?;
        Ok(Some(Self {
            config,
            base,
            key,
            client,
            tunnels: Mutex::new(HashMap::new()),
        }))
    }

    async fn call(
        &self,
        method: Method,
        route: &str,
        body: Option<Value>,
    ) -> PlatformResult<Option<Value>> {
        let mut request = self
            .client
            .request(
                method,
                self.base
                    .join(&format!("v1/{route}"))
                    .map_err(|_| unavailable())?,
            )
            .header("OPEN-SANDBOX-API-KEY", self.key.clone());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request.send().await.map_err(|_| unavailable())?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(unavailable());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > 1_048_576 {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Ok(Some(json!({})));
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| unavailable())
    }

    fn owns(principal: &Principal, workspace: &ProjectWorkspace, sandbox: &Value) -> bool {
        sandbox
            .pointer("/metadata/feynman-project")
            .and_then(Value::as_str)
            == Some(workspace.project_id.as_str())
            && sandbox
                .pointer("/metadata/feynman-tenant")
                .and_then(Value::as_str)
                == Some(principal.tenant_id.as_str())
            && sandbox
                .pointer("/metadata/feynman-environment")
                .and_then(Value::as_str)
                == Some(workspace.environment_id.as_str())
    }

    async fn inspect(
        &self,
        principal: &Principal,
        workspace: &mut ProjectWorkspace,
    ) -> PlatformResult<Option<Value>> {
        let Some(id) = &workspace.sandbox_id else {
            return Ok(None);
        };
        let sandbox = self
            .call(Method::GET, &format!("sandboxes/{id}"), None)
            .await?;
        if let Some(value) = &sandbox {
            if !Self::owns(principal, workspace, value) {
                return Err(PlatformError::Conflict(
                    "remote Workspace ownership mismatch".into(),
                ));
            }
            workspace.state = value
                .pointer("/status/state")
                .and_then(Value::as_str)
                .unwrap_or("Unknown")
                .to_ascii_lowercase();
            workspace.expires_at = value["expiresAt"]
                .as_str()
                .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
                .map(|v| v.with_timezone(&Utc));
        } else {
            workspace.state = "missing".into();
        }
        workspace.updated_at = Utc::now();
        Ok(sandbox)
    }

    pub async fn create_or_reconcile(
        &self,
        principal: &Principal,
        workspace: &mut ProjectWorkspace,
    ) -> PlatformResult<()> {
        if workspace.sandbox_id.is_some() {
            self.inspect(principal, workspace).await?;
            return Ok(());
        }
        // Recover an ambiguous POST (e.g. connection lost after creation), without creating duplicates.
        let mut url = self.base.join("v1/sandboxes").map_err(|_| unavailable())?;
        url.query_pairs_mut()
            .append_pair(
                "metadata",
                &format!(
                    "feynman-project={}&feynman-tenant={}&feynman-environment={}",
                    workspace.project_id, principal.tenant_id, workspace.environment_id
                ),
            )
            .append_pair("pageSize", "100");
        let route = url
            .as_str()
            .strip_prefix(self.base.as_str())
            .and_then(|v| v.strip_prefix("v1/"))
            .ok_or_else(unavailable)?;
        let records = self
            .call(Method::GET, route, None)
            .await?
            .ok_or_else(unavailable)?;
        let matches: Vec<_> = records["items"]
            .as_array()
            .ok_or_else(unavailable)?
            .iter()
            .filter(|v| {
                Self::owns(principal, workspace, v)
                    && !matches!(
                        v.pointer("/status/state").and_then(Value::as_str),
                        Some("Terminated" | "Failed")
                    )
            })
            .collect();
        if matches.len() > 1 {
            return Err(PlatformError::Conflict(
                "multiple owned remote sandboxes; operator reconciliation required".into(),
            ));
        }
        let sandbox = if let Some(record) = matches.first() {
            (*record).clone()
        } else {
            self.call(Method::POST, "sandboxes", Some(json!({
                "image":{"uri":self.config.image}, "entrypoint":["sh","-c","cd /workspace && exec /opt/feynman/bin/codex exec-server --listen ws://0.0.0.0:44773 --concurrent-requests 8"],
                "resourceLimits":{"cpu":"1000m","memory":"1Gi"}, "timeout":self.config.timeout_seconds,
                "metadata":{"feynman-project":workspace.project_id,"feynman-tenant":principal.tenant_id,"feynman-environment":workspace.environment_id},
                "volumes":[{"name":"workspace","pvc":{"claimName":workspace.volume_name,"createIfNotExists":true,"deleteOnSandboxTermination":false},"mountPath":"/workspace"}]
            }))).await?.ok_or_else(unavailable)?
        };
        let id = sandbox["id"]
            .as_str()
            .filter(|id| uuid::Uuid::parse_str(id).is_ok())
            .ok_or_else(unavailable)?;
        workspace.sandbox_id = Some(id.into());
        workspace.state = "pending".into();
        self.inspect(principal, workspace).await?;
        Ok(())
    }

    pub async fn action(
        &self,
        principal: &Principal,
        workspace: &mut ProjectWorkspace,
        action: &str,
    ) -> PlatformResult<()> {
        let existing = self.inspect(principal, workspace).await?;
        let id = workspace.sandbox_id.clone().ok_or_else(unavailable)?;
        match action {
            "refresh" => {},
            "stop" if workspace.state == "paused" => {},
            "stop" if workspace.state == "running" => { self.call(Method::POST, &format!("sandboxes/{id}/pause"), None).await?.ok_or_else(unavailable)?; self.drop_tunnel(&workspace.project_id).await; },
            "resume" if workspace.state == "running" => {},
            "resume" if workspace.state == "paused" => { self.call(Method::POST, &format!("sandboxes/{id}/resume"), None).await?.ok_or_else(unavailable)?; },
            "restore" if existing.is_none() || matches!(workspace.state.as_str(), "terminated" | "failed") => {
                self.drop_tunnel(&workspace.project_id).await;
                workspace.sandbox_id = None;
                self.create_or_reconcile(principal, workspace).await?;
            },
            "restore" => return Err(PlatformError::Conflict("restore requires a missing or terminated sandbox; it never replaces a live container".into())),
            "renew" if existing.is_some() => { self.renew(workspace).await?; },
            _ => return Err(PlatformError::Conflict(format!("cannot {action} Workspace in state {}", workspace.state))),
        }
        self.inspect(principal, workspace).await?;
        Ok(())
    }

    async fn renew(&self, workspace: &ProjectWorkspace) -> PlatformResult<()> {
        let id = workspace.sandbox_id.as_ref().ok_or_else(unavailable)?;
        self.call(Method::POST, &format!("sandboxes/{id}/renew-expiration"), Some(json!({"expiresAt":(Utc::now() + chrono::Duration::seconds(self.config.timeout_seconds)).to_rfc3339()}))).await?.ok_or_else(unavailable)?;
        Ok(())
    }

    pub fn renewal_due(&self, workspace: &ProjectWorkspace) -> bool {
        workspace.expires_at.is_some_and(|expires| {
            expires < Utc::now() + chrono::Duration::seconds(self.config.timeout_seconds / 3)
        })
    }

    pub async fn prepare(
        &self,
        principal: &Principal,
        workspace: &mut ProjectWorkspace,
    ) -> PlatformResult<NativeExecutionEnvironment> {
        self.inspect(principal, workspace).await?;
        if matches!(
            workspace.state.as_str(),
            "missing" | "terminated" | "failed"
        ) {
            // The Project already owns this execution binding. Restore only its
            // retained volume; a transport/auth error is never treated as expiration.
            self.action(principal, workspace, "restore").await?;
        }
        if matches!(workspace.state.as_str(), "pending" | "resuming") {
            let until = tokio::time::Instant::now() + Duration::from_secs(60);
            while matches!(workspace.state.as_str(), "pending" | "resuming") {
                if tokio::time::Instant::now() > until {
                    return Err(unavailable());
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
                self.inspect(principal, workspace).await?;
            }
        }
        if workspace.state != "running" {
            return Err(PlatformError::Conflict(format!(
                "remote Workspace is {}; refresh/resume/restore explicitly",
                workspace.state
            )));
        }
        self.renew(workspace).await?;
        self.inspect(principal, workspace).await?;
        let id = workspace.sandbox_id.as_ref().ok_or_else(unavailable)?;
        let endpoint = self
            .call(
                Method::GET,
                &format!("sandboxes/{id}/endpoints/44773"),
                None,
            )
            .await?
            .ok_or_else(unavailable)?;
        if endpoint
            .get("headers")
            .and_then(Value::as_object)
            .is_some_and(|h| !h.is_empty())
        {
            return Err(unavailable());
        }
        let address = endpoint["endpoint"].as_str().ok_or_else(unavailable)?;
        let endpoint_url = Url::parse(&format!("http://{address}")).map_err(|_| unavailable())?;
        if !matches!(endpoint_url.host_str(), Some("127.0.0.1" | "localhost")) {
            return Err(unavailable());
        }
        let socket = std::net::SocketAddr::from((
            [127, 0, 0, 1],
            endpoint_url.port().ok_or_else(unavailable)?,
        ));
        // Executor has no authentication. Only operator-enforced loopback publishing is accepted.
        if !socket.ip().is_loopback()
            || socket.port() == 0
            || !endpoint_url.username().is_empty()
            || endpoint_url.password().is_some()
            || endpoint_url.query().is_some()
            || endpoint_url.fragment().is_some()
            || !matches!(endpoint_url.path(), "/" | "/proxy/44773")
        {
            return Err(unavailable());
        }
        let address = socket.to_string();
        let proxy_path = endpoint_url.path();
        let url = if let Some(target) = &self.config.executor_ssh_target {
            let mut tunnels = self.tunnels.lock().await;
            if let Some(tunnel) = tunnels.get_mut(&workspace.project_id)
                && (tunnel.address != address
                    || tunnel
                        .child
                        .try_wait()
                        .map_err(|_| unavailable())?
                        .is_some())
            {
                let mut old = tunnels.remove(&workspace.project_id).unwrap();
                let _ = old.child.kill().await;
            }
            if !tunnels.contains_key(&workspace.project_id) {
                let listener =
                    std::net::TcpListener::bind("127.0.0.1:0").map_err(|_| unavailable())?;
                let port = listener.local_addr().map_err(|_| unavailable())?.port();
                drop(listener);
                let child = Command::new("ssh")
                    .args([
                        "-N",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "ExitOnForwardFailure=yes",
                        "-o",
                        "ConnectTimeout=10",
                        "-o",
                        "ServerAliveInterval=15",
                        "-o",
                        "ServerAliveCountMax=2",
                        "-L",
                        &format!("127.0.0.1:{port}:{address}"),
                        target,
                    ])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .kill_on_drop(true)
                    .spawn()
                    .map_err(|_| unavailable())?;
                tunnels.insert(
                    workspace.project_id.clone(),
                    Tunnel {
                        child,
                        address: address.clone(),
                        url: format!("ws://127.0.0.1:{port}{proxy_path}"),
                    },
                );
            }
            tunnels[&workspace.project_id].url.clone()
        } else {
            format!("ws://{address}{proxy_path}")
        };
        let port = Url::parse(&url)
            .map_err(|_| unavailable())?
            .port()
            .ok_or_else(unavailable)?;
        let until = tokio::time::Instant::now() + Duration::from_secs(12);
        loop {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                break;
            }
            if tokio::time::Instant::now() > until {
                return Err(unavailable());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(NativeExecutionEnvironment {
            id: workspace.environment_id.clone(),
            exec_server_url: url,
            cwd: workspace.cwd.clone(),
        })
    }

    async fn drop_tunnel(&self, project_id: &str) {
        if let Some(mut tunnel) = self.tunnels.lock().await.remove(project_id) {
            let _ = tunnel.child.kill().await;
        }
    }
    pub async fn seed(
        &self,
        principal: &Principal,
        workspace: &mut ProjectWorkspace,
        bytes: Vec<u8>,
    ) -> PlatformResult<Value> {
        self.inspect(principal, workspace).await?;
        if workspace.state != "running" {
            return Err(PlatformError::Conflict(
                "seed requires a running remote Workspace".into(),
            ));
        }
        let id = workspace.sandbox_id.as_deref().ok_or_else(unavailable)?;
        let path = format!("/workspace/.seed-{}.json", uuid::Uuid::new_v4());
        let route = format!("v1/sandboxes/{id}/proxy/44772/");
        let upload = reqwest::multipart::Form::new()
            .part(
                "metadata",
                reqwest::multipart::Part::text(json!({"path":path,"mode":600}).to_string())
                    .file_name("metadata.json")
                    .mime_str("application/json")
                    .map_err(|_| unavailable())?,
            )
            .part(
                "file",
                reqwest::multipart::Part::bytes(bytes).file_name("snapshot.json"),
            );
        let response = self
            .client
            .post(
                self.base
                    .join(&format!("{route}files/upload"))
                    .map_err(|_| unavailable())?,
            )
            .header("OPEN-SANDBOX-API-KEY", self.key.clone())
            .multipart(upload)
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(unavailable());
        }
        fn quote(s: &str) -> String {
            format!("'{}'", s.replace('\'', "'\\''"))
        }
        // Fixed bootstrap only; all caller data is JSON in a generated path, never shell text.
        let command = format!(
            "python3 -c {} /workspace {}",
            quote(include_str!("seed.py")),
            quote(&path)
        );
        let mut response = self
            .client
            .post(
                self.base
                    .join(&format!("{route}command"))
                    .map_err(|_| unavailable())?,
            )
            .header("OPEN-SANDBOX-API-KEY", self.key.clone())
            .json(&json!({"command":command,"cwd":"/workspace","background":false,"timeout":60000}))
            .timeout(Duration::from_secs(70))
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(unavailable());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > 1_048_576 {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        let text = String::from_utf8(bytes).map_err(|_| unavailable())?;
        let mut stdout = String::new();
        let mut completed = false;
        // The pinned 0.2.3 proxy returns JSONL; newer execd specifications use
        // SSE data framing. Accept only these two bounded JSON event encodings.
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            if line.starts_with("event:") || line.starts_with("id:") || line.starts_with(':') {
                continue;
            }
            let line = line.strip_prefix("data:").unwrap_or(line).trim();
            let event: Value = serde_json::from_str(line).map_err(|_| unavailable())?;
            match event["type"].as_str() {
                Some("stdout") => stdout.push_str(event["text"].as_str().unwrap_or("")),
                Some("error") => return Err(PlatformError::Conflict("remote bootstrap failed; check image Git/Python availability and use a new Project".into())),
                Some("execution_complete") => completed = true,
                _ => {},
            }
        }
        if !completed {
            return Err(unavailable());
        }
        serde_json::from_str(stdout.trim()).map_err(|_| {
            PlatformError::Conflict("remote bootstrap did not produce a verified snapshot".into())
        })
    }
    pub async fn close(&self) {
        for (_, mut tunnel) in std::mem::take(&mut *self.tunnels.lock().await) {
            let _ = tunnel.child.kill().await;
        }
    }
}
