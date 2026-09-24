use async_trait::async_trait;
use feynman_core::{
    AgentRunStatus, AgentRuntime, AgentRuntimeEvent, AgentTurnInput, AgentTurnStarted,
    PlatformError, PlatformResult, Principal,
};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc, oneshot};

pub struct CodexAppServerRuntime {
    sessions: Mutex<HashMap<String, Arc<CodexSession>>>,
    database_path: PathBuf,
}

impl CodexAppServerRuntime {
    pub fn new(database_path: impl Into<PathBuf>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            database_path: database_path.into(),
        }
    }
}

struct CodexSession {
    project_id: String,
    session_id: String,
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    pending: Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>,
    active_turn: Mutex<Option<ActiveTurn>>,
    thread_id: Mutex<Option<String>>,
    approvals: Mutex<HashMap<String, Value>>,
    next_id: AtomicU64,
    alive: std::sync::atomic::AtomicBool,
    initialized: std::sync::atomic::AtomicBool,
    init_lock: Mutex<()>,
    thread_lock: Mutex<()>,
}

struct ActiveTurn {
    thread_id: String,
    turn_id: Option<String>,
    events: mpsc::Sender<AgentRuntimeEvent>,
}

#[async_trait]
impl AgentRuntime for CodexAppServerRuntime {
    async fn start_turn(
        &self,
        input: AgentTurnInput,
    ) -> PlatformResult<(AgentTurnStarted, mpsc::Receiver<AgentRuntimeEvent>)> {
        let session = {
            let mut sessions = self.sessions.lock().await;
            if let Some(session) = sessions.get(&input.project.id) {
                if session.session_id != input.session.id || !session.alive.load(Ordering::Acquire)
                {
                    sessions.remove(&input.project.id);
                }
            }
            if let Some(session) = sessions.get(&input.project.id) {
                session.clone()
            } else {
                let session = CodexSession::spawn(
                    input.project.id.clone(),
                    input.session.id.clone(),
                    input.principal.clone(),
                    &self.database_path,
                    &input.workspace_path,
                )
                .await?;
                sessions.insert(input.project.id.clone(), session.clone());
                session
            }
        };

        session.initialize().await?;
        let thread_id = session.ensure_thread(&input).await?;
        let (event_tx, event_rx) = mpsc::channel(256);
        {
            let mut active = session.active_turn.lock().await;
            if active.is_some() {
                return Err(PlatformError::RunAlreadyActive);
            }
            *active = Some(ActiveTurn {
                thread_id: thread_id.clone(),
                turn_id: None,
                events: event_tx,
            });
        }
        let response = session
            .request(
                "turn/start",
                json!({
                    "threadId": thread_id,
                    "cwd": input.workspace_path,
                    "input": [{"type":"text", "text":input.message, "text_elements":[]}],
                    "approvalPolicy":"onRequest",
                    "sandboxPolicy":codex_sandbox_policy(&input.workspace_path)
                }),
            )
            .await;
        let result = match response {
            Ok(result) => result,
            Err(error) => {
                *session.active_turn.lock().await = None;
                return Err(error);
            }
        };
        let turn_id = result
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                PlatformError::Runtime("Codex App Server did not return a turn id".into())
            })?
            .to_owned();
        if let Some(active) = session.active_turn.lock().await.as_mut() {
            active.turn_id = Some(turn_id.clone());
        }
        Ok((
            AgentTurnStarted {
                session_id: input.session.id,
                thread_id,
                turn_id,
            },
            event_rx,
        ))
    }

    async fn respond_to_approval(
        &self,
        project_id: &str,
        request_id: &str,
        decision: &str,
    ) -> PlatformResult<()> {
        let session = self
            .sessions
            .lock()
            .await
            .get(project_id)
            .cloned()
            .ok_or(PlatformError::NotFound)?;
        let rpc_id = session
            .approvals
            .lock()
            .await
            .remove(request_id)
            .ok_or(PlatformError::NotFound)?;
        session
            .send(json!({"id":rpc_id,"result":{"decision":decision}}))
            .await
    }

    async fn close(&self) -> PlatformResult<()> {
        let sessions = std::mem::take(&mut *self.sessions.lock().await);
        for (_, session) in sessions {
            let mut child = session.child.lock().await;
            let _ = child.kill().await;
        }
        Ok(())
    }
}

