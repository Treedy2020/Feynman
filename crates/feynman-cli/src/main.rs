use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use feynman_core::{
    AgentRunStatus, ChatEvent, ConversationSnapshot, DwsAuthorization, Project, ProjectToolAccess,
    RunFeed, SendMessageResponse, ToolGrant,
};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::{self, Write};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::{Duration, sleep};

mod access;
mod auth;
mod automation;
mod evaluation;
mod feed;
mod management;
mod resources;

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
    /// Machine-readable output for local harnesses. Never answers approvals implicitly.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Unified read-only external message inbox; not the Agent prompt-delivery queue.
    Feed {
        #[command(subcommand)]
        command: feed::FeedCommand,
    },
    /// Versioned application environments; requests do not authorize themselves.
    Env {
        #[command(subcommand)]
        command: resources::EnvironmentCommand,
    },
    /// Owner-only secret metadata and stdin import. No secret-get/export.
    Secrets {
        #[command(subcommand)]
        command: resources::SecretCommand,
    },
    /// Inspect/renew/revoke usage leases, distinct from Provider token expiration.
    Leases {
        #[command(subcommand)]
        command: resources::LeaseCommand,
    },
    /// Evidence-linked durable project and Agent memory.
    Memory {
        #[command(subcommand)]
        command: resources::MemoryCommand,
    },
    /// Owner-managed durable Agent identities and capability allowlists.
    Agents {
        #[command(subcommand)]
        command: automation::AgentCommand,
    },
    /// Isolated, durable fact-source registrations and paginated local history.
    Sources {
        #[command(subcommand)]
        command: automation::SourceCommand,
    },
    /// Durable input queue and explicit unknown-delivery reconciliation.
    Inbox {
        #[command(subcommand)]
        command: automation::InboxCommand,
    },
    /// Prepare an audited historical snapshot for a direction-only runtime.
    Evaluation {
        #[command(subcommand)]
        command: evaluation::EvaluationCommand,
    },
    /// Authenticate the model runtime in Feynman's dedicated Codex home, not a SaaS user login.
    Auth {
        #[command(subcommand)]
        command: auth::AuthCommand,
    },
    /// Manage explicit external accounts, project bindings and CLI access requests.
    Access {
        #[command(subcommand)]
        command: access::AccessCommand,
    },
    /// Initialize the local user home without a running server or implicit login.
    Init {
        /// Explicitly copy only auth.json from this Codex home; never its config/history.
        #[arg(long)]
        import_codex_auth: Option<std::path::PathBuf>,
    },
    /// Show local filesystem paths without creating files or contacting the server.
    Home,
    /// Inspect every Project, or one Project's Main Agent, tasks and Workspace.
    Status {
        project_id: Option<String>,
    },
    /// Create, inspect and recover a Project's persistent execution environment.
    Workspace {
        #[command(subcommand)]
        command: management::WorkspaceCommand,
    },
    /// Delegate durable work to a Project's Main Agent.
    Tasks {
        #[command(subcommand)]
        command: management::TaskCommand,
    },
    /// Manage Project wake-ups; new plans are paused by default.
    Schedules {
        #[command(subcommand)]
        command: management::ScheduleCommand,
    },
    /// Install the bundled manager Skill into a local harness.
    Skill {
        #[command(subcommand)]
        command: management::SkillCommand,
    },
    /// Send one message without entering interactive chat.
    Send {
        project_id: String,
        #[arg(long)]
        message: String,
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value_t = 120)]
        timeout_seconds: u64,
    },
    /// Inspect durable events. --wait is bounded and returns on approval requests.
    Runs {
        project_id: String,
        run_id: String,
        #[arg(long, default_value_t = 0)]
        after: i64,
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value_t = 120)]
        timeout_seconds: u64,
    },
    /// Answer a specific pending Run approval with an explicit decision.
    Approve {
        project_id: String,
        run_id: String,
        request_id: String,
        #[arg(long, value_enum)]
        decision: management::ApprovalDecision,
    },
    /// Manage DWS device-flow authorization without exposing credentials.
    DwsAuth {
        #[command(subcommand)]
        command: DwsAuthCommand,
    },
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
        /// Prepare a Project/Workspace first; create its Agent explicitly later.
        #[arg(long)]
        without_agent: bool,
    },
    Show {
        project_id: String,
    },
}

