//! Same-origin local browser adapter. CLI routes keep their no-Origin contract.
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc};
use tower::ServiceExt;

#[derive(Clone)]
struct Console {
    api: Router,
    token: Arc<String>,
    hosts: Arc<Vec<String>>,
}

pub fn router(api: Router, bind: SocketAddr) -> Router {
    let console = Console {
        api: api.clone(),
        token: Arc::new(uuid::Uuid::new_v4().to_string()),
        hosts: Arc::new(vec![bind.to_string(), format!("localhost:{}", bind.port())]),
    };
    Router::new()
        .route(
            "/",
            get(|| async { axum::response::Redirect::temporary("/console/") }),
        )
        .route(
            "/console",
            get(|| async { axum::response::Redirect::temporary("/console/") }),
        )
        .route("/console/", get(index))
        .route("/console/app.js", get(javascript))
        .route("/console/stream.js", get(stream_javascript))
        .route("/console/app.css", get(stylesheet))
        .route("/console/session", get(session))
        .route("/console/api/{*path}", any(dispatch))
        .with_state(console.clone())
        .merge(api)
        .layer(middleware::from_fn_with_state(console, host_guard))
}

fn denied() -> Response {
    (StatusCode::FORBIDDEN, Json(json!({"error":{"message":"Local Console request rejected; reload the same-origin page."}}))).into_response()
}

async fn host_guard(State(s): State<Console>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    let mut response = if !host.is_some_and(|h| s.hosts.iter().any(|v| v == h)) {
        denied()
    } else {
        next.run(req).await
    };
    for (key, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    ] {
        response
            .headers_mut()
            .insert(key, HeaderValue::from_static(value));
    }
    response
}

fn same_origin(req: &Request, require_origin: bool) -> bool {
    let h = req.headers();
    let Some(host) = h.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    if h.contains_key(header::AUTHORIZATION) {
        return false;
    }
    if h.get("sec-fetch-site")
        .is_some_and(|v| v != "same-origin" && v != "none")
    {
        return false;
    }
    match h.get(header::ORIGIN) {
        Some(origin) => origin.as_bytes() == format!("http://{host}").as_bytes(),
        None => !require_origin,
    }
}

async fn session(State(s): State<Console>, req: Request) -> Response {
    if !same_origin(&req, false) {
        return denied();
    }
    Json(json!({"csrf_token":&*s.token,"mode":"local-owner","poll_ms":2000})).into_response()
}

/// Only public owner DTOs needed by this UI; no raw tokens, arbitrary invoke,
/// private filesystem, agent-resources, snapshot import or secret registration.
fn allowed(method: &Method, path: &str) -> bool {
    if path
        .bytes()
        .any(|b| !b.is_ascii_alphanumeric() && !b"/._-%".contains(&b))
    {
        return false;
    }
    let parts: Vec<_> = path.split('/').collect();
    if method == Method::GET {
        return matches!(
            parts.as_slice(),
            ["v1", "health" | "status" | "projects"]
                | ["v1", "notifications"]
                | ["v1", "notifications", "stream"]
                | ["v1", "access", "providers" | "connections"]
                | [
                    "v1",
                    "projects",
                    _,
                    "status"
                        | "agents"
                        | "conversation"
                        | "tasks"
                        | "schedules"
                        | "sources"
                        | "feed"
                        | "inbox"
                        | "workspace"
                        | "external-tools"
                        | "access"
                        | "dws-authorizations"
                        | "stream"
                ]
                | ["v1", "projects", _, "sources", _, "records" | "captures"]
                | ["v1", "projects", _, "schedules", _, "deliveries"]
                | ["v1", "projects", _, "runs", _, "events"]
                | ["v1", "projects", _, "dws-authorizations", _]
        );
    }
    if method == Method::POST {
        return matches!(
            parts.as_slice(),
            ["v1", "projects"]
                | ["v1", "notifications", _, "read"]
                | [
                    "v1",
                    "projects",
                    _,
                    "inbox"
                        | "tasks"
                        | "schedules"
                        | "agents"
                        | "sources"
                        | "workspace"
                        | "dws-authorizations"
                        | "resources"
                ]
                | ["v1", "projects", _, "workspace", "actions"]
                | ["v1", "projects", _, "access", "bindings"]
                | ["v1", "projects", _, "dws-authorizations", _, "check"]
                | ["v1", "projects", _, "runs", _, "approvals", _]
        );
    }
    if method == Method::PUT {
        return matches!(
            parts.as_slice(),
            [
                "v1",
                "projects",
                _,
                "agents" | "schedules" | "sources" | "external-tools",
                _
            ]
        );
    }
    method == Method::DELETE
        && matches!(
            parts.as_slice(),
            [
                "v1",
                "projects",
                _,
                "tasks" | "external-tools" | "dws-authorizations",
                _
            ] | ["v1", "projects", _, "access", "bindings", _]
        )
}

