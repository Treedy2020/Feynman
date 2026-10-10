//! Independent external permission control plane. Provider credentials never enter public DTOs.
mod cli_executor;
pub mod http;
pub mod providers;
pub mod proxy;
pub mod security;
mod store;

use chrono::{Duration, Utc};
use feynman_core::{
    AccessProvider as Provider, CliProviderDescriptor, ExternalAccessRequest, ExternalConnection,
    PlatformError, PlatformRepository, PlatformResult, Principal, ProjectCliBinding, opaque_id,
};
use providers::{AuthenticatedCliProvider, gitlab::GitlabProvider};
use security::{
    AccessPolicy, AuditSink, AuthIntent, AuthenticationStrategy, CredentialDeliveryStrategy,
    CredentialRef, CredentialStore, PolicyDecision, TrustedCall,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use store::{AccessStore, StoredConnection};

pub struct ExternalAccessBroker {
    repository: Arc<dyn PlatformRepository>,
    store: AccessStore,
    dws: Arc<providers::dws::authorization::DwsAuthorizationService>,
    gitlab: GitlabProvider,
    root: PathBuf,
    operations: tokio::sync::Mutex<()>,
    requests: std::sync::Mutex<()>,
    policy: Arc<dyn AccessPolicy>,
    delivery: Arc<dyn CredentialDeliveryStrategy>,
    token_auth: Arc<dyn AuthenticationStrategy>,
    credential_locks:
        std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    workspace: Option<Arc<crate::workspace::ProjectWorkspaceProvider>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterConnection {
    pub provider: Provider,
    pub hostname: Option<String>,
    pub token: Option<String>,
    pub project_id: Option<String>,
    pub authorization_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindRequest {
    pub provider: Provider,
    pub connection_id: String,
    pub repository: Option<String>,
    pub resource_id: Option<String>,
    #[serde(default = "binding_ttl")]
    pub ttl_seconds: i64,
}
fn binding_ttl() -> i64 {
    86400
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessRequestInput {
    pub provider: Provider,
    pub capability: String,
    pub reason: String,
}

impl ExternalAccessBroker {
    pub fn new(
        repository: Arc<dyn PlatformRepository>,
        root: PathBuf,
        dws: Arc<providers::dws::authorization::DwsAuthorizationService>,
    ) -> PlatformResult<Arc<Self>> {
        let store = AccessStore::open(root.clone())?;
        Ok(Arc::new(Self {
            repository,
            store,
            dws,
            gitlab: GitlabProvider {
                root: root.join("glab"),
            },
            root: root.clone(),
            operations: tokio::sync::Mutex::new(()),
            requests: std::sync::Mutex::new(()),
            policy: Arc::new(security::ProjectAccessPolicy),
            delivery: Arc::new(security::ServerProxyDelivery),
            token_auth: Arc::new(security::OwnerTokenAuthentication(GitlabProvider {
                root: root.join("glab"),
            })),
            credential_locks: std::sync::Mutex::new(std::collections::HashMap::new()),
            workspace: None,
        }))
    }
    pub fn providers(&self) -> Vec<CliProviderDescriptor> {
        let mut result = vec![
            CliProviderDescriptor {
                provider: Provider::Dws,
                display_name: "DWS / 钉钉".into(),
                summary: "企业搜索、历史消息读取与托管事件监听。".into(),
                source_kinds: vec!["dws_messages".into(), "dws_events".into()],
                executable: "dws".into(),
                authentication: vec!["device".into()],
                capabilities: vec![
                    "dws.auth.device".into(),
                    "dws.aisearch.enterprise".into(),
                    "dws.chat.messages.read".into(),
                    "dws.event.messages.listen".into(),
                ],
                delivery: "server_proxy".into(),
            },
            self.gitlab.descriptor(),
            CliProviderDescriptor {
                provider: Provider::Git,
                display_name: "Git".into(),
                summary: "读取绑定仓库的分支与标签，受限克隆到项目沙盒。".into(),
                source_kinds: vec![],
                executable: "git".into(),
                authentication: vec!["delegated_gitlab_https".into()],
                capabilities: vec!["git.remote.read".into(), "git.workspace.clone".into()],
                delivery: "server_proxy_fixed_git".into(),
            },
        ];
        result.extend(providers::native::descriptors());
        result
    }
    pub fn with_workspace(
        mut self: Arc<Self>,
        workspace: Arc<crate::workspace::ProjectWorkspaceProvider>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("configure Broker before sharing")
            .workspace = Some(workspace);
        self
    }
    pub fn connections(&self, p: &Principal) -> PlatformResult<Vec<ExternalConnection>> {
        Ok(self
            .store
            .list::<StoredConnection>(p, "connection", None)?
            .into_iter()
            .map(|c| c.public)
            .collect())
    }
    pub async fn register(
        &self,
        p: &Principal,
        input: RegisterConnection,
    ) -> PlatformResult<ExternalConnection> {
        let _serial = self.operations.lock().await;
        let connection_id = opaque_id("connection");
        let (host, identity, source, token) = match input.provider {
            Provider::Gitlab => {
                if input.project_id.is_some() || input.authorization_id.is_some() {
                    return Err(invalid(
                        "GitLab registration only accepts hostname and token",
                    ));
                }
                let host = valid_host(
                    input
                        .hostname
                        .as_deref()
                        .ok_or_else(|| invalid("hostname required"))?,
                )?;
                let token = input
                    .token
                    .ok_or_else(|| invalid("token required via owner stdin"))?;
                if token.len() < 8
                    || token.len() > 8192
                    || token.chars().any(|c| c.is_control() || c.is_whitespace())
                {
                    return Err(invalid("invalid credential format"));
                }
                let verified = self
                    .token_auth
                    .start(
                        p,
                        "",
                        AuthIntent::OwnerToken {
                            hostname: host.clone(),
                            token: token.clone(),
                        },
                    )
                    .await?;
                let identity = verified["identity"]
                    .as_str()
                    .ok_or_else(|| invalid("verified identity required"))?
                    .to_owned();
                (Some(host), identity, None, Some(token))
            }
            Provider::Dws => {
                if input.token.is_some() || input.hostname.is_some() {
                    return Err(invalid(
                        "DWS registration uses an existing verified authorization",
                    ));
                }
                let project = input
                    .project_id
                    .ok_or_else(|| invalid("project_id required"))?;
                let id = input
                    .authorization_id
                    .ok_or_else(|| invalid("authorization_id required"))?;
                let (_, identity) = self.dws.verified_home(p, &project, &id).await?;
                (None, identity, Some((project, id)), None)
            }
            Provider::Git => {
                return Err(invalid(
                    "Git uses an explicitly bound GitLab HTTPS connection; no git login",
                ));
            }
            Provider::Slate | Provider::Drlee => {
                if input.project_id.is_some() || input.authorization_id.is_some() {
                    return Err(invalid(
                        "native registration accepts hostname and private credential input",
                    ));
                }
                let host = valid_host(
                    input
                        .hostname
                        .as_deref()
                        .ok_or_else(|| invalid("hostname required"))?,
                )?;
                let credential = input
                    .token
                    .ok_or_else(|| invalid("owner credential import required"))?;
                if credential.is_empty() || credential.len() > 16384 || credential.contains('\0') {
                    return Err(invalid("invalid bounded credential"));
                }
                let home = self.root.join(&connection_id);
                store::private_dir(&home)?;
                let provider = if input.provider == Provider::Slate {
                    "slate"
                } else {
                    "drlee"
                };
                let data = providers::native::run(json!({"provider":provider,"operation":if input.provider == Provider::Drlee {"register"} else {"identity"},"hostname":host,"credential":credential,"home":home}), &home).await?;
                let identity = providers::native::identity(input.provider, &data)?;
                (
                    Some(host),
                    identity,
                    None,
                    (input.provider == Provider::Slate).then_some(credential),
                )
            }
        };
        let connection = ExternalConnection {
            id: connection_id,
            tenant_id: p.tenant_id.clone(),
            owner_user_id: p.user_id.clone(),
            provider: input.provider,
            hostname: host,
            identity,
            created_at: Utc::now(),
            disabled: false,
        };
        if let Some(token) = token {
            CredentialStore::put(
                &self.store,
                &CredentialRef {
                    connection_id: connection.id.clone(),
                },
                &token,
            )?;
        }
        self.store.save(
            p,
            "connection",
            &connection.id,
            "",
            &StoredConnection {
                public: connection.clone(),
                dws_source: source,
            },
        )?;
        self.audit(p, "connection_registered", "", Some(&connection.id))?;
        Ok(connection)
    }
    pub fn disable(&self, p: &Principal, id: &str) -> PlatformResult<()> {
        let mut connection = self.store.connection(p, id)?;
        connection.public.disabled = true;
        self.store.save(p, "connection", id, "", &connection)?;
        self.audit(p, "connection_disabled", "", Some(id))
    }
    pub fn bind(
        &self,
        p: &Principal,
        project: &str,
        input: BindRequest,
    ) -> PlatformResult<ProjectCliBinding> {
        self.repository.get_project(p, project)?;
        let connection = self.store.connection(p, &input.connection_id)?;
        if connection.public.disabled
            || !matches!(
                (input.provider, connection.public.provider),
                (Provider::Dws, Provider::Dws)
                    | (Provider::Gitlab, Provider::Gitlab)
                    | (Provider::Git, Provider::Gitlab)
                    | (Provider::Slate, Provider::Slate)
                    | (Provider::Drlee, Provider::Drlee)
            )
        {
            return Err(invalid("connection cannot be used by this provider"));
        }
        let repository = if matches!(
            input.provider,
            Provider::Dws | Provider::Slate | Provider::Drlee
        ) {
            if input.repository.is_some() {
                return Err(invalid("this provider does not accept a repository"));
            }
            None
        } else {
            Some(valid_repository(
                input
                    .repository
                    .as_deref()
                    .ok_or_else(|| invalid("repository required"))?,
            )?)
        };
        if input.resource_id.as_ref().is_some_and(|id| {
            input.provider != Provider::Drlee || uuid::Uuid::parse_str(id).is_err()
        }) {
            return Err(invalid("resource_id is an owner-bound Drlee channel UUID"));
        }
        if !(60..=604800).contains(&input.ttl_seconds) {
            return Err(invalid("binding ttl must be 60–604800 seconds"));
        }
        let binding = ProjectCliBinding {
            project_id: project.into(),
            provider: input.provider,
            connection_id: input.connection_id,
            repository,
            resource_id: input.resource_id,
            expires_at: Utc::now() + Duration::seconds(input.ttl_seconds),
        };
        self.store.save(
            p,
            "binding",
            &format!("{project}:{:?}", input.provider),
            project,
            &binding,
        )?;
        self.audit(p, "binding_updated", project, Some(&binding.connection_id))?;
        Ok(binding)
    }
    pub fn unbind(&self, p: &Principal, project: &str, provider: Provider) -> PlatformResult<()> {
        self.repository.get_project(p, project)?;
        self.store.unbind(p, project, provider)?;
        self.audit(p, "binding_removed", project, None)
    }
    pub fn status(&self, p: &Principal, project: &str) -> PlatformResult<Value> {
        self.repository.get_project(p, project)?;
        let bindings = self.store.bindings(p, project)?;
        let requests = self
            .store
            .requests(p, project)?
            .into_iter()
            .map(|request| {
                let state = if request.expires_at <= Utc::now() {
                    "expired"
                } else if self
                    .repository
                    .is_tool_granted(p, project, &request.capability)
                    .unwrap_or(false)
                    && bindings.iter().any(|b| {
                        b.provider == request.provider
                            && b.expires_at > Utc::now()
                            && self
                                .store
                                .connection(p, &b.connection_id)
                                .is_ok_and(|c| !c.public.disabled)
                    })
                {
                    "configured_not_verified"
                } else {
                    "owner_action_required"
                };
                json!({"request":request,"state":state})
            })
            .collect::<Vec<_>>();
        Ok(
            json!({"bindings":bindings,"requests":requests,"dws_scope":self.dws_scope(p,project)?,"note":"Binding metadata is not proof of live authentication; every business call rechecks identity and grants."}),
        )
    }
    pub fn dws_scope(
        &self,
        p: &Principal,
        project: &str,
    ) -> PlatformResult<Option<providers::dws::messages::Scope>> {
        self.repository.get_project(p, project)?;
        Ok(self
            .store
            .list(p, "dws_scope", Some(project))?
            .into_iter()
            .next())
    }
    pub fn configure_dws_scope(
        &self,
        p: &Principal,
        project: &str,
        scope: providers::dws::messages::Scope,
    ) -> PlatformResult<()> {
        self.repository.get_project(p, project)?;
        scope.validate()?;
        self.store.save(p, "dws_scope", project, project, &scope)?;
        self.audit(p, "dws_scope_updated", project, None)
    }
    pub fn request(
        &self,
        p: &Principal,
        project: &str,
        input: AccessRequestInput,
    ) -> PlatformResult<Value> {
        let _serial = self
            .requests
            .lock()
            .map_err(|_| PlatformError::Storage("request synchronization failed".into()))?;
        self.repository.get_project(p, project)?;
        if !self
            .providers()
            .iter()
            .any(|v| v.provider == input.provider && v.capabilities.contains(&input.capability))
            || input.reason.trim().is_empty()
            || input.reason.len() > 512
        {
            return Err(invalid("unknown capability or invalid reason"));
        }
        let existing = self.store.requests(p, project)?.into_iter().find(|r| {
            r.provider == input.provider
                && r.capability == input.capability
                && r.expires_at > Utc::now()
        });
        let request = existing.unwrap_or(ExternalAccessRequest {
            id: opaque_id("access_request"),
            project_id: project.into(),
            provider: input.provider,
            capability: input.capability,
            reason: input.reason,
            created_at: Utc::now(),
            expires_at: Utc::now() + Duration::hours(24),
        });
        self.store
            .save(p, "request", &request.id, project, &request)?;
        self.audit(p, "access_requested", project, None)?;
        Ok(
            json!({"request":request,"status":self.status(p,project)?,"next_step":"Owner uses access connections import-gitlab / login-slate / import-slate / login-drlee / import-drlee (or token stdin), then explicitly binds and grants this project. DWS uses dws-auth. Agents cannot read credentials, register, bind or approve. Git clone grants do not include push."}),
        )
    }
    pub async fn invoke(
        &self,
        p: &Principal,
        project: &str,
        tool: &str,
        args: Value,
    ) -> PlatformResult<Value> {
        self.invoke_pinned(p, project, tool, args, None).await
    }
    pub async fn invoke_source(
        &self,
        p: &Principal,
        project: &str,
        connection: &str,
        args: Value,
    ) -> PlatformResult<Value> {
        self.invoke_pinned(p, project, "dws.chat.messages.read", args, Some(connection))
            .await
    }
    pub async fn open_events(
        &self,
        p: &Principal,
        source: &feynman_core::ObservationSource,
    ) -> PlatformResult<tokio::process::Child> {
        let connection_id = source
            .connection_id
            .as_deref()
            .ok_or(PlatformError::Unauthorized)?;
        self.repository
            .check_agent_tool(p, &source.project_id, "dws.event.messages.listen")?;
        self.validate_dws_source(
            p,
            &source.project_id,
            connection_id,
            source.arguments.clone(),
        )?;
        let connection = self.store.connection(p, connection_id)?;
        let (origin, id) = connection
            .dws_source
            .as_ref()
            .ok_or(PlatformError::Unauthorized)?;
        let (_lock, home, identity) = self.dws.lock_verified_home(p, origin, id).await?;
        if identity != connection.public.identity {
            return Err(PlatformError::Conflict("event identity changed".into()));
        }
        let (kind, flag) = match source.arguments["target_kind"].as_str() {
            Some("group") => ("group", "--chat-id"),
            Some("user") => ("sender", "--user"),
            Some("open_dingtalk_id") => ("sender", "--open-dingtalk-id"),
            _ => return Err(invalid("unsupported event target")),
        };
        let target = source.arguments["target_id"]
            .as_str()
            .ok_or_else(|| invalid("event target required"))?;
        let args = vec![
            "event".into(),
            "+listen-im".into(),
            "--kind".into(),
            kind.into(),
            "--events".into(),
            "message".into(),
            flag.into(),
            target.into(),
            "--duration".into(),
            "10m".into(),
            "--format".into(),
            "ndjson".into(),
            "--profile".into(),
            identity,
        ];
        let env = vec![
            (
                "DWS_CONFIG_DIR".into(),
                home.join("config").to_string_lossy().into(),
            ),
            (
                "DWS_KEYCHAIN_DIR".into(),
                home.join("keychain").to_string_lossy().into(),
            ),
            ("DWS_DISABLE_KEYCHAIN".into(), "1".into()),
        ];
        self.validate_dws_source(
            p,
            &source.project_id,
            connection_id,
            source.arguments.clone(),
        )?;
        self.repository
            .check_agent_tool(p, &source.project_id, "dws.event.messages.listen")?;
        self.audit(p, "listener_start", &source.project_id, Some(connection_id))?;
        cli_executor::command(
            std::env::var_os("FEYNMAN_DWS_BIN").unwrap_or_else(|| "dws".into()),
            &args,
            &env,
            &home,
        )?
        .stdin(std::process::Stdio::piped())
        .kill_on_drop(false)
        .spawn()
        .map_err(|_| PlatformError::Runtime("DWS event consumer unavailable".into()))
    }
    pub async fn verify_event_identity(
        &self,
        p: &Principal,
        source: &feynman_core::ObservationSource,
    ) -> PlatformResult<()> {
        let id = source
            .connection_id
            .as_deref()
            .ok_or(PlatformError::Unauthorized)?;
        self.validate_dws_source(p, &source.project_id, id, source.arguments.clone())?;
        self.repository
            .check_agent_tool(p, &source.project_id, "dws.event.messages.listen")?;
        let connection = self.store.connection(p, id)?;
        let (origin, auth) = connection
            .dws_source
            .as_ref()
            .ok_or(PlatformError::Unauthorized)?;
        let (_, identity) = self.dws.verified_home(p, origin, auth).await?;
        if identity != connection.public.identity {
            return Err(PlatformError::Conflict(
                "DWS listener identity changed".into(),
            ));
        }
        Ok(())
    }
    async fn invoke_pinned(
        &self,
        p: &Principal,
        project: &str,
        tool: &str,
        args: Value,
        expected_connection: Option<&str>,
    ) -> PlatformResult<Value> {
        let result = self
            .invoke_pinned_inner(p, project, tool, args, expected_connection)
            .await;
        if result.is_err() && self.repository.get_project(p, project).is_ok() {
            // No arguments, external payload or raw CLI error in denial metadata.
            let _ = self.audit(p, "cli_invoke_denied", project, None);
        }
        result
    }
    async fn invoke_pinned_inner(
        &self,
        p: &Principal,
        project: &str,
        tool: &str,
        args: Value,
        expected_connection: Option<&str>,
    ) -> PlatformResult<Value> {
        let agent_id = self.repository.get_project(p, project)?.main_agent_id;
        let source_arguments = if tool == providers::dws::messages::TOOL_ID {
            Some(args.clone())
        } else {
            None
        };
        let provider = match tool {
            "dws.aisearch.enterprise" | "dws.chat.messages.read" => Provider::Dws,
            "gitlab.repo.read" | "gitlab.mr.read" => Provider::Gitlab,
            "git.remote.read" | "git.workspace.clone" => Provider::Git,
            "slate.identity.read" | "slate.tasks.read" | "slate.projects.read" => Provider::Slate,
            "drlee.identity.read"
            | "drlee.channels.read"
            | "drlee.messages.read"
            | "drlee.knowledge.read" => Provider::Drlee,
            _ => return Err(invalid("unsupported CLI capability")),
        };
        self.repository.check_agent_tool(p, project, tool)?;
        let binding = self
            .store
            .bindings(p, project)?
            .into_iter()
            .find(|b| {
                b.provider == provider
                    && b.expires_at > Utc::now()
                    && expected_connection.is_none_or(|id| id == b.connection_id)
            })
            .ok_or_else(|| PlatformError::Conflict("active project CLI binding required".into()))?;
        let connection = self.store.connection(p, &binding.connection_id)?;
        let lock = self
            .credential_locks
            .lock()
            .map_err(|_| PlatformError::Storage("credential synchronization failed".into()))?
            .entry(binding.connection_id.clone())
            .or_default()
            .clone();
        let _credential_serial = lock.lock().await;
        if connection.public.disabled {
            return Err(PlatformError::Conflict("connection disabled".into()));
        }
        let result = if provider == Provider::Dws {
            let scope = if tool == providers::dws::messages::TOOL_ID {
                Some(self.dws_scope(p, project)?.ok_or_else(|| {
                    PlatformError::Conflict("owner-configured DWS message scope required".into())
                })?)
            } else {
                None
            };
            let mut command = if let Some(scope) = &scope {
                scope.command(args)?
            } else {
                providers::dws::search_args(args)?
            };
            let (origin, id) = connection
                .dws_source
                .as_ref()
                .ok_or_else(|| invalid("invalid DWS connection"))?;
            let (_refresh_serial, home, identity) =
                self.dws.lock_verified_home(p, origin, id).await?;
            if identity != connection.public.identity {
                return Err(PlatformError::Conflict("external identity changed".into()));
            }
            // Pin every business read to the verified account, not a mutable current profile.
            command.extend(["--profile".into(), identity]);
            let env = vec![
                (
                    "DWS_CONFIG_DIR".into(),
                    home.join("config").to_string_lossy().into(),
                ),
                (
                    "DWS_KEYCHAIN_DIR".into(),
                    home.join("keychain").to_string_lossy().into(),
                ),
                ("DWS_DISABLE_KEYCHAIN".into(), "1".into()),
            ];
            self.recheck(p, project, tool, &binding)?;
            if scope.is_some() && scope != self.dws_scope(p, project)? {
                return Err(PlatformError::Conflict(
                    "DWS scope changed before execution".into(),
                ));
            }
            let output = cli_executor::run(
                std::env::var_os("FEYNMAN_DWS_BIN").unwrap_or_else(|| "dws".into()),
                &command,
                &env,
                &home,
            )
            .await?;
            let mut data: Value = serde_json::from_slice(&output)
                .map_err(|_| PlatformError::Runtime("DWS returned invalid JSON".into()))?;
            scrub_auth_fields(&mut data);
            data
        } else if matches!(provider, Provider::Slate | Provider::Drlee) {
            if args != json!({}) {
                return Err(invalid("fixed native read accepts no caller CLI arguments"));
            }
            let host = connection
                .public
                .hostname
                .as_deref()
                .ok_or_else(|| invalid("missing host"))?;
            let home = self.root.join(&connection.public.id);
            let credential = if provider == Provider::Slate {
                Some(self.store.secret(&connection.public.id)?)
            } else {
                None
            };
            let provider_name = if provider == Provider::Slate {
                "slate"
            } else {
                "drlee"
            };
            let base = json!({"provider":provider_name,"hostname":host,"home":home,"credential":credential});
            let mut verify = base.clone();
            verify["operation"] = json!("identity");
            if providers::native::identity(provider, &providers::native::run(verify, &home).await?)?
                != connection.public.identity
            {
                return Err(PlatformError::Conflict("external identity changed".into()));
            }
            self.recheck(p, project, tool, &binding)?;
            let operation = match tool {
                "slate.identity.read" | "drlee.identity.read" => "identity",
                "slate.tasks.read" => "tasks",
                "slate.projects.read" => "projects",
                "drlee.channels.read" => "channels",
                "drlee.messages.read" => "messages",
                "drlee.knowledge.read" => "knowledge",
                _ => unreachable!(),
            };
            if operation == "messages" && binding.resource_id.is_none() {
                return Err(invalid("owner-bound channel resource_id required"));
            }
            let mut payload = base;
            payload["operation"] = json!(operation);
            payload["resource_id"] = json!(binding.resource_id);
            let mut data = providers::native::run(payload, &home).await?;
            if provider == Provider::Drlee
                && providers::native::identity(provider, &data)? != connection.public.identity
            {
                return Err(PlatformError::Conflict(
                    "external identity changed during native read".into(),
                ));
            }
            scrub_auth_fields(&mut data);
            data
        } else {
            if tool != "git.workspace.clone" && args != json!({}) {
                return Err(invalid(
                    "this fixed read operation accepts no CLI arguments",
                ));
            }
            let host = connection
                .public
                .hostname
                .as_deref()
                .ok_or_else(|| invalid("missing connection host"))?;
            let token = CredentialStore::read(
                &self.store,
                &CredentialRef {
                    connection_id: connection.public.id.clone(),
                },
            )?;
            if self.gitlab.verify(host, &token).await? != connection.public.identity {
                return Err(PlatformError::Conflict("external identity changed".into()));
            }
            self.recheck(p, project, tool, &binding)?;
            let repo = binding
                .repository
                .as_deref()
                .ok_or_else(|| invalid("missing repository binding"))?;
            if provider == Provider::Git {
                if tool == "git.workspace.clone" {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct CloneInput {
                        idempotency_key: uuid::Uuid,
                    }
                    let input: CloneInput = serde_json::from_value(args)
                        .map_err(|_| invalid("clone requires only UUID idempotency_key"))?;
                    let workspace = self
                        .workspace
                        .as_ref()
                        .ok_or_else(|| invalid("workspace clone not configured"))?;
                    let w = workspace
                        .get(p, project)?
                        .ok_or_else(|| invalid("create a Sandbox Workspace before cloning"))?;
                    if w.provider != feynman_core::WorkspaceKind::Opensandbox {
                        return Err(invalid(
                            "credential-bearing clone is Sandbox-only, never the local host",
                        ));
                    }
                    self.audit(p, "git_clone_start", project, Some(&binding.connection_id))?;
                    workspace.clone_repository(p,&w,json!({"hostname":host,"repository":repo,"token":token,"idempotency_key":input.idempotency_key})).await?
                } else {
                    providers::git::read(host, &token, repo, &self.root).await?
                }
            } else {
                self.gitlab.read(host, &token, repo, tool).await?
            }
        };
        self.recheck(p, project, tool, &binding)?;
        if self.repository.get_project(p, project)?.main_agent_id != agent_id {
            return Err(PlatformError::Conflict(
                "Agent changed during CLI execution".into(),
            ));
        }
        if let Some(arguments) = source_arguments {
            self.validate_dws_source(p, project, &binding.connection_id, arguments)?;
        }
        self.audit(
            p,
            if tool == "git.workspace.clone" {
                "git_clone_complete"
            } else {
                "cli_read"
            },
            project,
            Some(&binding.connection_id),
        )?;
        Ok(result)
    }
    fn recheck(
        &self,
        p: &Principal,
        project: &str,
        tool: &str,
        binding: &ProjectCliBinding,
    ) -> PlatformResult<()> {
        let connection = &binding.connection_id;
        let project_info = self.repository.get_project(p, project)?;
        let call = TrustedCall {
            principal: p,
            project_id: project,
            agent_id: &project_info.main_agent_id,
            tool,
            binding,
            connection_enabled: !self.store.connection(p, connection)?.public.disabled,
            granted: self.repository.check_agent_tool(p, project, tool).is_ok(),
        };
        if !matches!(self.policy.evaluate(&call), PolicyDecision::Allow) {
            return Err(PlatformError::Conflict(
                "access policy denied or needs owner approval".into(),
            ));
        }
        self.delivery.authorize("server_proxy", &call)?;
        if self.repository.check_agent_tool(p, project, tool).is_err()
            || self.store.connection(p, connection)?.public.disabled
        {
            return Err(PlatformError::Conflict(
                "access revoked before execution".into(),
            ));
        }
        if !self
            .store
            .bindings(p, project)?
            .iter()
            .any(|b| b == binding && b.expires_at > Utc::now())
        {
            return Err(PlatformError::Conflict(
                "binding changed or expired before execution".into(),
            ));
        }
        Ok(())
    }
    pub fn validate_dws_source(
        &self,
        p: &Principal,
        project: &str,
        connection_id: &str,
        args: Value,
    ) -> PlatformResult<()> {
        self.repository
            .check_agent_tool(p, project, "dws.chat.messages.read")?;
        let binding = self
            .store
            .bindings(p, project)?
            .into_iter()
            .find(|b| {
                b.provider == Provider::Dws
                    && b.expires_at > Utc::now()
                    && b.connection_id == connection_id
            })
            .ok_or_else(|| {
                PlatformError::Conflict("observation source binding changed/expired".into())
            })?;
        self.recheck(p, project, "dws.chat.messages.read", &binding)?;
        self.dws_scope(p, project)?
            .ok_or_else(|| PlatformError::Conflict("DWS source scope required".into()))?
            .command(args)?;
        Ok(())
    }
    fn audit(
        &self,
        p: &Principal,
        action: &str,
        project: &str,
        connection: Option<&str>,
    ) -> PlatformResult<()> {
        AuditSink::record(&self.store, p, project, action, connection)
    }
}
fn invalid(message: &str) -> PlatformError {
    PlatformError::InvalidInput(message.into())
}
pub(crate) fn scrub_auth_fields(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.retain(|name, _| {
                !matches!(
                    name.to_ascii_lowercase().replace(['_', '-'], "").as_str(),
                    "token"
                        | "accesstoken"
                        | "refreshtoken"
                        | "clientsecret"
                        | "devicecode"
                        | "authorizationcode"
                        | "idtoken"
                )
            });
            for field in fields.values_mut() {
                scrub_auth_fields(field);
            }
        }
        Value::Array(fields) => {
            for field in fields {
                scrub_auth_fields(field);
            }
        }
        _ => {}
    }
}
pub fn valid_host(host: &str) -> PlatformResult<String> {
    if host.len() > 253
        || !host.contains('.')
        || !host.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && part.as_bytes()[0].is_ascii_alphanumeric()
                && part.as_bytes()[part.len() - 1].is_ascii_alphanumeric()
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(invalid(
            "expected a DNS hostname, without scheme, port or credentials",
        ));
    }
    Ok(host.to_ascii_lowercase())
}
pub fn valid_repository(repo: &str) -> PlatformResult<String> {
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if repo.len() > 512
        || !repo.contains('/')
        || !repo.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.starts_with('-')
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        })
    {
        return Err(invalid(
            "expected namespace/repository, not a URL or filesystem path",
        ));
    }
    Ok(repo.into())
}