#[derive(Subcommand)]
enum ExternalToolCommand {
    List {
        project_id: String,
    },
    Grant {
        project_id: String,
        tool_id: String,
    },
    Revoke {
        project_id: String,
        tool_id: String,
    },
    Invoke {
        project_id: String,
        tool_id: String,
        #[arg(long, default_value = "{}")]
        arguments: String,
    },
}

#[derive(Subcommand)]
enum DwsAuthCommand {
    Request {
        project_id: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        wait: bool,
    },
    List {
        project_id: String,
    },
    Status {
        project_id: String,
        authorization_id: String,
        #[arg(long)]
        wait: bool,
    },
    Cancel {
        project_id: String,
        authorization_id: String,
    },
    Check {
        project_id: String,
        authorization_id: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Command::Evaluation { command } = &cli.command {
        return evaluation::handle(command);
    }
    if let Command::Auth { command } = &cli.command {
        return auth::handle(command).await;
    }
    if matches!(&cli.command, Command::Init { .. } | Command::Home) {
        let home = feynman_local::LocalHome::from_env()?;
        if let Command::Init { import_codex_auth } = &cli.command {
            if let Some(source) = import_codex_auth {
                home.import_codex_auth(source)?;
            } else {
                home.initialize()?;
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "paths": home,
                "initialized": home.codex_home.join("config.toml").is_file(),
                "auth_cache_present": home.auth_cache_present(),
                "auth_imported": matches!(&cli.command, Command::Init { import_codex_auth: Some(_) }),
                "note": "Cache presence is not proof of valid login. CODEX_HOME is owned by Feynman; personal config and session history are not imported."
            }))?
        );
        return Ok(());
    }
    let client = Client::new();
    let base = cli.server.trim_end_matches('/').to_owned();
    if let Command::Access { command } = &cli.command {
        return access::handle(&base, command).await;
    }
    if let Command::Skill { command } = &cli.command {
        return management::install_skill(command);
    }
    if feed::handle(&base, &cli.command, cli.json).await? {
        return Ok(());
    }
    if management::handle(&client, &base, &cli.command).await? {
        return Ok(());
    }
    if automation::handle(&client, &base, &cli.command).await? {
        return Ok(());
    }
    if resources::handle(&client, &base, &cli.command).await? {
        return Ok(());
    }
    if cli.json {
        return management::json_command(&client, &base, &cli.command).await;
    }
    match cli.command {
        Command::DwsAuth { command } => dws_auth(&client, &base, command).await?,
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
            ProjectCommand::Create {
                name,
                without_agent,
            } => {
                let response: Value = post(
                    &client,
                    &format!("{base}/v1/projects"),
                    &json!({"name":name,"without_agent":without_agent}),
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
                let response: Value = put(
                    &client,
                    &format!("{base}/v1/projects/{project_id}/external-tools/{tool_id}"),
                )
                .await?;
                let grant: ToolGrant = serde_json::from_value(
                    response
                        .get("grant")
                        .context("server returned no grant")?
                        .clone(),
                )?;
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
            ExternalToolCommand::Invoke {
                project_id,
                tool_id,
                arguments,
            } => {
                let arguments: Value =
                    serde_json::from_str(&arguments).context("arguments must be JSON")?;
                let result: Value = post(
                    &client,
                    &format!("{base}/v1/projects/{project_id}/external-tools/{tool_id}/invoke"),
                    &json!({"arguments": arguments}),
                )
                .await?;
                println!("{}", serde_json::to_string_pretty(&result)?);
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
        _ => unreachable!("management command already handled"),
    }
    Ok(())
}

async fn dws_auth(client: &Client, base: &str, command: DwsAuthCommand) -> Result<()> {
    let (project_id, authorization_id, wait) = match command {
        DwsAuthCommand::Request {
            project_id,
            reason,
            wait,
        } => {
            let response: Value = post(
                client,
                &format!("{base}/v1/projects/{project_id}/dws-authorizations"),
                &json!({"reason":reason}),
            )
            .await?;
            let record: DwsAuthorization =
                serde_json::from_value(response["authorization"].clone())?;
            show_authorization(&record);
            (project_id, record.id, wait)
        }
        DwsAuthCommand::List { project_id } => {
            let response: Value = get(
                client,
                &format!("{base}/v1/projects/{project_id}/dws-authorizations"),
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
            return Ok(());
        }
        DwsAuthCommand::Status {
            project_id,
            authorization_id,
            wait,
        } => (project_id, authorization_id, wait),
        DwsAuthCommand::Cancel {
            project_id,
            authorization_id,
        } => {
            let response = check_response(
                client
                    .delete(format!(
                        "{base}/v1/projects/{project_id}/dws-authorizations/{authorization_id}"
                    ))
                    .send()
                    .await?,
            )
            .await?;
            let value: Value = response.json().await?;
            show_authorization(&serde_json::from_value(value["authorization"].clone())?);
            return Ok(());
        }
        DwsAuthCommand::Check {
            project_id,
            authorization_id,
        } => {
            let value: Value = post(
                client,
                &format!(
                    "{base}/v1/projects/{project_id}/dws-authorizations/{authorization_id}/check"
                ),
                &json!({}),
            )
            .await?;
            show_authorization(&serde_json::from_value(value["authorization"].clone())?);
            return Ok(());
        }
    };
    let mut last = String::new();
    loop {
        let response: Value = get(
            client,
            &format!("{base}/v1/projects/{project_id}/dws-authorizations/{authorization_id}"),
        )
        .await?;
        let record: DwsAuthorization = serde_json::from_value(response["authorization"].clone())?;
        let current = serde_json::to_string(&record)?;
        if current != last {
            show_authorization(&record);
            last = current;
        }
        if !wait || !record.status.is_pending() {
            if wait && record.status != feynman_core::DwsAuthorizationStatus::Authorized {
                bail!("authorization ended: {:?}", record.status);
            }
            return Ok(());
        }
        sleep(Duration::from_secs(1)).await;
    }
}

fn show_authorization(record: &DwsAuthorization) {
    println!("DWS authorization {} · {:?}", record.id, record.status);
    if let Some(uri) = record
        .verification_uri_complete
        .as_ref()
        .or(record.verification_uri.as_ref())
    {
        println!("Open to authorize: {uri}");
        if let Some(code) = &record.user_code {
            println!("User code: {code}");
        }
        println!("Expires: {}", record.expires_at.to_rfc3339());
    }
    if let Some(identity) = &record.identity {
        println!("Account: {}:{}", identity.corp_id, identity.user_id);
    }
    if let Some(code) = &record.error_code {
        println!("State detail: {code}");
    }
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
    // Authorization progresses independently of a model turn and remains visible while input is idle.
    let _authorization_watcher =
        watch_authorizations(client.clone(), base.into(), project_id.into());
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

struct AuthorizationWatcher(tokio::task::JoinHandle<()>);
impl Drop for AuthorizationWatcher {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn watch_authorizations(client: Client, base: String, project_id: String) -> AuthorizationWatcher {
    AuthorizationWatcher(tokio::spawn(async move {
        let mut seen = std::collections::HashMap::<String, String>::new();
        loop {
            if let Ok(value) = get::<Value>(
                &client,
                &format!("{base}/v1/projects/{project_id}/dws-authorizations"),
            )
            .await
                && let Some(items) = value["authorizations"].as_array()
            {
                for item in items {
                    if let Ok(record) = serde_json::from_value::<DwsAuthorization>(item.clone())
                        && (record.status.is_pending() || seen.contains_key(&record.id))
                    {
                        let fingerprint = format!("{:?}|{:?}", record.status, record.user_code);
                        if seen.get(&record.id) != Some(&fingerprint) {
                            println!();
                            show_authorization(&record);
                            seen.insert(record.id, fingerprint);
                        }
                    }
                }
            }
            sleep(Duration::from_secs(1)).await;
        }
    }))
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
