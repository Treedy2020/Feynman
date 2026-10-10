use super::*;
use std::collections::BTreeMap;

fn identifier(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 100
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn variable(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 100
        && !v.as_bytes()[0].is_ascii_digit()
        && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !matches!(
            v,
            "PATH"
                | "HOME"
                | "CODEX_HOME"
                | "CODEX_SQLITE_HOME"
                | "LD_PRELOAD"
                | "LD_LIBRARY_PATH"
                | "PYTHONPATH"
                | "PYTHONHOME"
                | "NODE_OPTIONS"
                | "BASH_ENV"
                | "ENV"
                | "SHELLOPTS"
        )
        && !v.starts_with("FEYNMAN_")
        && !v.starts_with("DYLD_")
}
impl ResourceManager {
    pub(super) async fn environment_call(
        &self,
        p: &Principal,
        project: &str,
        agent: Option<&str>,
        op: &str,
        args: Value,
    ) -> PlatformResult<Value> {
        match op {
            "secret.put" => {
                if agent.is_some() {
                    return Err(PlatformError::Unauthorized);
                }
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    name: String,
                    value: String,
                    id: Option<String>,
                    #[serde(default)]
                    expected_revision: i64,
                }
                let input: Input = decode(args)?;
                if !identifier(&input.name) || input.expected_revision < 0 {
                    return Err(invalid("invalid secret name/revision"));
                }
                let id = input.id.unwrap_or_else(|| opaque_id("sec"));
                match self.repo.resource(p, project, "secret", &id, None) {
                    Ok(current) if current.revision == input.expected_revision => {}
                    Err(PlatformError::NotFound) if input.expected_revision == 0 => {}
                    _ => {
                        return Err(PlatformError::Conflict(
                            "secret revision changed; no existing version was overwritten".into(),
                        ));
                    }
                }
                if input.expected_revision >= 1000 {
                    return Err(invalid("secret version limit; register a new reference"));
                }
                let storage =
                    self.secrets
                        .put(project, &id, input.expected_revision + 1, &input.value)?;
                let meta = SecretMetadata {
                    id: id.clone(),
                    name: input.name,
                    version: input.expected_revision + 1,
                    storage,
                };
                Ok(
                    json!({"secret":self.repo.put_resource(p,project,"secret",&id,input.expected_revision,encode(meta)?)?}),
                )
            }
            "profile.put" => {
                if agent.is_some() {
                    return Err(PlatformError::Unauthorized);
                }
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    profile: EnvironmentProfile,
                    #[serde(default)]
                    expected_revision: i64,
                }
                let input: Input = decode(args)?;
                let profile = input.profile;
                if !identifier(&profile.name)
                    || profile.argv.is_empty()
                    || profile.argv.len() > 64
                    || profile
                        .argv
                        .iter()
                        .any(|s| s.len() > 8192 || s.contains('\0'))
                    || !(1..=86400).contains(&profile.max_seconds)
                    || std::path::Path::new(&profile.cwd).is_absolute()
                    || std::path::Path::new(&profile.cwd)
                        .components()
                        .any(|p| matches!(p, std::path::Component::ParentDir))
                {
                    return Err(invalid("invalid profile; cwd must stay within Workspace"));
                }
                let mut keys = std::collections::HashSet::new();
                for key in profile
                    .variables
                    .keys()
                    .chain(profile.secrets.keys())
                    .chain(profile.secret_files.keys())
                {
                    if !variable(key) || !keys.insert(key) {
                        return Err(invalid("protected or duplicate environment variable"));
                    }
                }
                if keys.len() > 128
                    || profile
                        .variables
                        .values()
                        .any(|v| v.len() > 16384 || v.contains('\0'))
                {
                    return Err(invalid("environment exceeds limits"));
                }
                if profile.variables.keys().any(|k| {
                    let k = k.to_ascii_lowercase();
                    ["token", "password", "secret", "api_key"]
                        .iter()
                        .any(|s| k.contains(s))
                }) {
                    return Err(invalid("sensitive variables require secret references"));
                }
                for id in profile
                    .secrets
                    .values()
                    .chain(profile.secret_files.values())
                {
                    self.repo.resource(p, project, "secret", id, None)?;
                }
                self.reject_secrets(
                    p,
                    project,
                    &serde_json::to_string(&profile).map_err(|_| invalid("invalid profile"))?,
                )?;
                Ok(
                    json!({"profile":self.repo.put_resource(p,project,"profile",&profile.name,input.expected_revision,encode(&profile)?)?}),
                )
            }
            "env.request" => {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    profile: String,
                    reason: String,
                    agent_id: Option<String>,
                    seconds: Option<i64>,
                }
                let input: Input = decode(args)?;
                let requested_agent = if let Some(agent) = agent {
                    if input.agent_id.as_deref().is_some_and(|id| id != agent) {
                        return Err(PlatformError::Unauthorized);
                    }
                    Some(agent)
                } else {
                    input.agent_id.as_deref()
                };
                let a = self.repo.resolve_agent(p, project, requested_agent)?;
                if input.reason.trim().is_empty() || input.reason.len() > 2000 {
                    return Err(invalid("bounded reason required"));
                }
                self.reject_secrets(p, project, &input.reason)?;
                let profile = self
                    .repo
                    .resource(p, project, "profile", &input.profile, None)?;
                let spec: EnvironmentProfile = decode(profile.data.clone())?;
                let seconds = input.seconds.unwrap_or(1800).min(spec.max_seconds);
                if seconds < 1 {
                    return Err(invalid("positive lease duration required"));
                }
                let mut pins = BTreeMap::new();
                for id in spec.secrets.values().chain(spec.secret_files.values()) {
                    pins.insert(
                        id.clone(),
                        decode::<SecretMetadata>(
                            self.repo.resource(p, project, "secret", id, None)?.data,
                        )?,
                    );
                }
                let id = opaque_id("envreq");
                let data = json!({"agent_id":a.id,"profile":profile,"credentials":pins,"seconds":seconds,"reason":input.reason,"state":"pending","auto_renew":false,"created_at":Utc::now()});
                Ok(json!({"request":self.repo.put_resource(p,project,"env_request",&id,0,data)?}))
            }
            "env.approve" | "env.deny" => {
                if agent.is_some() {
                    return Err(PlatformError::Unauthorized);
                }
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    id: String,
                    expected_revision: i64,
                    #[serde(default)]
                    accept_secret_exposure: bool,
                    #[serde(default)]
                    auto_renew: bool,
                    #[serde(default = "approval_seconds")]
                    seconds: i64,
                }
                let input: Input = decode(args)?;
                let mut request = self
                    .repo
                    .resource(p, project, "env_request", &input.id, None)?;
                if request.data["state"] != "pending" {
                    return Err(invalid("only pending requests can be approved/denied"));
                }
                self.repo
                    .resolve_agent(p, project, Some(string(&request.data, "agent_id")?))?;
                if op == "env.approve" {
                    if !(1..=86400).contains(&input.seconds) {
                        return Err(invalid("approval duration must be 1..86400 seconds"));
                    }
                    if !request.data["credentials"]
                        .as_object()
                        .is_some_and(|v| v.is_empty())
                        && !input.accept_secret_exposure
                    {
                        return Err(invalid(
                            "approve must explicitly accept secret exposure to the workload/Agent",
                        ));
                    }
                    request.data["state"] = json!("approved");
                    request.data["authorized_until"] =
                        json!(Utc::now() + Duration::seconds(input.seconds));
                    request.data["auto_renew"] = json!(input.auto_renew);
                } else {
                    request.data["state"] = json!("denied");
                }
                Ok(
                    json!({"request":self.repo.put_resource(p,project,"env_request",&input.id,input.expected_revision,request.data)?}),
                )
            }
            "env.status" | "lease.list" => {
                let kind = if op == "env.status" {
                    "env_request"
                } else {
                    "lease"
                };
                Ok(
                    json!({"items":self.repo.resources(p,project,kind)?.into_iter().filter(|r|agent.is_none_or(|a|r.data["agent_id"]==a)).collect::<Vec<_>>(),"provider_credential_lease":"unsupported","cleanup_capability":"managed_process_group_not_strong_isolation"}),
                )
            }
            "env.run" => {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    request_id: String,
                    idempotency_key: String,
                }
                let input: Input = decode(args)?;
                let uuid = uuid::Uuid::parse_str(&input.idempotency_key)
                    .map_err(|_| invalid("idempotency_key must be a UUID"))?;
                let id = format!("work_{}", uuid.simple());
                match self.repo.resource(p, project, "lease", &id, None) {
                    Ok(record) => {
                        if record.data["request_id"] != input.request_id
                            || agent.is_some_and(|a| record.data["agent_id"] != a)
                        {
                            return Err(PlatformError::Unauthorized);
                        }
                        return Ok(json!({"lease":record,"replayed":false}));
                    }
                    Err(PlatformError::NotFound) => {}
                    Err(e) => return Err(e),
                }
                let request =
                    self.repo
                        .resource(p, project, "env_request", &input.request_id, None)?;
                self.check_request(p, project, agent, &request)?;
                if self
                    .repo
                    .resources(p, project, "lease")?
                    .iter()
                    .any(|lease| {
                        lease.data["request_id"] == request.id
                            && matches!(
                                lease.data["state"].as_str(),
                                Some("starting" | "active" | "unknown" | "revoking")
                            )
                    })
                {
                    return Err(PlatformError::Conflict(
                        "this approval has a live or unresolved workload; reconcile or stop it before a new launch".into(),
                    ));
                }
                let project_record = self.repo.get_project(p, project)?;
                self.workspace.ensure_workspace(p, &project_record).await?;
                let w = self
                    .workspace
                    .get(p, project)?
                    .ok_or(PlatformError::NotFound)?;
                let instance = self.workspace.workload_instance(&w)?;
                let expires = (Utc::now()
                    + Duration::seconds(
                        request.data["seconds"]
                            .as_i64()
                            .ok_or_else(|| invalid("invalid request duration"))?,
                    ))
                .min(time(&request.data, "authorized_until")?);
                let mut data = json!({"kind":"usage_lease","agent_id":request.data["agent_id"],"owner_user":p.user_id,"request_id":request.id,"grant_revision":request.revision,"workspace_id":w.environment_id,"sandbox_instance_id":instance,"workload_id":id,"state":"starting","expires_at":expires,"cleanup":"pending","provider_revocation":"unsupported","profile_revision":request.data["profile"]["revision"],"credential_versions":request.data["credentials"]});
                let mut lease =
                    self.repo
                        .put_resource(p, project, "lease", &id, 0, data.clone())?;
                let spec: EnvironmentProfile = decode(request.data["profile"]["data"].clone())?;
                let mut env = spec.variables;
                let mut files = BTreeMap::new();
                for (name, id) in &spec.secrets {
                    let meta: SecretMetadata = decode(request.data["credentials"][id].clone())?;
                    env.insert(name.clone(), self.secrets.read(project, &meta)?);
                }
                for (name, id) in &spec.secret_files {
                    let meta: SecretMetadata = decode(request.data["credentials"][id].clone())?;
                    files.insert(name.clone(), self.secrets.read(project, &meta)?);
                }
                let mut secret_values = Vec::new();
                for name in spec.secrets.keys() {
                    secret_values.push(env[name].clone());
                }
                secret_values.extend(files.values().cloned());
                let payload = json!({"id":lease.id,"instance":instance,"expires_at":expires.timestamp_millis(),"argv":spec.argv,"cwd":spec.cwd,"env":env,"files":files,"secret_values":secret_values});
                // Recheck after async Workspace preparation and before crossing the execution seam.
                self.check_request(p, project, agent, &request)?;
                let result = self
                    .workspace
                    .workload_control(p, &w, "start", payload, &self.runner_root)
                    .await;
                match result {
                    Ok(status) => {
                        data["state"] = json!(match status["state"].as_str() {
                            Some("running") => "active",
                            Some("completed") => "completed",
                            Some("expired") => "expired",
                            Some("failed") => "failed",
                            _ => "unknown",
                        });
                        data["runtime"] = status;
                    }
                    Err(_) => {
                        data["state"] = json!("unknown");
                        data["cleanup"] = json!("unreachable");
                    }
                }
                lease =
                    self.repo
                        .put_resource(p, project, "lease", &lease.id, lease.revision, data)?;
                Ok(json!({"lease":lease,"replayed":false}))
            }
            "lease.revoke" | "lease.renew" => {
                if agent.is_some() {
                    return Err(PlatformError::Unauthorized);
                }
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    id: String,
                    expected_revision: i64,
                }
                let input: Input = decode(args)?;
                let mut lease = self.repo.resource(p, project, "lease", &input.id, None)?;
                if lease.revision != input.expected_revision {
                    return Err(PlatformError::Conflict("lease revision changed".into()));
                }
                if op == "lease.revoke" {
                    let mut req = self.repo.resource(
                        p,
                        project,
                        "env_request",
                        string(&lease.data, "request_id")?,
                        None,
                    )?;
                    req.data["state"] = json!("revoked");
                    self.repo.put_resource(
                        p,
                        project,
                        "env_request",
                        &req.id,
                        req.revision,
                        req.data,
                    )?;
                    lease.data["state"] = json!("revoking");
                    lease = self.repo.put_resource(
                        p,
                        project,
                        "lease",
                        &lease.id,
                        lease.revision,
                        lease.data,
                    )?;
                } else {
                    if lease.data["state"] != "active"
                        || time(&lease.data, "expires_at")? <= Utc::now()
                    {
                        return Err(invalid("expired/unknown leases cannot be revived"));
                    }
                    self.renew(p, project, &mut lease).await?;
                }
                self.reconcile(p, project, lease).await?;
                Ok(json!({"lease":self.repo.resource(p,project,"lease",&input.id,None)?}))
            }
            "workload.logs" => {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    id: String,
                }
                let input: Input = decode(args)?;
                let lease = self.repo.resource(p, project, "lease", &input.id, None)?;
                if let Some(agent) = agent {
                    self.check_lease(p, project, Some(agent), &lease)?;
                }
                let w = self
                    .workspace
                    .get(p, project)?
                    .ok_or(PlatformError::NotFound)?;
                if self.workspace.workload_instance(&w)? != lease.data["sandbox_instance_id"] {
                    return Err(PlatformError::Conflict("Workspace instance changed".into()));
                }
                self.workspace
                    .workload_control(p, &w, "logs", json!({"id":input.id}), &self.runner_root)
                    .await
            }
            _ => Err(PlatformError::Unauthorized),
        }
    }
    fn check_request(
        &self,
        p: &Principal,
        project: &str,
        agent: Option<&str>,
        r: &ResourceRecord,
    ) -> PlatformResult<()> {
        let current = self.repo.resource(p, project, "env_request", &r.id, None)?;
        if current.revision != r.revision
            || current.data["state"] != "approved"
            || time(&current.data, "authorized_until")? <= Utc::now()
            || agent.is_some_and(|a| current.data["agent_id"] != a)
        {
            return Err(PlatformError::Unauthorized);
        }
        self.repo
            .resolve_agent(p, project, Some(string(&r.data, "agent_id")?))?;
        let profile = self.repo.resource(
            p,
            project,
            "profile",
            string(&r.data["profile"], "id")?,
            None,
        )?;
        if json!(profile.revision) != r.data["profile"]["revision"] {
            return Err(PlatformError::Conflict(
                "Profile changed; request new approval".into(),
            ));
        }
        for (id, meta) in r.data["credentials"]
            .as_object()
            .ok_or_else(|| invalid("invalid credential pins"))?
        {
            if self.repo.resource(p, project, "secret", id, None)?.data["version"]
                != meta["version"]
            {
                return Err(PlatformError::Conflict(
                    "credential rotated; request new approval".into(),
                ));
            }
        }
        Ok(())
    }
    fn check_lease(
        &self,
        p: &Principal,
        project: &str,
        agent: Option<&str>,
        lease: &ResourceRecord,
    ) -> PlatformResult<ResourceRecord> {
        if lease.data["state"] != "active" || time(&lease.data, "expires_at")? <= Utc::now() {
            return Err(PlatformError::Unauthorized);
        }
        let request = self.repo.resource(
            p,
            project,
            "env_request",
            string(&lease.data, "request_id")?,
            None,
        )?;
        if json!(request.revision) != lease.data["grant_revision"] {
            return Err(PlatformError::Unauthorized);
        }
        self.check_request(p, project, agent, &request)?;
        Ok(request)
    }
    async fn renew(
        &self,
        p: &Principal,
        project: &str,
        lease: &mut ResourceRecord,
    ) -> PlatformResult<()> {
        let request = self.check_lease(p, project, None, lease)?;
        let w = self
            .workspace
            .get(p, project)?
            .ok_or(PlatformError::NotFound)?;
        if self.workspace.workload_instance(&w)? != lease.data["sandbox_instance_id"] {
            return Err(PlatformError::Conflict("Workspace instance changed".into()));
        }
        let expires = (Utc::now()
            + Duration::seconds(request.data["seconds"].as_i64().unwrap_or(1800)))
        .min(time(&request.data, "authorized_until")?);
        match self
            .workspace
            .workload_control(
                p,
                &w,
                "renew",
                json!({"id":lease.id,"expires_at":expires.timestamp_millis()}),
                &self.runner_root,
            )
            .await
        {
            Ok(_) => {
                lease.data["expires_at"] = json!(expires);
            }
            Err(_) => {
                lease.data["state"] = json!("unknown");
                lease.data["cleanup"] = json!("unreachable");
            }
        }
        *lease = self.repo.put_resource(
            p,
            project,
            "lease",
            &lease.id,
            lease.revision,
            lease.data.clone(),
        )?;
        Ok(())
    }
    pub(super) async fn reconcile(
        &self,
        p: &Principal,
        project: &str,
        mut lease: ResourceRecord,
    ) -> PlatformResult<()> {
        let original = lease.data.clone();
        let mut stop = lease.data["state"] == "revoking"
            || self.check_lease(p, project, None, &lease).is_err();
        let workspace = match self.workspace.get(p, project)? {
            Some(w) => w,
            None => return Ok(()),
        };
        if workspace.provider == WorkspaceKind::Opensandbox {
            match self.workspace.action(p, project, "refresh").await {
                Ok(w) => {
                    if w.state != "running" {
                        stop = true;
                    }
                }
                Err(_) => {
                    lease.data["cleanup"] = json!("unreachable");
                    self.repo.put_resource(
                        p,
                        project,
                        "lease",
                        &lease.id,
                        lease.revision,
                        lease.data,
                    )?;
                    return Ok(());
                }
            }
        }
        if self.workspace.workload_instance(&workspace)? != lease.data["sandbox_instance_id"] {
            lease.data["state"] = json!("unknown");
            lease.data["cleanup"] = json!("instance_changed");
        } else {
            let action = if stop { "stop" } else { "status" };
            match self
                .workspace
                .workload_control(
                    p,
                    &workspace,
                    action,
                    json!({"id":lease.id}),
                    &self.runner_root,
                )
                .await
            {
                Ok(status) => {
                    let state = status["state"].as_str().unwrap_or("unknown");
                    lease.data["runtime"] = status.clone();
                    if matches!(state, "completed" | "expired" | "stopped" | "failed") {
                        lease.data["state"] = json!(if stop {
                            if time(&lease.data, "expires_at")? <= Utc::now() {
                                "expired"
                            } else {
                                "revoked"
                            }
                        } else {
                            state
                        });
                        lease.data["cleanup"] = json!(if status["cleanup"] == "unknown" {
                            "unknown"
                        } else {
                            "managed_group_stopped"
                        });
                    } else if state == "missing" {
                        lease.data["state"] = json!("unknown");
                        lease.data["cleanup"] = json!("missing");
                    } else if stop {
                        lease.data["state"] = json!("revoking");
                        lease.data["cleanup"] = json!("pending");
                    } else if lease.data["state"] == "active" {
                        let r = self.check_lease(p, project, None, &lease)?;
                        let remaining =
                            (time(&lease.data, "expires_at")? - Utc::now()).num_seconds();
                        if r.data["auto_renew"] == true
                            && remaining < (r.data["seconds"].as_i64().unwrap_or(1800) / 4).max(1)
                            && time(&r.data, "authorized_until")? > time(&lease.data, "expires_at")?
                        {
                            self.renew(p, project, &mut lease).await?;
                            return Ok(());
                        }
                    }
                }
                Err(_) => {
                    lease.data["cleanup"] = json!("unreachable");
                }
            }
        }
        if lease.data != original {
            self.repo
                .put_resource(p, project, "lease", &lease.id, lease.revision, lease.data)?;
        }
        Ok(())
    }
}
fn approval_seconds() -> i64 {
    28800
}
