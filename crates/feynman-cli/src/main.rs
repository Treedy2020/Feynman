use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use feynman_core::{
    AgentRunStatus, ChatEvent, ConversationSnapshot, Project, ProjectToolAccess, RunFeed,
    SendMessageResponse, ToolGrant,
};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::{self, Write};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::{Duration, sleep};

#[derive(Parser)]
#[command(
    name = "feynman",
    version,
    about = "Manage Feynman projects and talk with their Main Agents"
)]
struct Cli {
    #[arg(
        long,
        env = "FEYNMAN_SERVER_URL",
        default_value = "http://127.0.0.1:4317",
        global = true
    )]
    server: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Projects {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    ExternalTools {
        #[command(subcommand)]
        command: ExternalToolCommand,
    },
    Chat {
        project_id: String,
    },
    Conversation {
        project_id: String,
    },
    Health,
}

#[derive(Subcommand)]
enum ProjectCommand {
    List,
    Create {
        #[arg(long)]
        name: String,
    },
    Show {
        project_id: String,
    },
}

#[derive(Subcommand)]
enum ExternalToolCommand {
    List { project_id: String },
    Grant { project_id: String, tool_id: String },
    Revoke { project_id: String, tool_id: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = Client::new();
    let base = cli.server.trim_end_matches('/').to_owned();
    match cli.command {
        Command::Health => {
            let response: Value = get(&client, &format!("{base}/v1/health")).await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Command::Projects { command } => match command {
            ProjectCommand::List => {
                let response: Value = get(&client, &format!("{base}/v1/projects")).await?;
                let projects = response
                    .get("projects")
                    .and_then(Value::as_array)
                    .context("server returned no projects array")?;
                if projects.is_empty() {
                    println!(
                        "No projects yet. Create one with `feynman projects create --name …`."
                    );
                }
                for item in projects {
                    let project: Project = serde_json::from_value(item.clone())?;
                    println!(
                        "{}\t{}\t{}",
                        project.id,
                        project.name,
                        project.updated_at.to_rfc3339()
                    );
                }
            }
            ProjectCommand::Create { name } => {
                let response: Value = post(
                    &client,
                    &format!("{base}/v1/projects"),
                    &json!({"name":name}),
                )
                .await?;
                let project: Project = serde_json::from_value(
                    response
                        .get("project")
                        .context("server returned no project")?
                        .clone(),
                )?;
                println!("Created {} ({})", project.name, project.id);
            }
            ProjectCommand::Show { project_id } => {
                let response: Value =
                    get(&client, &format!("{base}/v1/projects/{project_id}")).await?;
                println!("{}", serde_json::to_string_pretty(&response)?);
            }
        },
        Command::ExternalTools { command } => match command {
            ExternalToolCommand::List { project_id } => {
                let response: Value = get(
                    &client,
                    &format!("{base}/v1/projects/{project_id}/external-tools"),
                )
                .await?;
                let tools = response
                    .get("tools")
                    .and_then(Value::as_array)
                    .context("server returned no tools array")?;
                if tools.is_empty() {
                    println!("No external tools are registered.");
                }
                for item in tools {
                    let tool: ProjectToolAccess = serde_json::from_value(item.clone())?;
                    println!(
                        "{}\t{}\t{}",
                        tool.id,
                        if tool.granted {
                            "granted"
                        } else {
                            "not granted"
                        },
                        tool.name
                    );
                    println!("  {}", tool.description);
                }
            }
            ExternalToolCommand::Grant {
                project_id,
                tool_id,
            } => {
                let grant: ToolGrant = put(
                    &client,
                    &format!("{base}/v1/projects/{project_id}/external-tools/{tool_id}"),
                )
                .await?;
                println!("Granted {} to project {}", grant.tool_id, grant.project_id);
            }
            ExternalToolCommand::Revoke {
                project_id,
                tool_id,
            } => {
                let response = client
                    .delete(format!(
                        "{base}/v1/projects/{project_id}/external-tools/{tool_id}"
                    ))
                    .send()
                    .await?;
                check_response(response).await?;
                println!("Revoked {tool_id} from project {project_id}");
            }
        },
        Command::Conversation { project_id } => {
            let snapshot: ConversationSnapshot = get(
                &client,
                &format!("{base}/v1/projects/{project_id}/conversation"),
            )
            .await?;
            for message in snapshot.messages {
                println!(
                    "{}: {}",
                    if message.role == feynman_core::MessageRole::User {
                        "you"
                    } else {
                        "agent"
                    },
                    message.content
                );
            }
            if let Some(run) = snapshot.active_run {
                println!("Active Agent Run: {} ({:?})", run.id, run.status);
            }
        }
        Command::Chat { project_id } => chat(&client, &base, &project_id).await?,
    }
    Ok(())
}

async fn chat(client: &Client, base: &str, project_id: &str) -> Result<()> {
    let snapshot: ConversationSnapshot = get(
        client,
        &format!("{base}/v1/projects/{project_id}/conversation"),
    )
    .await?;
    println!(
        "Feynman · {} · Main Agent",
        snapshot.conversation.project_id
    );
    for message in &snapshot.messages {
        println!(
            "{}: {}",
            if message.role == feynman_core::MessageRole::User {
                "you"
            } else {
                "agent"
            },
            message.content
        );
    }
    println!("Type /exit to leave this conversation.");
    let mut input = String::new();
    let mut stdin = BufReader::new(tokio::io::stdin());
    if let Some(run) = snapshot.active_run {
        println!("Resuming Agent Run {}…", run.id);
        let feed: RunFeed = get(
            client,
            &format!(
                "{base}/v1/projects/{project_id}/runs/{}/events?after=0",
                run.id
            ),
        )
        .await?;
        let resolved: HashSet<&str> = feed
            .events
            .iter()
            .filter(|event| event.kind == "approval_resolved")
            .filter_map(|event| event.payload.get("requestId").and_then(Value::as_str))
            .collect();
        for event in &feed.events {
            if event.kind == "approval_requested"
                && let Some(request_id) = event.payload.get("requestId").and_then(Value::as_str)
                && !resolved.contains(request_id)
            {
                render_event(client, base, project_id, &run.id, event, &mut stdin).await?;
            }
        }
        let cursor = feed.events.last().map_or(0, |event| event.sequence);
        if !feed.run.status.is_terminal() {
            follow_run(client, base, project_id, &run.id, cursor, &mut stdin).await?;
        }
    }
    loop {
        print!("\nyou> ");
        io::stdout().flush()?;
        input.clear();
        if stdin.read_line(&mut input).await? == 0 {
            break;
        }
        let message = input.trim();
        if message.is_empty() {
            continue;
        }
        if matches!(message, "/exit" | "/quit") {
            break;
        }
        let submitted: SendMessageResponse = post(
            client,
            &format!("{base}/v1/projects/{project_id}/messages"),
            &json!({"content":message}),
        )
        .await?;
        print!("agent> ");
        io::stdout().flush()?;
        follow_run(client, base, project_id, &submitted.run.id, 0, &mut stdin).await?;
    }
    Ok(())
}

async fn follow_run(
    client: &Client,
    base: &str,
    project_id: &str,
    run_id: &str,
    mut cursor: i64,
    stdin: &mut BufReader<tokio::io::Stdin>,
) -> Result<()> {
    let mut printed_text = false;
    loop {
        let url = format!("{base}/v1/projects/{project_id}/runs/{run_id}/events?after={cursor}");
        let feed: RunFeed = get(client, &url).await?;
        for event in feed.events {
            cursor = cursor.max(event.sequence);
            if render_event(client, base, project_id, run_id, &event, stdin).await? {
                printed_text = true;
            }
        }
        if feed.run.status.is_terminal() {
            if printed_text {
                println!();
            }
            if feed.run.status != AgentRunStatus::Completed {
                eprintln!(
                    "Agent Run {:?}: {}",
                    feed.run.status,
                    feed.run.error.unwrap_or_else(|| "no error detail".into())
                );
            }
            return Ok(());
        }
        sleep(Duration::from_millis(300)).await;
    }
}

async fn render_event(
    client: &Client,
    base: &str,
    project_id: &str,
    run_id: &str,
    event: &ChatEvent,
    stdin: &mut BufReader<tokio::io::Stdin>,
) -> Result<bool> {
    match event.kind.as_str() {
        "agent_text_delta" => {
            if let Some(text) = event.payload.get("text").and_then(Value::as_str) {
                print!("{text}");
                io::stdout().flush()?;
                return Ok(true);
            }
        }
        "tool_activity" => {
            let tool = event
                .payload
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let status = event
                .payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("running");
            println!("\n[{tool}: {status}]");
        }
        "approval_requested" => {
            let request_id = event
                .payload
                .get("requestId")
                .and_then(Value::as_str)
                .context("approval event has no request id")?;
            let summary = event
                .payload
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("Codex requested approval");
            eprint!("\nApproval required: {summary}. Allow? [y/N] ");
            io::stderr().flush()?;
            let mut answer = String::new();
            stdin.read_line(&mut answer).await?;
            let decision = if answer.trim().eq_ignore_ascii_case("y") {
                "accept"
            } else {
                "decline"
            };
            let url =
                format!("{base}/v1/projects/{project_id}/runs/{run_id}/approvals/{request_id}");
            let response = client
                .post(url)
                .json(&json!({"decision":decision}))
                .send()
                .await?;
            check_response(response).await?;
        }
        "run_failed" => {
            if let Some(error) = event.payload.get("error").and_then(Value::as_str) {
                eprintln!("\nAgent error: {error}");
            }
        }
        _ => {}
    }
    Ok(false)
}

async fn get<T: DeserializeOwned>(client: &Client, url: &str) -> Result<T> {
    let response = client.get(url).send().await?;
    decode(response).await
}

async fn post<T: DeserializeOwned>(client: &Client, url: &str, body: &Value) -> Result<T> {
    let response = client.post(url).json(body).send().await?;
    decode(response).await
}

async fn put<T: DeserializeOwned>(client: &Client, url: &str) -> Result<T> {
    let response = client.put(url).send().await?;
    decode(response).await
}

async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let response = check_response(response).await?;
    response
        .json()
        .await
        .context("invalid response from Feynman server")
}

async fn check_response(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let message = response.text().await.unwrap_or_default();
    bail!("Feynman server returned {status}: {message}");
}
