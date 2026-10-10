use super::*;
use clap::ValueEnum;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum WorkspaceCommand {
    /// Initialize a new, unused Workspace with a prepared historical snapshot.
    Seed {
        project_id: String,
        #[arg(long)]
        input: PathBuf,
    },
    Show {
        project_id: String,
    },
    Create {
        project_id: String,
        #[arg(long, value_enum)]
        provider: Provider,
    },
    Refresh {
        project_id: String,
    },
    /// Pause the container; retained disk and daemon memory still consume host resources.
    Stop {
        project_id: String,
    },
    Resume {
        project_id: String,
    },
    /// Replace a missing/terminated container, preserving its named volume.
    Restore {
        project_id: String,
    },
    Renew {
        project_id: String,
    },
}
#[derive(Clone, Copy, ValueEnum)]
pub enum Provider {
    Local,
    Opensandbox,
}

#[derive(Subcommand)]
pub enum TaskCommand {
    List {
        project_id: String,
    },
    Create {
        project_id: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        prompt: String,
    },
    Show {
        project_id: String,
        task_id: String,
    },
    Cancel {
        project_id: String,
        task_id: String,
    },
}
#[derive(Subcommand)]
pub enum ScheduleCommand {
    List {
        project_id: String,
    },
    /// Paused by default. --enable authorizes future Agent turns while the server runs.
    Create {
        project_id: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        prompt: String,
        #[arg(long, required_unless_present_any = ["at", "source"])]
        every_seconds: Option<i64>,
        /// RFC3339 timestamp with timezone, e.g. 2026-09-30T09:00:00+08:00.
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        enable: bool,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long = "source")]
        source: Vec<String>,
        #[arg(long)]
        collect_for_seconds: Option<i64>,
        #[arg(long, default_value = "queue")]
        delivery: String,
    },
    Pause {
        project_id: String,
        schedule_id: String,
    },
    Resume {
        project_id: String,
        schedule_id: String,
    },
    Deliveries {
        project_id: String,
        schedule_id: String,
    },
}
#[derive(Subcommand)]
pub enum SkillCommand {
    /// Install the bundled Skill into an explicitly selected harness skills directory.
    Install {
        #[arg(long)]
        skills_dir: PathBuf,
    },
}
#[derive(Clone, Copy, ValueEnum)]
pub enum ApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}
impl ApprovalDecision {
    fn wire(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::AcceptForSession => "acceptForSession",
            Self::Decline => "decline",
            Self::Cancel => "cancel",
        }
    }
}

