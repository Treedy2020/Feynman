mod external_tools;
mod http;
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
use workspace::LocalWorkspaceProvider;

#[tokio::main]
async fn main() -> Result<()> {
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

    let data_root = std::env::var_os("FEYNMAN_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(".feynman"));
    std::fs::create_dir_all(&data_root)?;
    let db_path = std::env::var_os("FEYNMAN_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| data_root.join("rust-platform.sqlite3"));
    let workspace_root = std::env::var_os("FEYNMAN_WORKSPACE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| data_root.join("workspaces"));

    let repository = Arc::new(SqliteRepository::open(&db_path)?);
    let absolute_db_path = std::fs::canonicalize(&db_path)?;
    let repository_for_tools: Arc<dyn PlatformRepository> = repository.clone();
    let external_tools = external_tools::service(repository_for_tools)?;
    let interrupted_runs = repository.recover_after_server_restart()?;
    if interrupted_runs > 0 {
        tracing::warn!(
            interrupted_runs,
            "marked unfinished Agent Runs interrupted after server restart"
        );
    }
    let runtime: Arc<dyn AgentRuntime> = Arc::new(CodexAppServerRuntime::new(absolute_db_path));
    let workspace: Arc<dyn WorkspaceProvider> =
        Arc::new(LocalWorkspaceProvider::new(workspace_root));
    let identity: Arc<dyn IdentityProvider> = Arc::new(http::DevIdentityProvider);
    let service = Arc::new(PlatformService::new(
        repository,
        runtime.clone(),
        workspace,
        identity,
        external_tools,
    ));
    let app = http::router(service);
    let listener = TcpListener::bind(bind).await?;
    tracing::info!(%bind, "Feynman Rust dev server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    runtime.close().await.context("closing Agent Runtime")?;
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
            _project: &feynman_core::Project,
        ) -> PlatformResult<String> {
            Ok(std::env::temp_dir().to_string_lossy().into_owned())
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
