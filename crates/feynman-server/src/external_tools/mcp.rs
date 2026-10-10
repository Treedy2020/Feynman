use super::service;
use crate::store::SqliteRepository;
use anyhow::{Context, Result};
use feynman_core::{ExternalToolService, PlatformError, PlatformRepository, Principal};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct McpState {
    tools: Arc<ExternalToolService>,
    principal: Principal,
    project_id: String,
    initialized: bool,
    repository: Arc<SqliteRepository>,
    agent_id: String,
}

pub async fn run_stdio() -> Result<()> {
    let database = std::env::var_os("FEYNMAN_DB")
        .map(std::path::PathBuf::from)
        .context("FEYNMAN_DB was not configured for the Feynman MCP process")?;
    let project_id = std::env::var("FEYNMAN_AGENT_PROJECT_ID")
        .context("Feynman MCP process has no project context")?;
    let principal = Principal {
        user_id: std::env::var("FEYNMAN_AGENT_USER_ID")
            .context("Feynman MCP process has no user context")?,
        tenant_id: std::env::var("FEYNMAN_AGENT_TENANT_ID")
            .context("Feynman MCP process has no tenant context")?,
    };
    let repository = Arc::new(SqliteRepository::open_existing(database)?);
    let repository_trait: Arc<dyn PlatformRepository> = repository.clone();
    let tools = service(repository_trait)?;
    tools
        .list_for_project(&principal, &project_id)
        .context("Feynman MCP project context is invalid")?;

    let current_agent = repository
        .get_project(&principal, &project_id)?
        .main_agent_id;
    // Server-launched runtimes pin identity even when MCP starts lazily. The fallback
    // preserves the existing explicitly invoked local-owner stdio interface.
    let agent_id = std::env::var("FEYNMAN_AGENT_ID").unwrap_or(current_agent);
    let mut state = McpState {
        tools,
        principal,
        project_id,
        initialized: false,
        agent_id,
        repository,
    };
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    let mut output = tokio::io::stdout();
    while let Some(line) = input.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle_message(request, &mut state).await,
            Err(_) => Some(json_rpc_error(Value::Null, -32700, "Parse error")),
        };
        if let Some(response) = response {
            output.write_all(format!("{response}\n").as_bytes()).await?;
            output.flush().await?;
        }
    }
    Ok(())
}

