use super::*;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};

struct ApiError(PlatformError);
impl From<PlatformError> for ApiError {
    fn from(e: PlatformError) -> Self {
        Self(e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self.0 {
            PlatformError::NotFound => (StatusCode::NOT_FOUND, "not found".to_owned()),
            PlatformError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".into()),
            PlatformError::ToolNotGranted(id) => {
                (StatusCode::FORBIDDEN, format!("tool not granted: {id}"))
            }
            PlatformError::InvalidInput(message) => (StatusCode::BAD_REQUEST, message),
            PlatformError::Conflict(message) => (StatusCode::CONFLICT, message),
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "External Access operation failed".into(),
            ),
        };
        (status, Json(json!({"error":{"message":message}}))).into_response()
    }
}
fn principal(headers: &HeaderMap) -> Result<Principal, ApiError> {
    if headers.contains_key("authorization") || headers.contains_key("origin") {
        return Err(PlatformError::Unauthorized.into());
    }
    Ok(Principal {
        tenant_id: "local".into(),
        user_id: "local-user".into(),
    })
}
fn body<T>(input: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    input
        .map(|Json(v)| v)
        .map_err(|_| invalid("invalid External Access request").into())
}

pub fn router(broker: Arc<ExternalAccessBroker>) -> Router {
    Router::new()
        .route("/v1/access/providers", get(catalog))
        .route("/v1/access/connections", get(connections).post(register))
        .route("/v1/access/connections/{id}/disable", post(disable))
        .route("/v1/projects/{project}/access", get(status))
        .route("/v1/projects/{project}/access/requests", post(request))
        .route("/v1/projects/{project}/access/bindings", post(bind))
        .route(
            "/v1/projects/{project}/access/dws-scope",
            axum::routing::put(dws_scope),
        )
        .route(
            "/v1/projects/{project}/access/bindings/{provider}",
            axum::routing::delete(unbind),
        )
        .route("/v1/projects/{project}/access/invoke/{tool}", post(invoke))
        .with_state(broker)
        .layer(DefaultBodyLimit::max(16384))
        .layer(axum::middleware::map_response(
            |mut response: Response| async move {
                response
                    .headers_mut()
                    .insert("cache-control", "no-store".parse().unwrap());
                response
                    .headers_mut()
                    .insert("referrer-policy", "no-referrer".parse().unwrap());
                response
            },
        ))
}
async fn catalog(
    State(b): State<Arc<ExternalAccessBroker>>,
    h: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    principal(&h)?;
    Ok(Json(
        json!({"providers":b.providers(),"security":security::descriptor()}),
    ))
}
async fn connections(
    State(b): State<Arc<ExternalAccessBroker>>,
    h: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({"connections":b.connections(&principal(&h)?)?})))
}
async fn register(
    State(b): State<Arc<ExternalAccessBroker>>,
    h: HeaderMap,
    input: Result<Json<RegisterConnection>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let p = principal(&h)?;
    Ok(Json(
        json!({"connection":b.register(&p,body(input)?).await?}),
    ))
}
async fn disable(
    State(b): State<Arc<ExternalAccessBroker>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    b.disable(&principal(&h)?, &id)?;
    Ok(Json(
        json!({"disabled":true,"provider_token_revoked":false}),
    ))
}
async fn status(
    State(b): State<Arc<ExternalAccessBroker>>,
    h: HeaderMap,
    Path(project): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(b.status(&principal(&h)?, &project)?))
}
async fn request(
    State(b): State<Arc<ExternalAccessBroker>>,
    h: HeaderMap,
    Path(project): Path<String>,
    input: Result<Json<AccessRequestInput>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let p = principal(&h)?;
    Ok(Json(b.request(&p, &project, body(input)?)?))
}
async fn bind(
    State(b): State<Arc<ExternalAccessBroker>>,
    h: HeaderMap,
    Path(project): Path<String>,
    input: Result<Json<BindRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let p = principal(&h)?;
    Ok(Json(
        json!({"binding":b.bind(&p,&project,body(input)?)?,"business_grant_created":false}),
    ))
}
async fn dws_scope(
    State(b): State<Arc<ExternalAccessBroker>>,
    h: HeaderMap,
    Path(project): Path<String>,
    input: Result<Json<providers::dws::messages::Scope>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let p = principal(&h)?;
    b.configure_dws_scope(&p, &project, body(input)?)?;
    Ok(Json(
        json!({"scope":b.dws_scope(&p,&project)?,"business_grant_created":false}),
    ))
}
async fn unbind(
    State(b): State<Arc<ExternalAccessBroker>>,
    h: HeaderMap,
    Path((project, provider)): Path<(String, Provider)>,
) -> Result<StatusCode, ApiError> {
    b.unbind(&principal(&h)?, &project, provider)?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Invoke {
    arguments: Value,
}
async fn invoke(
    State(b): State<Arc<ExternalAccessBroker>>,
    h: HeaderMap,
    Path((project, tool)): Path<(String, String)>,
    input: Result<Json<Invoke>, JsonRejection>,
) -> Result<Json<feynman_core::ExternalToolResult>, ApiError> {
    let p = principal(&h)?;
    Ok(Json(feynman_core::ExternalToolResult {
        source: tool.clone(),
        observed_at: Utc::now(),
        data: b
            .invoke(&p, &project, &tool, body(input)?.arguments)
            .await?,
    }))
}
