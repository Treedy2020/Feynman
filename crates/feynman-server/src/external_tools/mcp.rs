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
    let repository = Arc::new(SqliteRepository::open(database)?);
    let repository_trait: Arc<dyn PlatformRepository> = repository.clone();
    let tools = service(repository_trait)?;
    tools
        .list_for_project(&principal, &project_id)
        .context("Feynman MCP project context is invalid")?;

    let mut state = McpState {
        tools,
        principal,
        project_id,
        initialized: false,
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
                            "readOnlyHint": true,
                            "destructiveHint": false,
                            "idempotentHint": true,
                            "openWorldHint": true
                        }
                    })
                })
                .collect::<Vec<_>>();
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
    async fn mcp_lists_only_the_fixed_read_only_tool_and_denies_it_without_a_grant() {
        let repository = Arc::new(SqliteRepository::open(":memory:").unwrap());
        let principal = Principal {
            user_id: "local-user".into(),
            tenant_id: "local".into(),
        };
        let project = repository.create_project(&principal, "MCP test").unwrap();
        let repository_trait: Arc<dyn PlatformRepository> = repository.clone();
        let mut state = McpState {
            tools: external_tools::service(repository_trait).unwrap(),
            principal,
            project_id: project.id,
            initialized: false,
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
        assert_eq!(listing["result"]["tools"].as_array().unwrap().len(), 1);
        assert_eq!(
            listing["result"]["tools"][0]["name"],
            "dws_search_enterprise"
        );
        assert_eq!(
            listing["result"]["tools"][0]["annotations"]["readOnlyHint"],
            true
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