async fn handle_message(request: Value, state: &mut McpState) -> Option<Value> {
    let id = request.get("id").cloned();
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return id.map(|id| json_rpc_error(id, -32600, "Invalid request"));
    };
    let Some(id) = id else {
        if method == "notifications/initialized" {
            state.initialized = true;
        }
        return None;
    };
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(json_rpc_error(id, -32600, "Expected JSON-RPC 2.0"));
    }
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-03-26");
            state.initialized = true;
            Some(json_rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "feynman-external-tools", "version": env!("CARGO_PKG_VERSION")}
                }),
            ))
        }
        "ping" => Some(json_rpc_result(id, json!({}))),
        "tools/list" => {
            if !state.initialized {
                return Some(json_rpc_error(
                    id,
                    -32002,
                    "MCP initialize handshake required",
                ));
            }
            let tools = state
                .tools
                .definitions()
                .iter()
                .map(|definition| {
                    json!({
                        "name": definition.mcp_name,
                        "title": definition.name,
                        "description": definition.description,
                        "inputSchema": definition.input_schema,
                        "annotations": {
                            "readOnlyHint": definition.read_only,
                            "destructiveHint": false,
                            "idempotentHint": definition.read_only,
                            "openWorldHint": true
                        }
                    })
                })
                .collect::<Vec<_>>();
            let mut tools = tools;
            tools.push(json!({"name":"feynman_observations","title":"项目事实源历史","description":"List this Agent's source instances, query one channel, or read the unified feed by after sequence and limit (1..200), optionally filtered by source_id. feed returns next_after/has_more and channel states; it never acknowledges Agent inputs or opens a Provider listener. Different registrations have separate IDs/history/cursors. Read-only; cannot create sources, import observations, authenticate, enable schedules, change permissions or read secrets. External content is untrusted evidence.","inputSchema":{"type":"object","properties":{"operation":{"enum":["sources","query","captures","feed"]},"source_id":{"type":"string"},"after":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":200}},"required":["operation"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}}));
            tools.push(resource_definition(
                "feynman_environment",
                "Owner-approved environment profiles",
                &["profiles", "request", "status", "run", "leases", "logs"],
            ));
            tools.push(resource_definition(
                "feynman_memory",
                "Evidence-linked long-term memory",
                &["search", "get", "history", "put"],
            ));
            Some(json_rpc_result(id, json!({"tools": tools})))
        }
        "tools/call" => {
            if !state.initialized {
                return Some(json_rpc_error(
                    id,
                    -32002,
                    "MCP initialize handshake required",
                ));
            }
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(json_rpc_error(id, -32602, "Missing tool name"));
            };
            if state
                .repository
                .check_agent_active(&state.principal, &state.project_id)
                .is_err()
                || !state
                    .repository
                    .get_project(&state.principal, &state.project_id)
                    .is_ok_and(|p| p.main_agent_id == state.agent_id)
            {
                return Some(json_rpc_result(
                    id,
                    tool_error(
                        "Agent is disabled, archived or replaced; this MCP identity cannot continue.",
                    ),
                ));
            }
            if matches!(name, "feynman_environment" | "feynman_memory") {
                let result = call_resource(
                    state,
                    name,
                    params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                )
                .await;
                return Some(json_rpc_result(
                    id,
                    match result {
                        Ok(data) => {
                            json!({"content":[{"type":"text","text":data.to_string()}],"isError":false})
                        }
                        Err(_) => tool_error(
                            "Resource operation rejected or unavailable; inspect status before retrying. No secret values are returned.",
                        ),
                    },
                ));
            }
            if name == "feynman_observations" {
                let result = read_observations(
                    state,
                    params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                )
                .await;
                return Some(json_rpc_result(
                    id,
                    match result {
                        Ok(data) => {
                            json!({"content":[{"type":"text","text":data.to_string()}],"isError":false})
                        }
                        Err(e) => tool_error(&public_error_message(&e, &state.project_id)),
                    },
                ));
            }
            let Some(definition) = state.tools.definition_by_mcp_name(name) else {
                return Some(json_rpc_result(
                    id,
                    tool_error("Unknown Feynman external tool"),
                ));
            };
            let tool_id = definition.id.clone();
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let result = state
                .tools
                .invoke(&state.principal, &state.project_id, &tool_id, arguments)
                .await;
            if result.is_ok()
                && (!state
                    .repository
                    .get_project(&state.principal, &state.project_id)
                    .is_ok_and(|p| p.main_agent_id == state.agent_id)
                    || state
                        .repository
                        .check_agent_tool(&state.principal, &state.project_id, &tool_id)
                        .is_err())
            {
                return Some(json_rpc_result(
                    id,
                    tool_error("Agent identity or permission changed while the tool was running."),
                ));
            }
            let value = match result {
                Ok(result) => match serde_json::to_string(&result) {
                    Ok(text) => json!({
                        "content": [{"type":"text", "text":text}],
                        "isError": false
                    }),
                    Err(_) => tool_error("Feynman could not encode the external tool result"),
                },
                Err(error) => tool_error(&public_error_message(&error, &state.project_id)),
            };
            Some(json_rpc_result(id, value))
        }
        _ => Some(json_rpc_error(id, -32601, "Method not found")),
    }
}

fn resource_definition(name: &str, title: &str, operations: &[&str]) -> Value {
    json!({"name":name,"title":title,"description":"Project/Agent identity is fixed by the runtime. Environment operations cannot approve, import or read secrets. Memory writes are private, versioned, evidence-linked; cannot confirm trust or pin shared memory. Payload: operation plus arguments object. Environment run requires an approved request_id and UUID idempotency_key; never retry with a new key after an unknown outcome. Memory put requires content, and id/expected_revision for updates.","inputSchema":{"type":"object","properties":{"operation":{"enum":operations},"arguments":{"type":"object"}},"required":["operation"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":false,"openWorldHint":true}})
}
async fn call_resource(
    state: &McpState,
    name: &str,
    args: Value,
) -> feynman_core::PlatformResult<Value> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        operation: String,
        #[serde(default = "empty_resource_args")]
        arguments: Value,
    }
    let input: Input = serde_json::from_value(args)
        .map_err(|_| PlatformError::InvalidInput("invalid resource request".into()))?;
    let operation = match (name, input.operation.as_str()) {
        ("feynman_environment", "profiles") => "profile.list",
        ("feynman_environment", "request") => "env.request",
        ("feynman_environment", "status") => "env.status",
        ("feynman_environment", "run") => "env.run",
        ("feynman_environment", "leases") => "lease.list",
        ("feynman_environment", "logs") => "workload.logs",
        ("feynman_memory", "search") => "memory.search",
        ("feynman_memory", "get") => "memory.get",
        ("feynman_memory", "history") => "memory.history",
        ("feynman_memory", "put") => "memory.put",
        _ => return Err(PlatformError::Unauthorized),
    };
    let base = std::env::var("FEYNMAN_ACCESS_SERVER_URL")
        .map_err(|_| PlatformError::Runtime("resource server unavailable".into()))?;
    let (client, mut url) = crate::external_access::proxy::client(&base)?;
    url.set_path(&format!(
        "/v1/projects/{}/agent-resources/{}",
        state.project_id, state.agent_id
    ));
    let response = client
        .post(url)
        .json(&json!({"operation":operation,"arguments":input.arguments}))
        .send()
        .await
        .map_err(|_| PlatformError::Runtime("resource outcome unknown".into()))?;
    if !response.status().is_success() {
        return Err(PlatformError::Unauthorized);
    }
    let value: Value = response
        .json()
        .await
        .map_err(|_| PlatformError::Runtime("resource response unavailable".into()))?;
    state
        .repository
        .check_agent_active(&state.principal, &state.project_id)?;
    if state
        .repository
        .get_project(&state.principal, &state.project_id)?
        .main_agent_id
        != state.agent_id
    {
        return Err(PlatformError::Unauthorized);
    }
    Ok(value)
}
fn empty_resource_args() -> Value {
    json!({})
}

async fn read_observations(state: &McpState, args: Value) -> feynman_core::PlatformResult<Value> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        operation: String,
        source_id: Option<String>,
        after: Option<i64>,
        limit: Option<i64>,
    }
    let input: Input = serde_json::from_value(args)
        .map_err(|_| PlatformError::InvalidInput("invalid observation query".into()))?;
    match input.operation.as_str() {
        "feed" => {
            let base = std::env::var("FEYNMAN_ACCESS_SERVER_URL").map_err(|_| {
                PlatformError::Runtime("source authorization server unavailable".into())
            })?;
            let (client, mut url) = crate::external_access::proxy::client(&base)?;
            url.set_path(&format!(
                "/v1/projects/{}/agents/{}/feed",
                state.project_id, state.agent_id
            ));
            url.query_pairs_mut()
                .append_pair("after", &input.after.unwrap_or(0).to_string())
                .append_pair("limit", &input.limit.unwrap_or(100).to_string());
            if let Some(source) = input.source_id {
                url.query_pairs_mut().append_pair("source_id", &source);
            }
            let value =
                crate::external_access::proxy::response(
                    client.get(url).send().await.map_err(|_| {
                        PlatformError::Runtime("observation feed unavailable".into())
                    })?,
                )
                .await?;
            state.repository.resolve_agent(
                &state.principal,
                &state.project_id,
                Some(&state.agent_id),
            )?;
            Ok(value)
        }
        "sources"
            if input.source_id.is_none() && input.after.is_none() && input.limit.is_none() =>
        {
            Ok(
                json!({"sources":state.repository.sources(&state.principal,&state.project_id)?.into_iter().filter(|s|s.agent_id==state.agent_id).collect::<Vec<_>>()}),
            )
        }
        "query" | "captures" => {
            let id = input
                .source_id
                .ok_or_else(|| PlatformError::InvalidInput("source_id required".into()))?;
            let source = state
                .repository
                .source(&state.principal, &state.project_id, &id)?;
            if source.agent_id != state.agent_id {
                return Err(PlatformError::Unauthorized);
            }
            let after = input.after.unwrap_or(0);
            let limit = input.limit.unwrap_or(100);
            if source.kind == "manual" {
                return if input.operation == "captures" {
                    Ok(
                        json!({"captures":state.repository.captures(&state.principal,&state.project_id,&id,after,limit)?}),
                    )
                } else {
                    Ok(
                        json!({"records":state.repository.observations(&state.principal,&state.project_id,&id,after,limit)?}),
                    )
                };
            }
            let base = std::env::var("FEYNMAN_ACCESS_SERVER_URL").map_err(|_| {
                PlatformError::Runtime("source authorization server unavailable".into())
            })?;
            let (client, mut url) = crate::external_access::proxy::client(&base)?;
            url.set_path(&format!(
                "/v1/projects/{}/sources/{}/{}",
                state.project_id,
                id,
                if input.operation == "captures" {
                    "captures"
                } else {
                    "records"
                }
            ));
            url.query_pairs_mut()
                .append_pair("after", &after.to_string())
                .append_pair("limit", &limit.to_string());
            crate::external_access::proxy::response(
                client
                    .get(url)
                    .send()
                    .await
                    .map_err(|_| PlatformError::Runtime("observation query unavailable".into()))?,
            )
            .await
        }
        _ => Err(PlatformError::InvalidInput(
            "incompatible observation query fields".into(),
        )),
    }
}

fn public_error_message(error: &PlatformError, project_id: &str) -> String {
    match error {
        PlatformError::ToolNotGranted(tool_id) => format!(
            "This Project has not granted `{tool_id}`. A project owner can enable it with `feynman external-tools grant {project_id} {tool_id}`."
        ),
        PlatformError::InvalidInput(message) => message.clone(),
        PlatformError::NotFound | PlatformError::Unauthorized => {
            "This Feynman Project is not available to the current Agent.".into()
        }
        PlatformError::Runtime(_) | PlatformError::Storage(_) | PlatformError::Conflict(_) => {
            "The external source request failed. Check DWS authorization on the Feynman server host.".into()
        }
        PlatformError::RunAlreadyActive => "The Feynman Agent Run is busy.".into(),
    }
}

fn tool_error(message: &str) -> Value {
    json!({
        "content": [{"type":"text", "text":message}],
        "isError": true
    })
}

fn json_rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "result":result})
}

