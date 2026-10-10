use crate::external_access::security::{AuthIntent, AuthenticationStrategy, DeviceAuthentication};
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
mod streams;

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
    dws_authorization: Option<Arc<crate::dws_authorization::DwsAuthorizationService>>,
    manager: Option<Arc<crate::management::ProjectManager>>,
}

#[cfg(test)]
pub fn router(service: Arc<PlatformService>) -> Router {
    router_with_management(service, None, None)
}

pub fn router_with_management(
    service: Arc<PlatformService>,
    dws_authorization: Option<Arc<crate::dws_authorization::DwsAuthorizationService>>,
    manager: Option<Arc<crate::management::ProjectManager>>,
) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/status", get(manager_overview))
        .route("/v1/notifications", get(streams::notifications))
        .route("/v1/notifications/stream", get(streams::notification_stream))
        .route("/v1/notifications/{id}/read", post(streams::read_notification))
        .route("/v1/projects/{project_id}/stream", get(streams::project_stream))
        .route("/v1/projects", get(list_projects).post(create_project))
        .route("/v1/projects/{project_id}", get(get_project))
        .route("/v1/projects/{project_id}/status", get(manager_status))
        .route(
            "/v1/projects/{project_id}/agents",
            get(list_agents).post(create_agent),
        )
        .route(
            "/v1/projects/{project_id}/agents/{agent_id}",
            put(update_agent),
        )
        .route(
            "/v1/projects/{project_id}/sources",
            get(list_sources).post(create_source),
        )
        .route("/v1/projects/{project_id}/feed", get(read_observation_feed))
        .route(
            "/v1/projects/{project_id}/agents/{agent_id}/feed",
            get(read_agent_feed),
        )
        .route(
            "/v1/projects/{project_id}/sources/{source_id}",
            put(enable_source),
        )
        .route(
            "/v1/projects/{project_id}/sources/{source_id}/records",
            get(read_observations).post(ingest_observations),
        )
        .route(
            "/v1/projects/{project_id}/sources/{source_id}/backfill",
            post(backfill_source),
        )
        .route(
            "/v1/projects/{project_id}/sources/{source_id}/captures",
            get(read_captures),
        )
        .route(
            "/v1/projects/{project_id}/inbox",
            get(list_inbox).post(enqueue_input),
        )
        .route(
            "/v1/projects/{project_id}/inbox/{message_id}",
            put(resolve_delivery),
        )
        .route(
            "/v1/projects/{project_id}/schedules/{schedule_id}/deliveries",
            get(schedule_deliveries),
        )
        .route(
            "/v1/projects/{project_id}/workspace",
            get(get_workspace).post(create_workspace),
        )
        .route(
            "/v1/projects/{project_id}/workspace/actions",
            post(workspace_action),
        )
        .route(
            "/v1/projects/{project_id}/workspace/seed",
            post(seed_workspace).layer(axum::extract::DefaultBodyLimit::max(48 * 1024 * 1024)),
        )
        .route(
            "/v1/projects/{project_id}/tasks",
            get(list_tasks).post(create_task),
        )
        .route(
            "/v1/projects/{project_id}/tasks/{task_id}",
            get(get_task).delete(cancel_task),
        )
        .route(
            "/v1/projects/{project_id}/schedules",
            get(list_schedules).post(create_schedule),
        )
        .route(
            "/v1/projects/{project_id}/schedules/{schedule_id}",
            put(enable_schedule),
        )
        .route(
            "/v1/projects/{project_id}/external-tools",
            get(list_project_tools),
        )
        .route(
            "/v1/projects/{project_id}/external-tools/{tool_id}",
            put(grant_project_tool).delete(revoke_project_tool),
        )
        .route(
            "/v1/projects/{project_id}/external-tools/{tool_id}/invoke",
            post(invoke_project_tool),
        )
        .route(
            "/v1/projects/{project_id}/conversation",
            get(get_conversation),
        )
        .route("/v1/projects/{project_id}/messages", post(send_message))
        .route(
            "/v1/projects/{project_id}/dws-authorizations",
            get(list_dws_authorizations).post(request_dws_authorization),
        )
        .route(
            "/v1/projects/{project_id}/dws-authorizations/{authorization_id}",
            get(get_dws_authorization).delete(cancel_dws_authorization),
        )
        .route(
            "/v1/projects/{project_id}/dws-authorizations/{authorization_id}/check",
            post(check_dws_authorization),
        )
        .route(
            "/v1/projects/{project_id}/runs/{run_id}/events",
            get(get_run_feed),
        )
        .route(
            "/v1/projects/{project_id}/runs/{run_id}/approvals/{request_id}",
            post(respond_to_approval),
        )
        .with_state(HttpState {
            service,
            dws_authorization,
            manager,
        })
        .layer(axum::middleware::map_response(private_response))
}

