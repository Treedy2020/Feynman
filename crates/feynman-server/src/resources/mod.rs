//! Project environments, usage leases and evidence-aware memory. Local trusted-owner mode.
use crate::{
    external_access::ExternalAccessBroker, store::SqliteRepository,
    workspace::ProjectWorkspaceProvider,
};
use chrono::{DateTime, Duration, Utc};
use feynman_core::*;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::Arc;
mod environment;
pub mod http;
mod memory;
mod secrets;
pub const RUNNER: &str = include_str!("runner.py");

pub struct ResourceManager {
    pub(super) repo: Arc<SqliteRepository>,
    workspace: Arc<ProjectWorkspaceProvider>,
    access: Arc<ExternalAccessBroker>,
    secrets: secrets::Secrets,
    runner_root: std::path::PathBuf,
    lock: tokio::sync::Mutex<()>,
}
pub(super) fn invalid(s: &str) -> PlatformError {
    PlatformError::InvalidInput(s.into())
}
pub(super) fn decode<T: DeserializeOwned>(v: Value) -> PlatformResult<T> {
    serde_json::from_value(v).map_err(|_| invalid("invalid resource arguments"))
}
pub(super) fn encode<T: serde::Serialize>(v: T) -> PlatformResult<Value> {
    serde_json::to_value(v).map_err(|_| PlatformError::Storage("resource encoding failed".into()))
}
pub(super) fn string<'a>(v: &'a Value, k: &str) -> PlatformResult<&'a str> {
    v[k].as_str()
        .ok_or_else(|| invalid("missing string argument"))
}
pub(super) fn time(v: &Value, k: &str) -> PlatformResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(string(v, k)?)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| invalid("invalid timestamp"))
}
impl ResourceManager {
    pub fn new(
        repo: Arc<SqliteRepository>,
        workspace: Arc<ProjectWorkspaceProvider>,
        access: Arc<ExternalAccessBroker>,
        root: std::path::PathBuf,
    ) -> PlatformResult<Self> {
        let runner_root = root.join("workloads");
        feynman_local::private_directory(&root)
            .map_err(|_| invalid("private resource directory unavailable"))?;
        feynman_local::private_directory(&runner_root)
            .map_err(|_| invalid("private workload directory unavailable"))?;
        Ok(Self {
            repo,
            workspace,
            access,
            secrets: secrets::Secrets::new(root.join("secrets"))?,
            runner_root,
            lock: tokio::sync::Mutex::new(()),
        })
    }
    pub async fn call(
        &self,
        p: &Principal,
        project: &str,
        agent: Option<&str>,
        op: &str,
        args: Value,
    ) -> PlatformResult<Value> {
        self.repo.get_project(p, project)?;
        if let Some(id) = agent {
            self.repo.resolve_agent(p, project, Some(id))?;
        }
        match op {
            "memory.search" | "memory.get" | "memory.history" => {
                self.memory_read(p, project, agent, op, args)
            }
            "memory.put" | "memory.confirm" | "memory.delete" => {
                self.memory_write(p, project, agent, op, args)
            }
            "profile.list" => Ok(json!({"profiles":self.repo.resources(p,project,"profile")?})),
            "secret.list" if agent.is_none() => {
                Ok(json!({"secrets":self.repo.resources(p,project,"secret")?}))
            }
            "profile.put" | "secret.put" | "env.approve" | "env.deny" | "lease.revoke"
            | "lease.renew" | "env.run" | "env.request" | "env.status" | "lease.list"
            | "workload.logs" => {
                let _guard = self.lock.lock().await;
                self.environment_call(p, project, agent, op, args).await
            }
            _ => Err(PlatformError::Unauthorized),
        }
    }
    fn reject_secrets(&self, p: &Principal, project: &str, text: &str) -> PlatformResult<()> {
        // Best effort, not a guarantee against transformed/fragmented disclosures by a trusted Agent.
        for record in self.repo.resources(p, project, "secret")? {
            for revision in 1..=record.revision {
                let meta: SecretMetadata = decode(
                    self.repo
                        .resource(p, project, "secret", &record.id, Some(revision))?
                        .data,
                )?;
                let value = self.secrets.read(project, &meta)?;
                let encoded = serde_json::to_string(&value)
                    .map_err(|_| invalid("invalid secret encoding"))?;
                let escaped = &encoded[1..encoded.len() - 1];
                if value.len() >= 4 && (text.contains(&value) || text.contains(escaped)) {
                    return Err(invalid(
                        "content contains a registered secret; use a reference instead",
                    ));
                }
            }
        }
        Ok(())
    }
    pub fn source_allowed(
        &self,
        p: &Principal,
        project: &str,
        source_id: &str,
        agent: Option<&str>,
    ) -> PlatformResult<()> {
        let source = self.repo.source(p, project, source_id)?;
        if agent.is_none() && source.kind == "manual" {
            return Ok(());
        }
        if agent.is_some_and(|id| id != source.agent_id) {
            return Err(PlatformError::Unauthorized);
        }
        self.repo
            .resolve_agent(p, project, Some(&source.agent_id))?;
        if source.kind.starts_with("dws_") {
            self.access.validate_dws_source(
                p,
                project,
                source
                    .connection_id
                    .as_deref()
                    .ok_or(PlatformError::Unauthorized)?,
                source.arguments,
            )?;
            if source.kind == "dws_events" {
                self.repo
                    .check_agent_tool(p, project, "dws.event.messages.listen")?;
            }
        }
        Ok(())
    }
    pub async fn tick(&self) -> PlatformResult<()> {
        let _guard = self.lock.lock().await;
        // No issuing on restart/tick: only inspect, renew within an existing approval, or stop.
        for owner in self.repo.resource_owners()? {
            for project in self.repo.list_projects(&owner)? {
                for lease in self.repo.resources(&owner, &project.id, "lease")? {
                    if matches!(
                        lease.data["state"].as_str(),
                        Some("active" | "starting" | "revoking" | "unknown")
                    ) && self.reconcile(&owner, &project.id, lease).await.is_err()
                    {
                        tracing::warn!("workload reconciliation pending; no automatic re-issue");
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn prepare_payload(mut payload: Value) -> Value {
    payload["_runner_code"] = json!(RUNNER);
    if payload["env"].is_object() {
        // Only registered secrets are passed separately for output redaction by the launcher.
        if payload["secret_values"].is_null() {
            payload["secret_values"] = json!([]);
        }
    }
    payload
}
pub async fn run_control(
    _unused: Option<&str>,
    action: &str,
    payload: Value,
) -> PlatformResult<Value> {
    let mut command = tokio::process::Command::new("python3");
    command.args(["-c", RUNNER, action]);
    bounded_control(command, prepare_payload(payload)).await
}
pub async fn run_container_control(target: Option<&str>, payload: Value) -> PlatformResult<Value> {
    let script = include_str!("remote.py");
    let command = if let Some(target) = target {
        let script = script.replace('\'', "'\"'\"'");
        let mut command = tokio::process::Command::new("ssh");
        command.args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=2",
            target,
            &format!("python3 -c '{script}'"),
        ]);
        command
    } else {
        // Operator-selected local Docker uses the same ownership-checked transport,
        // never the host workload runner or a fallback after an SSH failure.
        let mut command = tokio::process::Command::new("python3");
        command.args(["-c", script]);
        command
    };
    bounded_control(command, payload).await
}
async fn bounded_control(
    mut command: tokio::process::Command,
    payload: Value,
) -> PlatformResult<Value> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    // Do not pass model/application secrets from the Server's inherited environment.
    let safe: Vec<_> = ["PATH", "HOME", "SSH_AUTH_SOCK"]
        .iter()
        .filter_map(|k| std::env::var_os(k).map(|v| (*k, v)))
        .collect();
    command.env_clear().envs(safe);
    let mut child = command
        .spawn()
        .map_err(|_| PlatformError::Runtime("workload transport unavailable".into()))?;
    let bytes = serde_json::to_vec(&payload).map_err(|_| invalid("invalid workload payload"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| invalid("workload stdin unavailable"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid("workload stdout unavailable"))?
        .take(65537);
    let seconds = payload
        .get("control_timeout")
        .and_then(Value::as_u64)
        .unwrap_or(12)
        .clamp(1, 120)
        + 8;
    let result = tokio::time::timeout(std::time::Duration::from_secs(seconds), async {
        stdin.write_all(&bytes).await?;
        drop(stdin);
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).await?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, output))
    })
    .await
    .map_err(|_| PlatformError::Runtime("workload transport outcome unknown".into()))?
    .map_err(|_| PlatformError::Runtime("workload transport failed".into()))?;
    if !result.0.success() || result.1.len() > 65536 {
        return Err(PlatformError::Runtime(
            "workload transport outcome unknown".into(),
        ));
    }
    serde_json::from_slice(&result.1)
        .map_err(|_| PlatformError::Runtime("invalid workload transport response".into()))
}