fn output(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub async fn handle(client: &Client, base: &str, command: &Command) -> Result<bool> {
    let response: Value = match command {
        Command::Status {project_id} => get(client, &match project_id {Some(id) => format!("{base}/v1/projects/{id}/status"), None => format!("{base}/v1/status")}).await?,
        Command::Workspace {command} => {
            let (project, action) = match command {
                WorkspaceCommand::Seed {project_id,input} => {
                    let metadata = std::fs::symlink_metadata(input)?;
                    if !metadata.file_type().is_file() || metadata.len() > 48*1024*1024 { bail!("input must be a bounded regular snapshot file"); }
                    let seed: Value = serde_json::from_slice(&std::fs::read(input)?)?;
                    output(&post::<Value>(client, &format!("{base}/v1/projects/{project_id}/workspace/seed"), &seed).await?)?;
                    return Ok(true);
                },
                WorkspaceCommand::Show {project_id} => (project_id, None),
                WorkspaceCommand::Create {project_id, provider} => {
                    let provider = match provider {Provider::Local => "local", Provider::Opensandbox => "opensandbox"};
                    output(&post::<Value>(client, &format!("{base}/v1/projects/{project_id}/workspace"), &json!({"provider":provider})).await?)?;
                    return Ok(true);
                },
                WorkspaceCommand::Refresh {project_id} => (project_id, Some("refresh")),
                WorkspaceCommand::Stop {project_id} => (project_id, Some("stop")),
                WorkspaceCommand::Resume {project_id} => (project_id, Some("resume")),
                WorkspaceCommand::Restore {project_id} => (project_id, Some("restore")),
                WorkspaceCommand::Renew {project_id} => (project_id, Some("renew")),
            };
            let url = format!("{base}/v1/projects/{project}/workspace");
            if let Some(action) = action {post(client, &format!("{url}/actions"), &json!({"action":action})).await?} else {get(client, &url).await?}
        },
        Command::Tasks {command} => {
            match command {
                TaskCommand::List {project_id} => get(client, &format!("{base}/v1/projects/{project_id}/tasks")).await?,
                TaskCommand::Create {project_id, title, prompt} => post(client, &format!("{base}/v1/projects/{project_id}/tasks"), &json!({"title":title,"prompt":prompt})).await?,
                TaskCommand::Show {project_id, task_id} => get(client, &format!("{base}/v1/projects/{project_id}/tasks/{task_id}")).await?,
                TaskCommand::Cancel {project_id, task_id} => delete_json(client, &format!("{base}/v1/projects/{project_id}/tasks/{task_id}")).await?,
            }
        },
        Command::Schedules {command} => match command {
            ScheduleCommand::List {project_id} => get(client, &format!("{base}/v1/projects/{project_id}/schedules")).await?,
            ScheduleCommand::Create {project_id, title, prompt, every_seconds, at, enable,agent,source,collect_for_seconds,delivery} => post(client, &format!("{base}/v1/projects/{project_id}/schedules"), &json!({"title":title,"prompt":prompt,"interval_seconds":every_seconds,"at":at,"enabled":enable,"agent_id":agent,"source_ids":source,"collect_for_seconds":collect_for_seconds,"delivery":delivery})).await?,
            ScheduleCommand::Deliveries{project_id,schedule_id}=>get(client,&format!("{base}/v1/projects/{project_id}/schedules/{schedule_id}/deliveries")).await?,
            ScheduleCommand::Pause {project_id, schedule_id} | ScheduleCommand::Resume {project_id, schedule_id} => {
                let enabled = matches!(command, ScheduleCommand::Resume {..});
                check_response(client.put(format!("{base}/v1/projects/{project_id}/schedules/{schedule_id}")).json(&json!({"enabled":enabled})).send().await?).await?.json().await?
            },
        },
        Command::Send {project_id, message, wait, timeout_seconds} => {
            let submitted: SendMessageResponse = post(client, &format!("{base}/v1/projects/{project_id}/messages"), &json!({"content":message})).await?;
            if *wait {read_run(client, base, project_id, &submitted.run.id, 0, true, *timeout_seconds).await?} else {serde_json::to_value(submitted)?}
        },
        Command::Runs {project_id, run_id, after, wait, timeout_seconds} => read_run(client, base, project_id, run_id, *after, *wait, *timeout_seconds).await?,
        Command::Approve {project_id, run_id, request_id, decision} => {
            check_response(client.post(format!("{base}/v1/projects/{project_id}/runs/{run_id}/approvals/{request_id}")).json(&json!({"decision":decision.wire()})).send().await?).await?;
            json!({"project_id":project_id,"run_id":run_id,"request_id":request_id,"decision":decision.wire()})
        },
        _ => return Ok(false),
    };
    output(&response)?;
    Ok(true)
}

async fn read_run(
    client: &Client,
    base: &str,
    project: &str,
    run: &str,
    after: i64,
    wait: bool,
    timeout_seconds: u64,
) -> Result<Value> {
    let until = tokio::time::Instant::now() + Duration::from_secs(timeout_seconds.min(3600));
    loop {
        let feed: RunFeed = get(
            client,
            &format!("{base}/v1/projects/{project}/runs/{run}/events?after={after}"),
        )
        .await?;
        let ready = !wait
            || feed.run.status.is_terminal()
            || feed.run.status == AgentRunStatus::WaitingApproval;
        let timed_out = !ready && tokio::time::Instant::now() >= until;
        let mut value = serde_json::to_value(&feed)?;
        value["wait_timed_out"] = json!(timed_out);
        if ready || timed_out {
            if matches!(
                feed.run.status,
                AgentRunStatus::Failed | AgentRunStatus::Interrupted
            ) {
                output(&value)?;
                bail!(
                    "Agent Run ended {:?}; inspect its persisted error before retrying",
                    feed.run.status
                );
            }
            return Ok(value);
        }
        sleep(Duration::from_millis(300)).await;
    }
}

async fn delete_json(client: &Client, url: &str) -> Result<Value> {
    Ok(check_response(client.delete(url).send().await?)
        .await?
        .json()
        .await?)
}

pub async fn json_command(client: &Client, base: &str, command: &Command) -> Result<()> {
    let value: Value = match command {
        Command::Health => get(client, &format!("{base}/v1/health")).await?,
        Command::Projects {command} => match command {
            ProjectCommand::List => get(client, &format!("{base}/v1/projects")).await?,
            ProjectCommand::Create {name,without_agent} => post(client, &format!("{base}/v1/projects"), &json!({"name":name,"without_agent":without_agent})).await?,
            ProjectCommand::Show {project_id} => get(client, &format!("{base}/v1/projects/{project_id}")).await?,
        },
        Command::Conversation {project_id} => get(client, &format!("{base}/v1/projects/{project_id}/conversation")).await?,
        Command::ExternalTools {command} => match command {
            ExternalToolCommand::List {project_id} => get(client, &format!("{base}/v1/projects/{project_id}/external-tools")).await?,
            ExternalToolCommand::Grant {project_id, tool_id} => put(client, &format!("{base}/v1/projects/{project_id}/external-tools/{tool_id}")).await?,
            ExternalToolCommand::Revoke {project_id, tool_id} => {
                check_response(client.delete(format!("{base}/v1/projects/{project_id}/external-tools/{tool_id}")).send().await?).await?;
                json!({"revoked":true,"project_id":project_id,"tool_id":tool_id})
            },
            ExternalToolCommand::Invoke {project_id, tool_id, arguments} => post(client, &format!("{base}/v1/projects/{project_id}/external-tools/{tool_id}/invoke"), &json!({"arguments":serde_json::from_str::<Value>(arguments).context("arguments must be JSON")?})).await?,
        },
        Command::DwsAuth {command} => {
            if matches!(command, DwsAuthCommand::Request {wait: true, ..} | DwsAuthCommand::Status {wait: true, ..}) {
                bail!("use nonblocking --json request/status to present the device link, then poll status; --wait is interactive-only");
            }
            let (project, record, wait) = match command {
                DwsAuthCommand::Request {project_id, reason, wait} => {
                    let value: Value = post(client, &format!("{base}/v1/projects/{project_id}/dws-authorizations"), &json!({"reason":reason})).await?;
                    (project_id, value, *wait)
                },
                DwsAuthCommand::List {project_id} => {return output(&get(client, &format!("{base}/v1/projects/{project_id}/dws-authorizations")).await?);},
                DwsAuthCommand::Status {project_id, authorization_id, wait} => (project_id, get(client, &format!("{base}/v1/projects/{project_id}/dws-authorizations/{authorization_id}")).await?, *wait),
                DwsAuthCommand::Cancel {project_id, authorization_id} => (project_id, delete_json(client, &format!("{base}/v1/projects/{project_id}/dws-authorizations/{authorization_id}")).await?, false),
                DwsAuthCommand::Check {project_id, authorization_id} => (project_id, post(client, &format!("{base}/v1/projects/{project_id}/dws-authorizations/{authorization_id}/check"), &json!({})).await?, false),
            };
            if wait { bail!("use nonblocking --json request/status to present the device link, then poll status; --wait is interactive-only"); }
            let _ = project;
            record
        },
        Command::Chat {..} => bail!("chat is interactive; use send/runs/conversation with --json"),
        _ => unreachable!(),
    };
    output(&value)
}

pub fn install_skill(command: &SkillCommand) -> Result<()> {
    let SkillCommand::Install { skills_dir } = command;
    let root = skills_dir.join("feynman");
    const SKILL: &str = include_str!("../../../skills/feynman/SKILL.md");
    const METADATA: &str = include_str!("../../../skills/feynman/agents/openai.yaml");
    if root.exists() {
        if std::fs::read_to_string(root.join("SKILL.md"))
            .ok()
            .as_deref()
            != Some(SKILL)
            || std::fs::read_to_string(root.join("agents/openai.yaml"))
                .ok()
                .as_deref()
                != Some(METADATA)
        {
            bail!(
                "{} already exists and differs; preserve it and choose another skills directory",
                root.display()
            );
        }
    } else {
        std::fs::create_dir_all(skills_dir)?;
        std::fs::create_dir(&root)?;
        std::fs::create_dir(root.join("agents"))?;
        std::fs::write(root.join("SKILL.md"), SKILL)?;
        std::fs::write(root.join("agents/openai.yaml"), METADATA)?;
    }
    output(
        &json!({"installed":true,"skill":"feynman","path":std::fs::canonicalize(root)?.to_string_lossy(),"reload_harness":true}),
    )
}