fn manager(state: &HttpState) -> Result<&Arc<crate::management::ProjectManager>, ApiError> {
    state.manager.as_ref().ok_or_else(|| {
        ApiError(PlatformError::Runtime(
            "project manager is not configured".into(),
        ))
    })
}

async fn list_agents(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path(p): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(json!({"agents":manager(&s)?.agents(&a,&p)?})))
}
async fn create_agent(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path(p): Path<String>,
    Json(input): Json<feynman_core::AgentInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(
        json!({"agent":manager(&s)?.save_agent(&a,&p,None,input)?}),
    ))
}
async fn update_agent(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path((p, id)): Path<(String, String)>,
    Json(input): Json<feynman_core::AgentInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(
        json!({"agent":manager(&s)?.save_agent(&a,&p,Some(&id),input)?}),
    ))
}
async fn list_sources(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path(p): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(json!({"sources":manager(&s)?.sources(&a,&p)?})))
}
async fn create_source(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path(p): Path<String>,
    Json(input): Json<feynman_core::SourceInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(
        json!({"source":manager(&s)?.create_source(&a,&p,input)?}),
    ))
}
async fn read_observation_feed(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path(p): Path<String>,
    Query(q): Query<feynman_core::ObservationFeedQuery>,
) -> Result<Json<feynman_core::ObservationFeedPage>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(manager(&s)?.observation_feed(&a, &p, None, &q)?))
}
async fn read_agent_feed(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path((p, id)): Path<(String, String)>,
    Query(q): Query<feynman_core::ObservationFeedQuery>,
) -> Result<Json<feynman_core::ObservationFeedPage>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(manager(&s)?.observation_feed(
        &a,
        &p,
        Some(&id),
        &q,
    )?))
}
async fn enable_source(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path((p, id)): Path<(String, String)>,
    Json(input): Json<EnableScheduleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(
        json!({"source":manager(&s)?.set_source_enabled(&a,&p,&id,input.enabled)?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationQuery {
    #[serde(default)]
    after: i64,
    #[serde(default = "record_limit")]
    limit: i64,
}
fn record_limit() -> i64 {
    100
}
async fn read_observations(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path((p, id)): Path<(String, String)>,
    Query(q): Query<ObservationQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(
        json!({"records":manager(&s)?.observations(&a,&p,&id,q.after,q.limit)?}),
    ))
}
async fn read_captures(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path((p, id)): Path<(String, String)>,
    Query(q): Query<ObservationQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(
        json!({"captures":manager(&s)?.captures(&a,&p,&id,q.after,q.limit)?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IngestInput {
    records: Vec<feynman_core::ObservationInput>,
}
async fn ingest_observations(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path((p, id)): Path<(String, String)>,
    Json(input): Json<IngestInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(
        json!({"inserted":manager(&s)?.ingest(&a,&p,&id,input.records)?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BackfillInput {
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
}
async fn backfill_source(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path((p, id)): Path<(String, String)>,
    Json(input): Json<BackfillInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(
        manager(&s)?
            .backfill(&a, &p, &id, input.start, input.end)
            .await?,
    ))
}
async fn list_inbox(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path(p): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(json!({"inbox":manager(&s)?.inbox(&a,&p)?})))
}
async fn enqueue_input(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path(p): Path<String>,
    Json(input): Json<feynman_core::InboxInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(json!({"message":manager(&s)?.enqueue(&a,&p,input)?})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveInput {
    state: String,
    run_id: Option<String>,
}
async fn resolve_delivery(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path((p, id)): Path<(String, String)>,
    Json(input): Json<ResolveInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(
        json!({"message":manager(&s)?.resolve_delivery(&a,&p,&id,&input.state,input.run_id)?}),
    ))
}
async fn schedule_deliveries(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path((p, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let a = principal(&s, &h).await?;
    Ok(Json(manager(&s)?.deliveries(&a, &p, &id)?))
}

async fn manager_overview(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(manager(&state)?.overview(&principal)?))
}
async fn manager_status(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(manager(&state)?.status(&principal, &project_id)?))
}
async fn get_workspace(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        json!({"workspace":manager(&state)?.workspace(&principal, &project_id)?}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceRequest {
    provider: feynman_core::WorkspaceKind,
}
async fn create_workspace(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<WorkspaceRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        json!({"workspace":manager(&state)?.create_workspace(&principal, &project_id, request.provider).await?}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum WorkspaceAction {
    Refresh,
    Stop,
    Resume,
    Restore,
    Renew,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceActionRequest {
    action: WorkspaceAction,
}
async fn workspace_action(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<WorkspaceActionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    let action = match request.action {
        WorkspaceAction::Refresh => "refresh",
        WorkspaceAction::Stop => "stop",
        WorkspaceAction::Resume => "resume",
        WorkspaceAction::Restore => "restore",
        WorkspaceAction::Renew => "renew",
    };
    Ok(Json(
        json!({"workspace":manager(&state)?.workspace_action(&principal, &project_id, action).await?}),
    ))
}
async fn seed_workspace(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(input): Json<feynman_local::evaluation::SeedInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        json!({"seed":manager(&state)?.seed_workspace(&principal,&project_id,input).await?}),
    ))
}
async fn list_tasks(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        json!({"tasks":manager(&state)?.tasks(&principal, &project_id)?}),
    ))
}
async fn create_task(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<feynman_core::CreateTaskRequest>,
) -> Result<Response, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task":manager(&state)?.create_task(&principal, &project_id, request)?})),
    )
        .into_response())
}
async fn get_task(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((project_id, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        json!({"task":manager(&state)?.task(&principal, &project_id, &id)?}),
    ))
}
async fn cancel_task(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((project_id, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        json!({"task":manager(&state)?.cancel_task(&principal, &project_id, &id)?}),
    ))
}
async fn list_schedules(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        json!({"schedules":manager(&state)?.schedules(&principal, &project_id)?}),
    ))
}
async fn create_schedule(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<feynman_core::CreateScheduleRequest>,
) -> Result<Response, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({"schedule":manager(&state)?.create_schedule(&principal, &project_id, request)?}),
        ),
    )
        .into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnableScheduleRequest {
    enabled: bool,
}
async fn enable_schedule(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((project_id, id)): Path<(String, String)>,
    Json(request): Json<EnableScheduleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        json!({"schedule":manager(&state)?.enable_schedule(&principal, &project_id, &id, request.enabled)?}),
    ))
}

async fn private_response(mut response: Response) -> Response {
    if response.status() == StatusCode::UNPROCESSABLE_ENTITY {
        response = (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error":{"code":"INVALID_INPUT","message":"invalid JSON input"}})),
        )
            .into_response();
    }
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        "referrer-policy",
        axum::http::HeaderValue::from_static("no-referrer"),
    );
    response
}

fn authorization_service(
    state: &HttpState,
) -> Result<&Arc<crate::dws_authorization::DwsAuthorizationService>, ApiError> {
    state.dws_authorization.as_ref().ok_or_else(|| {
        ApiError(PlatformError::Runtime(
            "DWS authorization is not configured".into(),
        ))
    })
}

async fn request_dws_authorization(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    request: Result<
        Json<crate::dws_authorization::AuthorizationRequest>,
        axum::extract::rejection::JsonRejection,
    >,
) -> Result<Response, ApiError> {
    let principal = principal(&state, &headers).await?;
    let Json(request) = request.map_err(|_| {
        ApiError(PlatformError::InvalidInput(
            "invalid device authorization request".into(),
        ))
    })?;
    let authorization = DeviceAuthentication(authorization_service(&state)?.clone())
        .start(
            &principal,
            &project_id,
            AuthIntent::Device {
                reason: request.reason,
            },
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(json!({"authorization":authorization,"credential_location":"local-control-plane","sandbox_installed":false}))).into_response())
}

