use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use feynman_core::{
    CreateProjectRequest, IdentityProvider, PlatformError, PlatformResult, PlatformService,
    Principal, SendMessageRequest,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub struct DevIdentityProvider;

#[async_trait::async_trait]
impl IdentityProvider for DevIdentityProvider {
    async fn authenticate(&self, authorization: Option<&str>) -> PlatformResult<Principal> {
        if authorization.is_some() {
            return Err(PlatformError::Unauthorized);
        }
        Ok(Principal {
            user_id: "local-user".into(),
            tenant_id: "local".into(),
        })
    }
}

#[derive(Clone)]
struct HttpState {
    service: Arc<PlatformService>,
}

pub fn router(service: Arc<PlatformService>) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/projects", get(list_projects).post(create_project))
        .route("/v1/projects/{project_id}", get(get_project))
        .route(
            "/v1/projects/{project_id}/external-tools",
            get(list_project_tools),
        )
        .route(
            "/v1/projects/{project_id}/external-tools/{tool_id}",
            put(grant_project_tool).delete(revoke_project_tool),
        )
        .route(
            "/v1/projects/{project_id}/conversation",
            get(get_conversation),
        )
        .route("/v1/projects/{project_id}/messages", post(send_message))
        .route(
            "/v1/projects/{project_id}/runs/{run_id}/events",
            get(get_run_feed),
        )
        .route(
            "/v1/projects/{project_id}/runs/{run_id}/approvals/{request_id}",
            post(respond_to_approval),
        )
        .with_state(HttpState { service })
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status":"ok", "mode":"local-dev", "apiVersion":"v1"}))
}

async fn list_projects(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    let projects = state.service.list_projects(&principal)?;
    Ok(Json(json!({"projects": projects})))
}

async fn create_project(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateProjectRequest>,
) -> Result<Response, ApiError> {
    let principal = principal(&state, &headers).await?;
    let project = state.service.create_project(&principal, request)?;
    Ok((StatusCode::CREATED, Json(json!({"project":project}))).into_response())
}

async fn get_project(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        json!({"project":state.service.get_project(&principal, &project_id)?}),
    ))
}

async fn list_project_tools(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    let tools = state.service.list_project_tools(&principal, &project_id)?;
    Ok(Json(json!({"tools": tools})))
}

async fn grant_project_tool(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((project_id, tool_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let principal = principal(&state, &headers).await?;
    let grant = state
        .service
        .grant_project_tool(&principal, &project_id, &tool_id)?;
    Ok((StatusCode::CREATED, Json(json!({"grant": grant}))).into_response())
}

async fn revoke_project_tool(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((project_id, tool_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let principal = principal(&state, &headers).await?;
    state
        .service
        .revoke_project_tool(&principal, &project_id, &tool_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_conversation(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(json!(
        state.service.get_conversation(&principal, &project_id)?
    )))
}

async fn send_message(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<SendMessageRequest>,
) -> Result<Response, ApiError> {
    let principal = principal(&state, &headers).await?;
    let result = state
        .service
        .send_message(&principal, &project_id, &request.content)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(result)).into_response())
}

#[derive(Deserialize)]
struct EventQuery {
    #[serde(default)]
    after: i64,
}

async fn get_run_feed(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((project_id, run_id)): Path<(String, String)>,
    Query(query): Query<EventQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    let feed = state
        .service
        .get_run_feed(&principal, &project_id, &run_id, query.after)?;
    Ok(Json(json!(feed)))
}

#[derive(Deserialize)]
struct ApprovalRequest {
    decision: String,
}

async fn respond_to_approval(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((project_id, run_id, request_id)): Path<(String, String, String)>,
    Json(request): Json<ApprovalRequest>,
) -> Result<StatusCode, ApiError> {
    let principal = principal(&state, &headers).await?;
    state
        .service
        .respond_to_approval(
            &principal,
            &project_id,
            &run_id,
            &request_id,
            &request.decision,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn principal(state: &HttpState, headers: &HeaderMap) -> Result<Principal, ApiError> {
    let authorization = headers
        .get(AUTHORIZATION)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| ApiError(PlatformError::Unauthorized))
        })
        .transpose()?;
    Ok(state.service.authenticate(authorization).await?)
}

struct ApiError(PlatformError);

impl From<PlatformError> for ApiError {
    fn from(value: PlatformError) -> Self {
        Self(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self.0 {
            PlatformError::NotFound => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            PlatformError::InvalidInput(_) => (StatusCode::BAD_REQUEST, "INVALID_INPUT"),
            PlatformError::RunAlreadyActive => (StatusCode::CONFLICT, "RUN_ALREADY_ACTIVE"),
            PlatformError::Unauthorized => (StatusCode::UNAUTHORIZED, "UNAUTHORIZED"),
            PlatformError::ToolNotGranted(_) => (StatusCode::FORBIDDEN, "TOOL_NOT_GRANTED"),
            PlatformError::Conflict(_) => (StatusCode::CONFLICT, "CONFLICT"),
            PlatformError::Storage(_) | PlatformError::Runtime(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR")
            }
        };
        let message = match self.0 {
            PlatformError::Storage(_) | PlatformError::Runtime(_) => {
                "The server could not complete the request.".to_owned()
            }
            error => error.to_string(),
        };
        (
            status,
            Json(json!({"error":{"code":code,"message":message}})),
        )
            .into_response()
    }
}
