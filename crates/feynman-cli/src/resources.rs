use super::*;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum EnvironmentCommand {
    Profiles {
        project_id: String,
    },
    Put {
        project_id: String,
        #[arg(long)]
        input: PathBuf,
    },
    Request {
        project_id: String,
        #[arg(long)]
        profile: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        seconds: Option<i64>,
    },
    Status {
        project_id: String,
    },
    Approve {
        project_id: String,
        request_id: String,
        #[arg(long)]
        revision: i64,
        #[arg(long)]
        accept_secret_exposure: bool,
        #[arg(long)]
        auto_renew: bool,
        #[arg(long, default_value_t = 28800)]
        seconds: i64,
    },
    Deny {
        project_id: String,
        request_id: String,
        #[arg(long)]
        revision: i64,
    },
    Run {
        project_id: String,
        request_id: String,
        /// Reuse this UUID after network errors; never generate a new key to retry an uncertain launch.
        #[arg(long)]
        idempotency_key: String,
    },
    Logs {
        project_id: String,
        workload_id: String,
    },
}
#[derive(Subcommand)]
pub enum SecretCommand {
    List {
        project_id: String,
    },
    /// Read value from private stdin only; never accept a secret argument or print its value.
    Put {
        project_id: String,
        #[arg(long)]
        name: String,
        #[arg(long, required = true)]
        stdin: bool,
        #[arg(long)]
        id: Option<String>,
        #[arg(long, default_value_t = 0)]
        revision: i64,
    },
}
#[derive(Subcommand)]
pub enum LeaseCommand {
    List {
        project_id: String,
    },
    Revoke {
        project_id: String,
        lease_id: String,
        #[arg(long)]
        revision: i64,
    },
    Renew {
        project_id: String,
        lease_id: String,
        #[arg(long)]
        revision: i64,
    },
}
#[derive(Subcommand)]
pub enum MemoryCommand {
    Search {
        project_id: String,
        #[arg(long, default_value = "")]
        query: String,
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Get {
        project_id: String,
        memory_id: String,
        #[arg(long)]
        revision: Option<i64>,
    },
    History {
        project_id: String,
        memory_id: String,
    },
    Put {
        project_id: String,
        #[arg(long)]
        input: PathBuf,
    },
    Confirm {
        project_id: String,
        memory_id: String,
        #[arg(long)]
        revision: i64,
        #[arg(long)]
        pin: bool,
    },
    Delete {
        project_id: String,
        memory_id: String,
        #[arg(long)]
        revision: i64,
    },
}
fn input(path: &PathBuf) -> Result<Value> {
    let m = std::fs::symlink_metadata(path)?;
    if !m.is_file() || m.len() > 131072 {
        bail!("input must be a regular JSON file <=128 KiB")
    }
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
pub async fn handle(_client: &Client, base: &str, command: &Command) -> Result<bool> {
    if !matches!(
        command,
        Command::Env { .. }
            | Command::Secrets { .. }
            | Command::Leases { .. }
            | Command::Memory { .. }
    ) {
        return Ok(false);
    }
    let url = reqwest::Url::parse(base)?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        bail!("resource management requires the trusted loopback HTTP server")
    }
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(60))
        .build()?;
    let (project, operation, args) = match command {
        Command::Env { command } => match command {
            EnvironmentCommand::Profiles { project_id } => (project_id, "profile.list", json!({})),
            EnvironmentCommand::Put {
                project_id,
                input: path,
            } => (project_id, "profile.put", input(path)?),
            EnvironmentCommand::Request {
                project_id,
                profile,
                reason,
                agent,
                seconds,
            } => (
                project_id,
                "env.request",
                json!({"profile":profile,"reason":reason,"agent_id":agent,"seconds":seconds}),
            ),
            EnvironmentCommand::Status { project_id } => (project_id, "env.status", json!({})),
            EnvironmentCommand::Approve {
                project_id,
                request_id,
                revision,
                accept_secret_exposure,
                auto_renew,
                seconds,
            } => (
                project_id,
                "env.approve",
                json!({"id":request_id,"expected_revision":revision,"accept_secret_exposure":accept_secret_exposure,"auto_renew":auto_renew,"seconds":seconds}),
            ),
            EnvironmentCommand::Deny {
                project_id,
                request_id,
                revision,
            } => (
                project_id,
                "env.deny",
                json!({"id":request_id,"expected_revision":revision}),
            ),
            EnvironmentCommand::Run {
                project_id,
                request_id,
                idempotency_key,
            } => (
                project_id,
                "env.run",
                json!({"request_id":request_id,"idempotency_key":idempotency_key}),
            ),
            EnvironmentCommand::Logs {
                project_id,
                workload_id,
            } => (project_id, "workload.logs", json!({"id":workload_id})),
        },
        Command::Secrets { command } => match command {
            SecretCommand::List { project_id } => (project_id, "secret.list", json!({})),
            SecretCommand::Put {
                project_id,
                name,
                id,
                revision,
                ..
            } => {
                use std::io::{IsTerminal, Read};
                if std::io::stdin().is_terminal() {
                    bail!(
                        "secret stdin must be a private pipe or redirected file; interactive echoing input is refused"
                    )
                }
                let mut value = String::new();
                std::io::stdin().take(16385).read_to_string(&mut value)?;
                if value.len() > 16384 {
                    bail!("secret exceeds 16 KiB")
                }
                (
                    project_id,
                    "secret.put",
                    json!({"name":name,"id":id,"expected_revision":revision,"value":value}),
                )
            }
        },
        Command::Leases { command } => match command {
            LeaseCommand::List { project_id } => (project_id, "lease.list", json!({})),
            LeaseCommand::Revoke {
                project_id,
                lease_id,
                revision,
            } => (
                project_id,
                "lease.revoke",
                json!({"id":lease_id,"expected_revision":revision}),
            ),
            LeaseCommand::Renew {
                project_id,
                lease_id,
                revision,
            } => (
                project_id,
                "lease.renew",
                json!({"id":lease_id,"expected_revision":revision}),
            ),
        },
        Command::Memory { command } => match command {
            MemoryCommand::Search {
                project_id,
                query,
                after,
                limit,
            } => (
                project_id,
                "memory.search",
                json!({"query":query,"after":after,"limit":limit}),
            ),
            MemoryCommand::Get {
                project_id,
                memory_id,
                revision,
            } => (
                project_id,
                "memory.get",
                json!({"id":memory_id,"revision":revision}),
            ),
            MemoryCommand::History {
                project_id,
                memory_id,
            } => (project_id, "memory.history", json!({"id":memory_id})),
            MemoryCommand::Put {
                project_id,
                input: path,
            } => (project_id, "memory.put", input(path)?),
            MemoryCommand::Confirm {
                project_id,
                memory_id,
                revision,
                pin,
            } => (
                project_id,
                "memory.confirm",
                json!({"id":memory_id,"expected_revision":revision,"confirmed":true,"pinned":pin}),
            ),
            MemoryCommand::Delete {
                project_id,
                memory_id,
                revision,
            } => (
                project_id,
                "memory.delete",
                json!({"id":memory_id,"expected_revision":revision}),
            ),
        },
        _ => return Ok(false),
    };
    let value: Value = post(
        &client,
        &format!("{base}/v1/projects/{project}/resources"),
        &json!({"operation":operation,"arguments":args}),
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(true)
}