async fn dispatch(State(s): State<Console>, mut req: Request) -> Response {
    let path = req
        .uri()
        .path()
        .strip_prefix("/console/api/")
        .unwrap_or("")
        .to_owned();
    let write = req.method() != Method::GET;
    if !same_origin(&req, write)
        || req.headers().get("x-feynman-console").map(|v| v.as_bytes()) != Some(s.token.as_bytes())
        || !allowed(req.method(), &path)
    {
        return denied();
    }
    if write {
        if req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            != Some("application/json")
        {
            return denied();
        }
        let body = std::mem::replace(req.body_mut(), Body::empty());
        let Ok(bytes) = to_bytes(body, 196608).await else {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({"error":{"message":"Console request too large"}})),
            )
                .into_response();
        };
        if path.ends_with("/resources") {
            let Ok(v) = serde_json::from_slice::<Value>(&bytes) else {
                return denied();
            };
            if !matches!(
                v["operation"].as_str(),
                Some(
                    "profile.list"
                        | "env.status"
                        | "lease.list"
                        | "memory.search"
                        | "memory.get"
                        | "memory.history"
                )
            ) {
                return denied();
            }
        }
        *req.body_mut() = Body::from(bytes);
    }
    let target = match req.uri().query() {
        Some(q) => format!("/{path}?{q}"),
        None => format!("/{path}"),
    };
    let Ok(uri) = target.parse::<Uri>() else {
        return denied();
    };
    *req.uri_mut() = uri;
    // An authenticated internal adapter call, not a relaxed CLI Origin policy.
    req.headers_mut().remove(header::ORIGIN);
    req.headers_mut().remove("x-feynman-console");
    // Do not carry the outer wildcard's UrlParams into the inner route matcher.
    // The inner API establishes its own route/extractor context.
    let (mut parts, body) = req.into_parts();
    parts.extensions.clear();
    s.api
        .oneshot(Request::from_parts(parts, body))
        .await
        .unwrap_or_else(|_| denied())
}

async fn index() -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("console/index.html"),
    )
        .into_response()
}
async fn javascript() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("console/app.js"),
    )
        .into_response()
}
async fn stream_javascript() -> Response {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], include_str!("console/stream.js")).into_response()
}
async fn stylesheet() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("console/app.css"),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    fn app() -> Router {
        router(
            Router::new().route(
                "/v1/projects",
                get(|| async { Json(json!({"projects":[]})) })
                    .post(|| async { StatusCode::CREATED }),
            ),
            "127.0.0.1:4318".parse().unwrap(),
        )
    }
    async fn token(app: &Router) -> String {
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/console/session")
                    .header("host", "127.0.0.1:4318")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        serde_json::from_slice::<Value>(&to_bytes(r.into_body(), 1000).await.unwrap()).unwrap()["csrf_token"].as_str().unwrap().into()
    }
    #[tokio::test]
    async fn console_rejects_rebinding_cross_origin_and_missing_csrf() {
        let a = app();
        let t = token(&a).await;
        for (host, origin, csrf) in [
            ("evil.test:4318", "http://evil.test:4318", t.as_str()),
            ("127.0.0.1:4318", "https://evil.test", t.as_str()),
            ("127.0.0.1:4318", "http://127.0.0.1:4318", ""),
            ("127.0.0.1:4318", "null", t.as_str()),
        ] {
            let r = a
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/console/api/v1/projects")
                        .header("host", host)
                        .header("origin", origin)
                        .header("x-feynman-console", csrf)
                        .header("content-type", "application/json")
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::FORBIDDEN);
        }
        let r = a
            .oneshot(
                Request::builder()
                    .uri("/console/session")
                    .header("host", "127.0.0.1:4318")
                    .header("sec-fetch-site", "cross-site")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }
    #[tokio::test]
    async fn console_routes_fixed_calls_and_excludes_secrets_and_arbitrary_execution() {
        let a = app();
        let t = token(&a).await;
        let request = |path: &str| {
            Request::builder()
                .method("POST")
                .uri(path)
                .header("host", "127.0.0.1:4318")
                .header("origin", "http://127.0.0.1:4318")
                .header("x-feynman-console", &t)
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap()
        };
        assert_eq!(
            a.clone()
                .oneshot(request("/console/api/v1/projects"))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        for path in [
            "/console/api/v1/access/connections",
            "/console/api/v1/projects/a/access/invoke/git.workspace.clone",
            "/console/api/v1/projects/a/agent-resources/b",
            "/console/api/v1/projects/a/workspace/seed",
            "/console/api/v1/projects/a/resources",
        ] {
            assert_eq!(
                a.clone().oneshot(request(path)).await.unwrap().status(),
                StatusCode::FORBIDDEN
            );
        }
        let r = a
            .oneshot(
                Request::builder()
                    .uri("/console/")
                    .header("host", "127.0.0.1:4318")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.headers()["x-frame-options"], "DENY");
        assert!(
            r.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("frame-ancestors 'none'")
        );
    }
}
