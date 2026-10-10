//! DWS submodule device orchestration. Public metadata never contains credentials.
//! DWS owns the OAuth/device polling protocol; this module owns process lifecycle and project policy.
use crate::store::SqliteRepository;
use chrono::{Duration as ChronoDuration, Utc};
use feynman_core::{
    DwsAuthorization, DwsAuthorizationStatus as Status, DwsIdentity, PlatformError,
    PlatformRepository, PlatformResult, Principal, opaque_id,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, sync::Mutex, task::JoinHandle};

pub const TOOL_ID: &str = "dws.auth.device";
const MAX_OUTPUT: usize = 65_536;
const MAX_WAIT_SECONDS: i64 = 600;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationRequest {
    pub reason: String,
}

pub struct DwsAuthorizationService {
    repository: Arc<SqliteRepository>,
    credential_root: PathBuf,
    executable: OsString,
    requests: Mutex<()>,
    checks: Mutex<()>,
    jobs: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl DwsAuthorizationService {
    pub fn new(
        repository: Arc<SqliteRepository>,
        credential_root: PathBuf,
        executable: OsString,
    ) -> PlatformResult<Arc<Self>> {
        private_directory(&credential_root)?;
        let credential_root = std::fs::canonicalize(credential_root)
            .map_err(|_| PlatformError::Storage("resolve private DWS storage".into()))?;
        Ok(Arc::new(Self {
            repository,
            credential_root,
            executable,
            requests: Mutex::new(()),
            checks: Mutex::new(()),
            jobs: Mutex::new(HashMap::new()),
        }))
    }

    pub async fn request(
        self: &Arc<Self>,
        principal: &Principal,
        project_id: &str,
        request: AuthorizationRequest,
    ) -> PlatformResult<DwsAuthorization> {
        self.ensure_granted(principal, project_id)?;
        if request.reason.trim().is_empty() || request.reason.chars().count() > 512 {
            return Err(PlatformError::InvalidInput(
                "reason must contain 1–512 characters".into(),
            ));
        }
        let _serial = self.requests.lock().await;
        if let Some(active) = self
            .list(principal, project_id)?
            .into_iter()
            .find(|item| item.status.is_pending())
        {
            return Ok(active);
        }
        let mut jobs = self.jobs.lock().await;
        jobs.retain(|_, job| !job.is_finished());
        if jobs.len() >= 16 {
            return Err(PlatformError::Conflict(
                "too many pending authorizations".into(),
            ));
        }
        let now = Utc::now();
        let record = DwsAuthorization {
            id: opaque_id("dws_auth"),
            project_id: project_id.into(),
            reason: request.reason.trim().into(),
            status: Status::Preparing,
            verification_uri: None,
            verification_uri_complete: None,
            user_code: None,
            expires_at: now + ChronoDuration::seconds(MAX_WAIT_SECONDS),
            identity: None,
            error_code: None,
            created_at: now,
            updated_at: now,
            checked_at: None,
        };
        private_directory(&self.home(&record))?;
        self.repository
            .insert_dws_authorization(principal, &record)?;
        let service = self.clone();
        let principal = principal.clone();
        let job_record = record.clone();
        let handle = tokio::spawn(async move {
            if let Err(code) = service.login(&principal, job_record.clone()).await {
                // No CLI stderr/stdout or provider error text is copied to logs or public state.
                if let Ok(current) = service.get(&principal, &job_record.project_id, &job_record.id)
                    && current.status.is_pending()
                {
                    let status = match code {
                        "DEVICE_CODE_EXPIRED" => Status::Expired,
                        "USER_DENIED" => Status::Denied,
                        "TOOL_GRANT_REVOKED" | "USER_CANCELLED" => Status::Cancelled,
                        _ => Status::Failed,
                    };
                    let _ = service.transition(&principal, current, status, Some(code));
                }
            }
        });
        jobs.insert(record.id.clone(), handle);
        Ok(record)
    }

    pub fn list(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<Vec<DwsAuthorization>> {
        self.repository
            .list_dws_authorizations(principal, project_id)
    }

    pub fn get(
        &self,
        principal: &Principal,
        project_id: &str,
        id: &str,
    ) -> PlatformResult<DwsAuthorization> {
        self.repository
            .get_dws_authorization(principal, project_id, id)
    }

    pub async fn cancel(
        &self,
        principal: &Principal,
        project_id: &str,
        id: &str,
    ) -> PlatformResult<DwsAuthorization> {
        let record = self.get(principal, project_id, id)?;
        if !record.status.is_pending() {
            return Ok(record);
        }
        self.transition(principal, record, Status::Cancelled, Some("USER_CANCELLED"))?;
        if let Some(job) = self.jobs.lock().await.remove(id) {
            job.abort();
            let _ = job.await;
        }
        self.get(principal, project_id, id)
    }

    /// A fresh check (and refresh when DWS requires it), not just a historical successful login.
    pub async fn check(
        &self,
        principal: &Principal,
        project_id: &str,
        id: &str,
    ) -> PlatformResult<DwsAuthorization> {
        self.ensure_granted(principal, project_id)?;
        // Avoid concurrent refresh-token rotation by two clients of the local MVP.
        let _check = self.checks.lock().await;
        self.check_locked(principal, project_id, id).await
    }

    async fn check_locked(
        &self,
        principal: &Principal,
        project_id: &str,
        id: &str,
    ) -> PlatformResult<DwsAuthorization> {
        let mut record = self.get(principal, project_id, id)?;
        if !matches!(
            record.status,
            Status::Authorized | Status::AuthenticationRequired
        ) {
            return Ok(record);
        }
        let expected = record.status.clone();
        let identity = self.inspect_identity(&record).await.map_err(|_| {
            PlatformError::Runtime("DWS authentication check failed; retry later".into())
        })?;
        record.status = if identity.is_some() {
            Status::Authorized
        } else {
            Status::AuthenticationRequired
        };
        let authenticated = identity.is_some();
        if authenticated {
            record.identity = identity;
        }
        record.error_code = if authenticated {
            None
        } else {
            Some("AUTHENTICATION_REQUIRED".into())
        };
        record.checked_at = Some(Utc::now());
        record.updated_at = Utc::now();
        self.repository
            .update_dws_authorization(principal, &record, &expected)?;
        self.get(principal, project_id, id)
    }

    pub async fn close(&self) -> PlatformResult<()> {
        let jobs = std::mem::take(&mut *self.jobs.lock().await);
        for (_, job) in jobs {
            job.abort();
            let _ = job.await;
        }
        self.repository.interrupt_dws_authorizations()
    }

    pub async fn verified_home(
        &self,
        principal: &Principal,
        project: &str,
        id: &str,
    ) -> PlatformResult<(PathBuf, String)> {
        let record = self.check(principal, project, id).await?;
        self.verified_record_home(&record)
    }

    /// Internal Broker use only: hold the same refresh lock through the search.
    /// The Broker has already checked the calling project's business grant and binding;
    /// a shared Connection must not depend on another project's device-start grant.
    pub(in crate::external_access) async fn lock_verified_home(
        &self,
        principal: &Principal,
        origin: &str,
        id: &str,
    ) -> PlatformResult<(tokio::sync::MutexGuard<'_, ()>, PathBuf, String)> {
        let guard = self.checks.lock().await;
        let record = self.check_locked(principal, origin, id).await?;
        let (home, identity) = self.verified_record_home(&record)?;
        Ok((guard, home, identity))
    }

    fn verified_record_home(&self, record: &DwsAuthorization) -> PlatformResult<(PathBuf, String)> {
        if record.status != Status::Authorized {
            return Err(PlatformError::Conflict(
                "DWS authentication is required".into(),
            ));
        }
        let identity = record
            .identity
            .as_ref()
            .ok_or_else(|| PlatformError::Conflict("DWS identity unavailable".into()))?;
        Ok((
            self.home(record),
            format!("{}:{}", identity.corp_id, identity.user_id),
        ))
    }

    fn ensure_granted(&self, principal: &Principal, project_id: &str) -> PlatformResult<()> {
        self.repository.get_project(principal, project_id)?;
        self.repository
            .check_agent_tool(principal, project_id, TOOL_ID)?;
        Ok(())
    }

    fn home(&self, record: &DwsAuthorization) -> PathBuf {
        self.credential_root.join(&record.id)
    }

    fn command(&self, record: &DwsAuthorization) -> Command {
        let root = self.home(record);
        let mut command = Command::new(&self.executable);
        // Do not inherit personal DWS profile, OAuth app secrets, or debug configuration.
        command.env_clear();
        for key in [
            "PATH",
            "HTTPS_PROXY",
            "HTTP_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "https_proxy",
            "http_proxy",
            "all_proxy",
            "no_proxy",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "TMPDIR",
            "SYSTEMROOT",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .env("DWS_CONFIG_DIR", root.join("config"))
            .env("DWS_KEYCHAIN_DIR", root.join("keychain"))
            .env("DWS_DISABLE_KEYCHAIN", "1")
            .env("DWS_LANG", "zh-CN")
            .env("NO_COLOR", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        command
    }

    fn transition(
        &self,
        principal: &Principal,
        mut record: DwsAuthorization,
        status: Status,
        code: Option<&str>,
    ) -> PlatformResult<()> {
        let expected = record.status.clone();
        record.status = status;
        record.error_code = code.map(str::to_owned);
        record.updated_at = Utc::now();
        if !record.status.is_pending() {
            record.verification_uri = None;
            record.verification_uri_complete = None;
            record.user_code = None;
        }
        self.repository
            .update_dws_authorization(principal, &record, &expected)?;
        Ok(())
    }

    async fn login(
        &self,
        principal: &Principal,
        mut record: DwsAuthorization,
    ) -> Result<(), &'static str> {
        let mut child = self
            .command(&record)
            .args([
                "auth",
                "login",
                "--device",
                "--no-browser",
                "--format",
                "json",
            ])
            .spawn()
            .map_err(|_| "DWS_START_FAILED")?;
        let mut stdout = child.stdout.take().ok_or("DWS_START_FAILED")?;
        let mut stderr = child.stderr.take().ok_or("DWS_START_FAILED")?;
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        let mut out_buf = [0u8; 4096];
        let mut err_buf = [0u8; 4096];
        let mut out_done = false;
        let mut err_done = false;
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        while !out_done || !err_done {
            tokio::select! {
                result = stdout.read(&mut out_buf), if !out_done => {
                    let n = result.map_err(|_| "DWS_IO_FAILED")?;
                    out_done = n == 0;
                    if output.len() + n > MAX_OUTPUT { return Err("DWS_OUTPUT_TOO_LARGE"); }
                    output.extend_from_slice(&out_buf[..n]);
                }
                result = stderr.read(&mut err_buf), if !err_done => {
                    let n = result.map_err(|_| "DWS_IO_FAILED")?;
                    err_done = n == 0;
                    if diagnostics.len() + n > MAX_OUTPUT { return Err("DWS_OUTPUT_TOO_LARGE"); }
                    diagnostics.extend_from_slice(&err_buf[..n]);
                    if let Some(challenge) = challenge_from_output(&diagnostics)? {
                        let current = self.get(principal, &record.project_id, &record.id).map_err(|_| "STORE_FAILED")?;
                        if !current.status.is_pending() { return Ok(()); }
                        if current.user_code.as_ref() != Some(&challenge.user_code) {
                            let expected = current.status.clone();
                            record = current;
                            record.status = Status::WaitingForUser;
                            record.verification_uri = Some(challenge.uri);
                            record.verification_uri_complete = challenge.complete_uri;
                            record.user_code = Some(challenge.user_code);
                            record.expires_at = std::cmp::min(record.created_at + ChronoDuration::seconds(MAX_WAIT_SECONDS), Utc::now() + ChronoDuration::seconds(challenge.expires_seconds));
                            record.updated_at = Utc::now();
                            self.repository.update_dws_authorization(principal, &record, &expected).map_err(|_| "STORE_FAILED")?;
                        }
                    }
                }
                _ = tick.tick() => {
                    let current = self.get(principal, &record.project_id, &record.id).map_err(|_| "STORE_FAILED")?;
                    if !current.status.is_pending() { return Ok(()); }
                    if self.ensure_granted(principal, &record.project_id).is_err() { return Err("TOOL_GRANT_REVOKED"); }
                    if Utc::now() >= current.expires_at { return Err("DEVICE_CODE_EXPIRED"); }
                }
            }
        }
        let deadline = (record.expires_at - Utc::now())
            .to_std()
            .map_err(|_| "DEVICE_CODE_EXPIRED")?;
        let exit = tokio::time::timeout(deadline, child.wait())
            .await
            .map_err(|_| "DEVICE_CODE_EXPIRED")?
            .map_err(|_| "DWS_IO_FAILED")?;
        if !exit.success() {
            return Err(classify_failure(&diagnostics));
        }
        let result: Value = serde_json::from_slice(&output).map_err(|_| "DWS_INVALID_RESPONSE")?;
        if result.get("success").and_then(Value::as_bool) != Some(true) {
            return Err("DWS_LOGIN_FAILED");
        }
        // A zero exit and login message are not sufficient: independently verify stored authentication.
        let identity = self
            .inspect_identity(&record)
            .await?
            .ok_or("DWS_IDENTITY_NOT_VERIFIED")?;
        self.ensure_granted(principal, &record.project_id)
            .map_err(|_| "TOOL_GRANT_REVOKED")?;
        let mut current = self
            .get(principal, &record.project_id, &record.id)
            .map_err(|_| "STORE_FAILED")?;
        if !current.status.is_pending() {
            return Ok(());
        }
        current.identity = Some(identity);
        current.checked_at = Some(Utc::now());
        self.transition(principal, current, Status::Authorized, None)
            .map_err(|_| "STORE_FAILED")
    }

    async fn inspect_identity(
        &self,
        record: &DwsAuthorization,
    ) -> Result<Option<DwsIdentity>, &'static str> {
        let mut command = self.command(record);
        command.args(["auth", "status", "--format", "json"]);
        if let Some(identity) = &record.identity {
            command.args([
                "--profile",
                &format!("{}:{}", identity.corp_id, identity.user_id),
            ]);
        }
        let mut child = command.spawn().map_err(|_| "DWS_START_FAILED")?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or("DWS_START_FAILED")?
            .take(MAX_OUTPUT as u64 + 1);
        let mut stderr = child
            .stderr
            .take()
            .ok_or("DWS_START_FAILED")?
            .take(MAX_OUTPUT as u64 + 1);
        let work = async {
            let mut bytes = Vec::new();
            let mut ignored = Vec::new();
            let (out, err, exit) = tokio::join!(
                stdout.read_to_end(&mut bytes),
                stderr.read_to_end(&mut ignored),
                child.wait()
            );
            out.map_err(|_| "DWS_IO_FAILED")?;
            err.map_err(|_| "DWS_IO_FAILED")?;
            if bytes.len() > MAX_OUTPUT || ignored.len() > MAX_OUTPUT {
                return Err("DWS_OUTPUT_TOO_LARGE");
            }
            if !exit.map_err(|_| "DWS_IO_FAILED")?.success() {
                return Err("DWS_CHECK_FAILED");
            }
            let data: Value = serde_json::from_slice(&bytes).map_err(|_| "DWS_INVALID_RESPONSE")?;
            if data.get("authenticated").and_then(Value::as_bool) == Some(false) {
                return Ok(None);
            }
            if data.get("authenticated").and_then(Value::as_bool) != Some(true)
                || data.get("token_valid").and_then(Value::as_bool) != Some(true)
            {
                return Err("DWS_IDENTITY_NOT_VERIFIED");
            }
            let string = |key: &str| {
                data.get(key)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty() && value.len() <= 512)
                    .map(str::to_owned)
            };
            let identity = DwsIdentity {
                corp_id: string("corp_id").ok_or("DWS_IDENTITY_NOT_VERIFIED")?,
                user_id: string("user_id").ok_or("DWS_IDENTITY_NOT_VERIFIED")?,
                corp_name: string("corp_name"),
                user_name: string("user_name"),
                token_expires_at: string("expires_at")
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
                    .map(|value| value.with_timezone(&Utc)),
                refresh_expires_at: string("refresh_expires_at")
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
                    .map(|value| value.with_timezone(&Utc)),
            };
            if let Some(expected) = &record.identity
                && (identity.corp_id != expected.corp_id || identity.user_id != expected.user_id)
            {
                return Err("DWS_ACCOUNT_CHANGED");
            }
            Ok(Some(identity))
        };
        tokio::time::timeout(Duration::from_secs(35), work)
            .await
            .map_err(|_| "DWS_CHECK_TIMEOUT")?
    }
}

fn private_directory(path: &Path) -> PlatformResult<()> {
    std::fs::create_dir_all(path)
        .map_err(|_| PlatformError::Storage("create private DWS storage".into()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| PlatformError::Storage("secure private DWS storage".into()))?;
    }
    Ok(())
}

struct Challenge {
    uri: String,
    complete_uri: Option<String>,
    user_code: String,
    expires_seconds: i64,
}

fn challenge_from_output(bytes: &[u8]) -> Result<Option<Challenge>, &'static str> {
    static ANSI: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap());
    static CODE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:授权码|User code|Code):\s*([A-Za-z0-9-]{4,64})\s").unwrap()
    });
    static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https://[^\s│]+\s").unwrap());
    static EXPIRY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"授权码将在\s*(\d+)\s*秒").unwrap());
    let text = ANSI
        .replace_all(&String::from_utf8_lossy(bytes), "")
        .into_owned();
    // Only the device-code box, not URLs in later CLI diagnostics, is eligible for presentation.
    let Some(box_start) = text.rfind("请在浏览器中打开以下链接") else {
        return Ok(None);
    };
    let box_text = &text[box_start..];
    let Some(box_end) = box_text.find('╰') else {
        return Ok(None);
    };
    let box_text = &box_text[..box_end];
    let Some(code) = CODE.captures(box_text) else {
        return Ok(None);
    };
    let urls: Vec<_> = URL
        .find_iter(box_text)
        .map(|item| item.as_str().trim().to_owned())
        .collect();
    let Some(uri) = urls.first() else {
        return Ok(None);
    };
    for uri in &urls {
        validate_verification_uri(uri)?;
    }
    let seconds = EXPIRY
        .captures(box_text)
        .and_then(|item| item[1].parse::<i64>().ok())
        .unwrap_or(MAX_WAIT_SECONDS)
        .clamp(1, MAX_WAIT_SECONDS);
    Ok(Some(Challenge {
        uri: uri.clone(),
        complete_uri: urls.get(1).cloned(),
        user_code: code[1].to_owned(),
        expires_seconds: seconds,
    }))
}