impl CodexSession {
    async fn spawn(
        project_id: String,
        session_id: String,
        principal: Principal,
        database_path: &Path,
        cwd: &str,
    ) -> PlatformResult<Arc<Self>> {
        let command = std::env::var("FEYNMAN_CODEX_BIN").unwrap_or_else(|_| "codex".into());
        let mcp_executable = std::env::current_exe().map_err(|error| {
            PlatformError::Runtime(format!("resolve Feynman MCP executable: {error}"))
        })?;
        let overrides =
            mcp_config_overrides(&mcp_executable, database_path, &principal, &project_id)?;
        let mut codex = Command::new(command);
        for value in overrides {
            codex.arg("--config").arg(value);
        }
        let mut child = codex
            .arg("app-server")
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| PlatformError::Runtime(format!("launch Codex App Server: {error}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| PlatformError::Runtime("Codex stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PlatformError::Runtime("Codex stdout unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| PlatformError::Runtime("Codex stderr unavailable".into()))?;
        let session = Arc::new(Self {
            project_id,
            session_id,
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            active_turn: Mutex::new(None),
            thread_id: Mutex::new(None),
            approvals: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            alive: std::sync::atomic::AtomicBool::new(true),
            initialized: std::sync::atomic::AtomicBool::new(false),
            init_lock: Mutex::new(()),
            thread_lock: Mutex::new(()),
        });

        let reader_session = session.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) if line.trim().is_empty() => continue,
                    Ok(Some(line)) => match serde_json::from_str::<Value>(&line) {
                        Ok(message) => reader_session.handle_message(message).await,
                        Err(error) => tracing::warn!(%error, "invalid JSONL from Codex App Server"),
                    },
                    Ok(None) => break,
                    Err(error) => {
                        reader_session
                            .fail_pending(format!("Codex App Server stdout failed: {error}"))
                            .await;
                        break;
                    }
                }
            }
            reader_session
                .fail_pending("Codex App Server exited".into())
                .await;
        });
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(_line)) = lines.next_line().await {
                // Drain stderr to prevent a blocked child, but do not log provider diagnostics.
            }
        });
        Ok(session)
    }

    async fn initialize(&self) -> PlatformResult<()> {
        let _guard = self.init_lock.lock().await;
        if self.initialized.load(Ordering::Acquire) {
            return Ok(());
        }
        self.request(
            "initialize",
            json!({
                    "clientInfo":{"name":"feynman_rust","title":"Feynman Server","version":"0.1.0"},
                    "capabilities":{"experimentalApi":true}
            }),
        )
        .await?;
        self.send(json!({"method":"initialized","params":{}}))
            .await?;
        self.initialized.store(true, Ordering::Release);
        Ok(())
    }

    async fn ensure_thread(&self, input: &AgentTurnInput) -> PlatformResult<String> {
        let _guard = self.thread_lock.lock().await;
        if let Some(thread_id) = self.thread_id.lock().await.clone() {
            return Ok(thread_id);
        }
        let result = if let Some(thread_id) = &input.conversation.runtime_thread_id {
            self.request(
                "thread/resume",
                json!({
                    "threadId":thread_id,
                    "cwd":input.workspace_path,
                    "approvalPolicy":"onRequest",
                    "sandbox":"workspaceWrite"
                }),
            )
            .await?
        } else {
            let mut params = json!({
                "cwd":input.workspace_path,
                "approvalPolicy":"onRequest",
                "sandbox":"workspaceWrite",
                "baseInstructions":input.instructions,
                "threadSource":"appServer"
            });
            if let Ok(model) = std::env::var("FEYNMAN_CODEX_MODEL") {
                params["model"] = Value::String(model);
            }
            self.request("thread/start", params).await?
        };
        let thread_id = result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                PlatformError::Runtime("Codex App Server did not return a thread id".into())
            })?
            .to_owned();
        *self.thread_id.lock().await = Some(thread_id.clone());
        Ok(thread_id)
    }

    async fn request(&self, method: &str, params: Value) -> PlatformResult<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);
        if let Err(error) = self
            .send(json!({"id":id,"method":method,"params":params}))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(result) => result
                .map_err(|_| {
                    PlatformError::Runtime("Codex request response channel closed".into())
                })?
                .map_err(PlatformError::Runtime),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(PlatformError::Runtime(format!(
                    "Codex App Server request '{method}' timed out"
                )))
            }
        }
    }

    async fn send(&self, message: Value) -> PlatformResult<()> {
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(format!("{}\n", message).as_bytes())
            .await
            .map_err(|error| {
                PlatformError::Runtime(format!("write to Codex App Server: {error}"))
            })?;
        stdin
            .flush()
            .await
            .map_err(|error| PlatformError::Runtime(error.to_string()))
    }

    async fn handle_message(&self, message: Value) {
        if let Some(id) = message.get("id") {
            let raw_id = id.clone();
            let id = id
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| id.to_string());
            if message.get("method").is_none() {
                if let Some(sender) = self.pending.lock().await.remove(&id) {
                    let result = if let Some(error) = message.get("error") {
                        Err(error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("Codex request failed")
                            .to_owned())
                    } else {
                        Ok(message.get("result").cloned().unwrap_or(Value::Null))
                    };
                    let _ = sender.send(result);
                }
                return;
            }
            self.handle_server_request(raw_id, &message).await;
            return;
        }
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        self.handle_notification(method, &params).await;
    }

    async fn handle_server_request(&self, rpc_id: Value, message: &Value) {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if method == "currentTime/read" {
            let seconds = chrono::Utc::now().timestamp();
            let _ = self
                .send(json!({"id":rpc_id,"result":{"currentTimeAt":seconds}}))
                .await;
            return;
        }
        let (kind, title) = match method {
            "item/commandExecution/requestApproval" => ("command", "Codex 请求批准执行命令"),
            "item/fileChange/requestApproval" => ("file_change", "Codex 请求批准修改工作区文件"),
            "item/permissions/requestApproval" => ("permissions", "Codex 请求扩大运行权限"),
            "item/tool/requestUserInput" => ("user_input", "Codex 请求补充输入"),
            _ => {
                let _ = self.send(json!({"id":rpc_id,"error":{"code":-32601,"message":"Unsupported App Server request"}})).await;
                return;
            }
        };
        let summary = approval_summary(method, title, &message["params"]);
        let request_id = format!(
            "{}:{}",
            self.project_id,
            rpc_id
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| rpc_id.to_string())
        );
        let active = self.active_turn.lock().await;
        if let Some(active) = active.as_ref() {
            self.approvals
                .lock()
                .await
                .insert(request_id.clone(), rpc_id.clone());
            let _ = active
                .events
                .send(AgentRuntimeEvent::ApprovalRequested {
                    request_id,
                    kind: kind.into(),
                    summary,
                })
                .await;
        } else {
            let _ = self.send(json!({
                "id":rpc_id,
                "error":{"code":-32000,"message":"No active Feynman Agent Run can answer this approval request"}
            })).await;
        }
    }

    async fn handle_notification(&self, method: &str, params: &Value) {
        let thread_id = params
            .get("threadId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut active_guard = self.active_turn.lock().await;
        let Some(active) = active_guard.as_mut() else {
            return;
        };
        if active.thread_id != thread_id {
            return;
        }
        let turn_id = params
            .get("turnId")
            .and_then(Value::as_str)
            .or(active.turn_id.as_deref());
        if let (Some(event_turn), Some(active_turn)) = (
            params.get("turnId").and_then(Value::as_str),
            active.turn_id.as_deref(),
        ) {
            if event_turn != active_turn {
                return;
            }
        }
        if method == "turn/started" {
            if let Some(id) = params.pointer("/turn/id").and_then(Value::as_str) {
                active.turn_id = Some(id.to_owned());
            }
            return;
        }
        let event = match method {
            "item/agentMessage/delta" => params.get("delta").and_then(Value::as_str).map(|text| {
                AgentRuntimeEvent::TextDelta {
                    text: text.to_owned(),
                }
            }),
            "item/mcpToolCall/progress" => Some(AgentRuntimeEvent::ToolActivity {
                status: "progress".into(),
                tool: "MCP tool".into(),
                message: params
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }),
            "item/started" | "item/completed" => {
                let item = params.get("item").unwrap_or(&Value::Null);
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
                if item_type == "commandExecution" || item_type == "mcpToolCall" {
                    Some(AgentRuntimeEvent::ToolActivity {
                        status: if method == "item/started" {
                            "started".into()
                        } else {
                            item.get("status")
                                .and_then(Value::as_str)
                                .unwrap_or("completed")
                                .into()
                        },
                        tool: if item_type == "commandExecution" {
                            "Shell command".into()
                        } else {
                            item.get("tool")
                                .and_then(Value::as_str)
                                .unwrap_or("MCP tool")
                                .into()
                        },
                        message: if item_type == "mcpToolCall" {
                            item.get("server")
                                .and_then(Value::as_str)
                                .map(|server| format!("MCP server: {server}"))
                        } else {
                            Some("Command activity".into())
                        },
                    })
                } else {
                    None
                }
            }
            "turn/completed" => {
                let status = params
                    .pointer("/turn/status")
                    .and_then(Value::as_str)
                    .unwrap_or("completed");
                let run_status = match status {
                    "interrupted" => AgentRunStatus::Interrupted,
                    "failed" => AgentRunStatus::Failed,
                    _ => AgentRunStatus::Completed,
                };
                let error = params
                    .pointer("/turn/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let _ = active
                    .events
                    .send(AgentRuntimeEvent::Completed {
                        status: run_status,
                        error,
                    })
                    .await;
                *active_guard = None;
                return;
            }
            _ => None,
        };
        if let Some(event) = event {
            let _ = turn_id;
            let _ = active.events.send(event).await;
        }
    }

    async fn fail_pending(&self, reason: String) {
        self.alive.store(false, Ordering::Release);
        let pending = std::mem::take(&mut *self.pending.lock().await);
        for (_, sender) in pending {
            let _ = sender.send(Err(reason.clone()));
        }
        if let Some(active) = self.active_turn.lock().await.take() {
            let _ = active
                .events
                .send(AgentRuntimeEvent::Completed {
                    status: AgentRunStatus::Failed,
                    error: Some(reason),
                })
                .await;
        }
    }
}

fn approval_summary(method: &str, title: &str, params: &Value) -> String {
    let mut parts = vec![title.to_owned()];
    if method == "item/commandExecution/requestApproval" {
        if let Some(network) = params.get("networkApprovalContext") {
            let protocol = network
                .get("protocol")
                .and_then(Value::as_str)
                .unwrap_or("network");
            let host = network
                .get("host")
                .and_then(Value::as_str)
                .unwrap_or("unknown host");
            parts.push(format!("目标: {protocol}://{host}"));
        } else if let Some(command) = params.get("command") {
            let command = match command {
                Value::String(command) => command.clone(),
                Value::Array(args) => args
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|arg| format!("{arg:?}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => String::new(),
            };
            if !command.is_empty() {
                parts.push(format!("命令: {}", redact_and_limit(&command, 400)));
            }
        }
        if let Some(cwd) = params.get("cwd").and_then(Value::as_str) {
            parts.push(format!("工作目录: {}", limit_text(cwd, 180)));
        }
    }
    if let Some(reason) = params.get("reason").and_then(Value::as_str) {
        parts.push(format!("原因: {}", redact_and_limit(reason, 240)));
    }
    parts.join("\n")
}

fn redact_and_limit(text: &str, max_chars: usize) -> String {
    static SENSITIVE_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?i)([A-Z0-9_-]*(?:password|passwd|token|secret|api[_-]?key|client[_-]?secret|authorization)[A-Z0-9_-]*[\"']?\s*[:=]\s*)(?:\"[^\"]*\"|'[^']*'|[^\s,;&|}]+)"#,
        )
        .expect("valid sensitive assignment regex")
    });
    static SENSITIVE_OPTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?i)(--?(?:password|passwd|token|secret|api[_-]?key|client[_-]?secret|authorization)(?:\s+|=))(?:\"[^\"]*\"|'[^']*'|[^\s,;&|}]+)"#,
        )
        .expect("valid sensitive option regex")
    });
    static BEARER_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]+").expect("valid bearer token regex")
    });

    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let redacted = SENSITIVE_ASSIGNMENT.replace_all(&flattened, "$1[redacted]");
    let redacted = SENSITIVE_OPTION.replace_all(&redacted, "$1[redacted]");
    let redacted = BEARER_TOKEN.replace_all(&redacted, "Bearer [redacted]");
    limit_text(&redacted, max_chars)
}