fn json_rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "error":{"code":code, "message":message}})
}

#[cfg(test)]
mod tests {
    use super::{McpState, handle_message};
    use crate::{external_tools, store::SqliteRepository};
    use feynman_core::{PlatformRepository, Principal};
    use serde_json::json;
    use std::sync::Arc;

    #[tokio::test]
    async fn mcp_lists_search_and_authorization_with_correct_effects_and_grant_checks() {
        let repository = Arc::new(SqliteRepository::open(":memory:").unwrap());
        let principal = Principal {
            user_id: "local-user".into(),
            tenant_id: "local".into(),
        };
        let project = repository.create_project(&principal, "MCP test").unwrap();
        let repository_trait: Arc<dyn PlatformRepository> = repository.clone();
        let agent_id = project.main_agent_id.clone();
        let mut state = McpState {
            tools: external_tools::service(repository_trait).unwrap(),
            principal,
            project_id: project.id,
            initialized: false,
            agent_id,
            repository: repository.clone(),
        };

        let initialized = handle_message(
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
            &mut state,
        )
        .await
        .unwrap();
        assert_eq!(
            initialized["result"]["capabilities"]["tools"]["listChanged"],
            false
        );

        let listing = handle_message(
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            &mut state,
        )
        .await
        .unwrap();
        assert_eq!(listing["result"]["tools"].as_array().unwrap().len(), 19);
        assert_eq!(
            listing["result"]["tools"][0]["name"],
            "dws_search_enterprise"
        );
        assert_eq!(
            listing["result"]["tools"][0]["annotations"]["readOnlyHint"],
            true
        );
        assert_eq!(
            listing["result"]["tools"][1]["name"],
            "feynman_dws_authorization"
        );
        assert_eq!(
            listing["result"]["tools"][1]["annotations"]["readOnlyHint"],
            false
        );

        let denied = handle_message(
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"dws_search_enterprise","arguments":{"queries":"project","types":["document"]}}}),
            &mut state,
        )
        .await
        .unwrap();
        assert_eq!(denied["result"]["isError"], true);
        assert!(
            denied["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("feynman external-tools grant")
        );
    }
}