fn validate_verification_uri(value: &str) -> Result<(), &'static str> {
    let url = reqwest::Url::parse(value).map_err(|_| "UNTRUSTED_VERIFICATION_URI")?;
    let host = url.host_str().ok_or("UNTRUSTED_VERIFICATION_URI")?;
    if value.len() > 4096
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port().is_some_and(|port| port != 443)
        || !(host == "dingtalk.com"
            || host.ends_with(".dingtalk.com")
            || host == "dingtalk.io"
            || host.ends_with(".dingtalk.io"))
        || url.query_pairs().any(|(name, _)| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                "token"
                    | "accesstoken"
                    | "access_token"
                    | "refreshtoken"
                    | "refresh_token"
                    | "devicecode"
                    | "device_code"
                    | "authcode"
                    | "auth_code"
                    | "authorization_code"
                    | "id_token"
                    | "clientsecret"
                    | "client_secret"
            )
        })
    {
        return Err("UNTRUSTED_VERIFICATION_URI");
    }
    Ok(())
}

fn classify_failure(bytes: &[u8]) -> &'static str {
    let text = String::from_utf8_lossy(bytes);
    if text.contains("已过期") || text.contains("EXPIRED") {
        "DEVICE_CODE_EXPIRED"
    } else if text.contains("拒绝") || text.contains("REJECTED") {
        "USER_DENIED"
    } else if text.contains("取消") || text.contains("CANCELLED") {
        "USER_CANCELLED"
    } else {
        "DWS_LOGIN_FAILED"
    }
}