fn limit_text(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let prefix = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn mcp_config_overrides(
    mcp_executable: &Path,
    database_path: &Path,
    principal: &Principal,
    project_id: &str,
) -> PlatformResult<Vec<String>> {
    fn toml_string(key: &str, value: &str) -> PlatformResult<String> {
        let value = serde_json::to_string(value)
            .map_err(|error| PlatformError::Runtime(error.to_string()))?;
        Ok(format!("{key}={value}"))
    }

    let executable = mcp_executable.to_string_lossy();
    let database = database_path.to_string_lossy();
    let mut overrides = vec![
        toml_string("mcp_servers.feynman_external_tools.command", &executable)?,
        "mcp_servers.feynman_external_tools.args=[\"external-tools-mcp\"]".into(),
        "mcp_servers.feynman_external_tools.enabled=true".into(),
        "mcp_servers.feynman_external_tools.required=true".into(),
        toml_string(
            "mcp_servers.feynman_external_tools.env.FEYNMAN_DB",
            &database,
        )?,
        toml_string(
            "mcp_servers.feynman_external_tools.env.FEYNMAN_AGENT_USER_ID",
            &principal.user_id,
        )?,
        toml_string(
            "mcp_servers.feynman_external_tools.env.FEYNMAN_AGENT_TENANT_ID",
            &principal.tenant_id,
        )?,
        toml_string(
            "mcp_servers.feynman_external_tools.env.FEYNMAN_AGENT_PROJECT_ID",
            project_id,
        )?,
    ];
    if let Some(dws_binary) = std::env::var_os("FEYNMAN_DWS_BIN") {
        let dws_binary = dws_binary.to_string_lossy();
        overrides.push(toml_string(
            "mcp_servers.feynman_external_tools.env.FEYNMAN_DWS_BIN",
            &dws_binary,
        )?);
    }
    Ok(overrides)
}

fn codex_sandbox_policy(workspace_path: &str) -> Value {
    json!({
        "type":"workspaceWrite",
        "writableRoots":[workspace_path],
        "readOnlyAccess":{
            "type":"restricted",
            "includePlatformDefaults":true,
            "readableRoots":[workspace_path]
        },
        "networkAccess":false
    })
}

#[cfg(test)]
mod tests {
    use super::{approval_summary, codex_sandbox_policy, mcp_config_overrides, redact_and_limit};
    use feynman_core::Principal;
    use std::path::Path;

    #[test]
    fn approval_summary_shows_command_and_redacts_common_secrets() {
        let summary = approval_summary(
            "item/commandExecution/requestApproval",
            "Codex 请求批准执行命令",
            &serde_json::json!({
                "command": "dws profile list --format json API_TOKEN=topsecret --password 'also secret'",
                "cwd": "/tmp/project",
                "reason": "Contact https://example.test with Bearer abc.def"
            }),
        );
        assert!(summary.contains("dws profile list --format json"));
        assert!(summary.contains("API_TOKEN=[redacted]"));
        assert!(summary.contains("--password [redacted]"));
        assert!(summary.contains("Bearer [redacted]"));
        assert!(!summary.contains("topsecret"));
        assert!(!summary.contains("also secret"));
        assert!(!summary.contains("abc.def"));
        assert!(summary.contains("工作目录: /tmp/project"));
    }

    #[test]
    fn network_approval_summary_shows_destination_without_shell_command() {
        let summary = approval_summary(
            "item/commandExecution/requestApproval",
            "Codex 请求批准执行命令",
            &serde_json::json!({
                "command": "an internal implementation detail",
                "networkApprovalContext": {"protocol": "https", "host": "api.example.test"}
            }),
        );
        assert!(summary.contains("目标: https://api.example.test"));
        assert!(!summary.contains("internal implementation"));
    }

    #[test]
    fn approval_summary_flattens_and_limits_untrusted_text() {
        let summary = redact_and_limit("line one\nline two", 8);
        assert_eq!(summary, "line one…");
    }

    #[test]
    fn codex_can_only_read_and_write_the_project_workspace() {
        let policy = codex_sandbox_policy("/tmp/feynman-project");
        assert_eq!(policy["type"], "workspaceWrite");
        assert_eq!(policy["writableRoots"][0], "/tmp/feynman-project");
        assert_eq!(policy["readOnlyAccess"]["type"], "restricted");
        assert_eq!(
            policy["readOnlyAccess"]["readableRoots"][0],
            "/tmp/feynman-project"
        );
        assert_eq!(policy["networkAccess"], false);
    }

    #[test]
    fn feynman_mcp_is_scoped_to_one_project_without_editing_user_config() {
        let principal = Principal {
            user_id: "user-a".into(),
            tenant_id: "tenant-a".into(),
        };
        let config = mcp_config_overrides(
            Path::new("/opt/feynman/feynman-server"),
            Path::new("/var/lib/feynman/platform.sqlite3"),
            &principal,
            "project-a",
        )
        .unwrap();
        assert!(config.iter().any(|value| value.contains("required=true")));
        assert!(
            config
                .iter()
                .any(|value| value.contains("FEYNMAN_AGENT_PROJECT_ID=\"project-a\""))
        );
        assert!(
            config
                .iter()
                .any(|value| value.contains("external-tools-mcp"))
        );
    }
}
