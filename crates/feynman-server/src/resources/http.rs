use super::*;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    operation: String,
    #[serde(default = "empty")]
    arguments: Value,
}
fn empty() -> Value {
    json!({})
}
pub fn router(manager: Arc<ResourceManager>) -> Router {
    Router::new()
        .route("/v1/projects/{project}/resources", post(owner))
        .route(
            "/v1/projects/{project}/agent-resources/{agent}",
            post(agent),
        )
        .with_state(manager)
        .layer(axum::extract::DefaultBodyLimit::max(196608))
        .layer(axum::middleware::map_response(
            |mut r: Response| async move {
                if r.status() == StatusCode::UNPROCESSABLE_ENTITY {
                    r = (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        Json(json!({"error":"invalid resource request"})),
                    )
                        .into_response();
                }
                r.headers_mut().insert(
                    "cache-control",
                    axum::http::HeaderValue::from_static("no-store"),
                );
                r
            },
        ))
}
fn principal(h: &HeaderMap) -> PlatformResult<Principal> {
    if h.contains_key("origin") || h.contains_key("authorization") {
        return Err(PlatformError::Unauthorized);
    }
    Ok(Principal {
        tenant_id: "local".into(),
        user_id: "local-user".into(),
    })
}
async fn owner(
    State(m): State<Arc<ResourceManager>>,
    Path(project): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> Result<Json<Value>, Error> {
    let p = principal(&headers)?;
    Ok(Json(
        m.call(&p, &project, None, &input.operation, input.arguments)
            .await?,
    ))
}
async fn agent(
    State(m): State<Arc<ResourceManager>>,
    Path((project, agent)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> Result<Json<Value>, Error> {
    let p = principal(&headers)?;
    if !matches!(
        input.operation.as_str(),
        "profile.list"
            | "env.request"
            | "env.status"
            | "env.run"
            | "lease.list"
            | "workload.logs"
            | "memory.search"
            | "memory.get"
            | "memory.history"
            | "memory.put"
    ) {
        return Err(Error(PlatformError::Unauthorized));
    }
    Ok(Json(
        m.call(
            &p,
            &project,
            Some(&agent),
            &input.operation,
            input.arguments,
        )
        .await?,
    ))
}
struct Error(PlatformError);
impl From<PlatformError> for Error {
    fn from(e: PlatformError) -> Self {
        Self(e)
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match self.0 {
            PlatformError::NotFound => (
                StatusCode::NOT_FOUND,
                "resource not found or inaccessible".into(),
            ),
            PlatformError::Unauthorized => (
                StatusCode::FORBIDDEN,
                "resource operation is not authorized".into(),
            ),
            PlatformError::InvalidInput(s) => (StatusCode::BAD_REQUEST, s),
            PlatformError::Conflict(s) => (StatusCode::CONFLICT, s),
            _ => (
                StatusCode::SERVICE_UNAVAILABLE,
                "resource operation unavailable; inspect status before retrying".into(),
            ),
        };
        (status, Json(json!({"error":message}))).into_response()
    }
}