async fn list_dws_authorizations(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        json!({"authorizations":authorization_service(&state)?.list(&principal, &project_id)?}),
    ))
}

async fn get_dws_authorization(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((project_id, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        json!({"authorization":DeviceAuthentication(authorization_service(&state)?.clone()).inspect(&principal, &project_id, &id).await?}),
    ))
}

async fn cancel_dws_authorization(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((project_id, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        json!({"authorization":DeviceAuthentication(authorization_service(&state)?.clone()).cancel(&principal, &project_id, &id).await?}),
    ))
}

async fn check_dws_authorization(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((project_id, id)): Path<(String, String)>,
    request: Result<Json<EmptyAuthorizationCheck>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let principal = principal(&state, &headers).await?;
    let Json(_) = request.map_err(|_| {
        ApiError(PlatformError::InvalidInput(
            "authentication check requires an empty JSON object".into(),
        ))
    })?;
    Ok(Json(
        json!({"authorization":DeviceAuthentication(authorization_service(&state)?.clone()).refresh(&principal, &project_id, &id).await?}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyAuthorizationCheck {}

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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InvokeToolRequest {
    arguments: serde_json::Value,
}

async fn invoke_project_tool(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((project_id, tool_id)): Path<(String, String)>,
    Json(request): Json<InvokeToolRequest>,
) -> Result<Json<feynman_core::ExternalToolResult>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        state
            .service
            .invoke_project_tool(&principal, &project_id, &tool_id, request.arguments)
            .await?,
    ))
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
    // This local-owner API has no browser session or CORS contract. Reject browser origins.
    if headers.contains_key("origin") {
        return Err(ApiError(PlatformError::Unauthorized));
    }
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
