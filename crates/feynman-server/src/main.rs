mod console;
mod external_access;
use external_access::providers::dws::authorization as dws_authorization;
mod external_tools;
mod http;
mod management;
mod resources;
mod runtime;
mod store;
mod workspace;

use anyhow::{Context, Result};
use feynman_core::{
    AgentRuntime, IdentityProvider, PlatformRepository, PlatformService, WorkspaceProvider,
};
use runtime::codex::CodexAppServerRuntime;
use std::net::SocketAddr;
use std::sync::Arc;
use store::SqliteRepository;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use workspace::ProjectWorkspaceProvider;

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var("FEYNMAN_GIT_ASKPASS").as_deref() == Ok("1") {
        external_access::providers::git::askpass()?;
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("external-tools-mcp") {
        external_tools::run_stdio_mcp_server().await?;
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let bind: SocketAddr = std::env::var("FEYNMAN_BIND")
        .unwrap_or_else(|_| "127.0.0.1:4317".into())
        .parse()
        .context("FEYNMAN_BIND must be a socket address")?;
    if !bind.ip().is_loopback() {
        anyhow::bail!(
            "the current dev identity provider has no user authentication; refusing non-loopback bind {bind}"
        );
    }

    let home = feynman_local::LocalHome::from_env()?;
    home.check_legacy_registry(
        &std::env::current_dir()?,
        ["FEYNMAN_HOME", "FEYNMAN_DATA_DIR", "FEYNMAN_DB"]
            .iter()
            .any(|key| std::env::var_os(key).is_some()),
    )?;
    home.initialize()?;
    let db_path = &home.database;
    let workspace_root = &home.workspace_root;

    // Lock before opening/migrating the database, not after recovery has already changed it.
    let lock_path = if db_path.exists() {
        std::fs::canonicalize(db_path)?
    } else {
        std::fs::canonicalize(db_path.parent().context("database parent missing")?)?
            .join(db_path.file_name().context("database filename missing")?)
    };
    let server_lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path.with_extension("server.lock"))?;
    fs2::FileExt::try_lock_exclusive(&server_lock).context("another Feynman server owns this database; use that server instead of starting a second scheduler")?;
    let repository = Arc::new(SqliteRepository::open(db_path)?);
    let absolute_db_path = std::fs::canonicalize(db_path)?;
    let listener = TcpListener::bind(bind).await?;
    let access_server_url = format!("http://{}", listener.local_addr()?);
    repository.interrupt_dws_authorizations()?;
    let authorization = dws_authorization::DwsAuthorizationService::new(
        repository.clone(),
        home.private_root.join("dws"),
        std::env::var_os("FEYNMAN_DWS_BIN").unwrap_or_else(|| "dws".into()),
    )?;
    let repository_for_tools: Arc<dyn PlatformRepository> = repository.clone();
    let workspace = Arc::new(ProjectWorkspaceProvider::new(
        repository.clone(),
        workspace_root.clone(),
    )?);
    let access = external_access::ExternalAccessBroker::new(
        repository_for_tools.clone(),
        home.private_root.join("external-access"),
        authorization.clone(),
    )?
    .with_workspace(workspace.clone());
    let external_tools =
        external_tools::service_at(repository_for_tools, Some(access_server_url.clone()))?;
    let interrupted_runs = repository.recover_after_server_restart()?;
    repository.recover_inbox()?;
    repository.recover_listeners()?;
    if interrupted_runs > 0 {
        tracing::warn!(
            interrupted_runs,
            "marked unfinished Agent Runs interrupted after server restart"
        );
    }
    let runtime: Arc<dyn AgentRuntime> = Arc::new(
        CodexAppServerRuntime::new(absolute_db_path, home.codex_home.clone())
            .with_access_server(access_server_url),
    );
    let workspace_for_runtime: Arc<dyn WorkspaceProvider> = workspace.clone();
    let identity: Arc<dyn IdentityProvider> = Arc::new(http::DevIdentityProvider);
    let resources = Arc::new(resources::ResourceManager::new(
        repository.clone(),
        workspace.clone(),
        access.clone(),
        home.private_root.join("resources"),
    )?);
    let service = Arc::new(
        PlatformService::new(
            repository.clone(),
            runtime.clone(),
            workspace_for_runtime,
            identity,
            external_tools,
        )
        .with_context(resources.clone()),
    );
    let manager = Arc::new(
        management::ProjectManager::new(repository, service.clone(), workspace.clone())
            .with_access(access.clone()),
    );
    let scheduler = manager.start_scheduler();
    let reconciler = {
        let resources = resources.clone();
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                timer.tick().await;
                let _ = resources.tick().await;
            }
        })
    };
    let app =
        http::router_with_management(service, Some(authorization.clone()), Some(manager.clone()))
            .merge(external_access::http::router(access))
            .merge(resources::http::router(resources));
    let app = console::router(app, listener.local_addr()?);
    tracing::info!(%bind, home = %home.home.display(), codex_home = %home.codex_home.display(), "Feynman Rust dev server listening; Console at /console/");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    scheduler.abort();
    reconciler.abort();
    let _ = reconciler.await;
    let _ = scheduler.await;
    manager.close_sources().await;
    runtime.close().await.context("closing Agent Runtime")?;
    workspace.close().await;
    authorization
        .close()
        .await
        .context("closing DWS authorizations")?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        let _ = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install terminate handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use feynman_core::{
        AgentRunStatus, AgentRuntimeEvent, AgentTurnInput, AgentTurnStarted, PlatformError,
        PlatformResult,
    };
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tower::ServiceExt;

    struct TestRuntime;

    #[async_trait::async_trait]
    impl AgentRuntime for TestRuntime {
        async fn start_turn(
            &self,
            input: AgentTurnInput,
        ) -> PlatformResult<(AgentTurnStarted, mpsc::Receiver<AgentRuntimeEvent>)> {
            let (tx, rx) = mpsc::channel(8);
            tx.send(AgentRuntimeEvent::TextDelta {
                text: "test reply".into(),
            })
            .await
            .map_err(|error| PlatformError::Runtime(error.to_string()))?;
            tx.send(AgentRuntimeEvent::Completed {
                status: AgentRunStatus::Completed,
                error: None,
            })
            .await
            .map_err(|error| PlatformError::Runtime(error.to_string()))?;
            Ok((
                AgentTurnStarted {
                    session_id: input.session.id,
                    thread_id: "thread-test".into(),
                    turn_id: "turn-test".into(),
                },
                rx,
            ))
        }

        async fn respond_to_approval(
            &self,
            _project_id: &str,
            _request_id: &str,
            _decision: &str,
        ) -> PlatformResult<()> {
            Ok(())
        }
        async fn close(&self) -> PlatformResult<()> {
            Ok(())
        }
    }

    struct TestWorkspace;
    #[async_trait::async_trait]
    impl feynman_core::WorkspaceProvider for TestWorkspace {
        async fn ensure_workspace(
            &self,
            _principal: &feynman_core::Principal,
            _project: &feynman_core::Project,
        ) -> PlatformResult<feynman_core::PreparedWorkspace> {
            Ok(feynman_core::PreparedWorkspace {
                local_cwd: std::env::temp_dir().to_string_lossy().into_owned(),
                environment: None,
            })
        }
    }

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn cli_api_creates_project_and_runs_a_streamed_chat_turn() {
        let path =
            std::env::temp_dir().join(format!("feynman-http-{}.sqlite3", uuid::Uuid::new_v4()));
        let repository = Arc::new(SqliteRepository::open(&path).unwrap());
        let repository_for_tools: Arc<dyn PlatformRepository> = repository.clone();
        let external_tools = external_tools::service(repository_for_tools).unwrap();
        let service = Arc::new(PlatformService::new(
            repository,
            Arc::new(TestRuntime),
            Arc::new(TestWorkspace),
            Arc::new(http::DevIdentityProvider),
            external_tools,
        ));
        let app = http::router(service.clone());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/projects")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"name":"Test", "tenant_id":"spoofed"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let project_id = body_json(response).await["project"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            service
                .list_projects(&feynman_core::Principal {
                    user_id: "local-user".into(),
                    tenant_id: "local".into()
                })
                .unwrap()
                .len(),
            1
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/projects/{project_id}/messages"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"content":"hello"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let run_id = body_json(response).await["run"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let principal = feynman_core::Principal {
            user_id: "local-user".into(),
            tenant_id: "local".into(),
        };
        let mut feed = service
            .get_run_feed(&principal, &project_id, &run_id, 0)
            .unwrap();
        for _ in 0..50 {
            if feed.run.status.is_terminal() {
                break;
            }
            tokio::task::yield_now().await;
            feed = service
                .get_run_feed(&principal, &project_id, &run_id, 0)
                .unwrap();
        }
        assert_eq!(feed.run.status, AgentRunStatus::Completed);
        assert!(
            feed.events
                .iter()
                .any(|event| event.kind == "agent_text_delta"
                    && event.payload["text"] == "test reply")
        );

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/projects")
                    .header("authorization", "Bearer something")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        drop(service);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn project_external_tool_grant_can_be_listed_granted_and_revoked() {
        let path = std::env::temp_dir().join(format!(
            "feynman-tools-http-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let repository = Arc::new(SqliteRepository::open(&path).unwrap());
        let repository_for_tools: Arc<dyn PlatformRepository> = repository.clone();
        let service = Arc::new(PlatformService::new(
            repository,
            Arc::new(TestRuntime),
            Arc::new(TestWorkspace),
            Arc::new(http::DevIdentityProvider),
            external_tools::service(repository_for_tools).unwrap(),
        ));
        let app = http::router(service);
        let project = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/projects")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"name":"Tool grant test"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(project.status(), StatusCode::CREATED);
        let project_id = body_json(project).await["project"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let list_uri = format!("/v1/projects/{project_id}/external-tools");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&list_uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let tools = body_json(response).await;
        assert_eq!(tools["tools"][0]["id"], "dws.aisearch.enterprise");
        assert_eq!(tools["tools"][0]["granted"], false);

        // The same public invocation interface used by the CLI must enforce grants.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("{list_uri}/dws.aisearch.enterprise/invoke"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"arguments": {}}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            body_json(response).await["error"]["code"],
            "TOOL_NOT_GRANTED"
        );

        let grant_uri = format!("{list_uri}/dws.aisearch.enterprise");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(&grant_uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            body_json(response).await["grant"]["tool_id"],
            "dws.aisearch.enterprise"
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&list_uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(response).await["tools"][0]["granted"], true);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(&grant_uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .oneshot(
                Request::builder()
                    .uri(&list_uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(response).await["tools"][0]["granted"], false);
        let _ = std::fs::remove_file(path);
    }
}
