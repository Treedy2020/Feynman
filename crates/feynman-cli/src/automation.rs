use super::*;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum AgentCommand {
    List {
        project_id: String,
    },
    Show {
        project_id: String,
        agent_id: String,
    },
    Create {
        project_id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        default: bool,
    },
    /// Replace name, lifecycle, default route and capability allowlist from an explicit JSON file.
    Update {
        project_id: String,
        agent_id: String,
        #[arg(long)]
        input: PathBuf,
    },
    /// Archive (not erase) after all active/unknown work is resolved.
    Delete {
        project_id: String,
        agent_id: String,
    },
    Disable {
        project_id: String,
        agent_id: String,
    },
    Enable {
        project_id: String,
        agent_id: String,
    },
    /// Set an explicit per-Agent capability; the Project Grant is still required separately.
    Grant {
        project_id: String,
        agent_id: String,
        tool_id: String,
    },
    Revoke {
        project_id: String,
        agent_id: String,
        tool_id: String,
    },
}
#[derive(Subcommand)]
pub enum SourceCommand {
    /// Read live local channel updates. Closing this view does not stop collection.
    Watch {
        project_id: String,
        source_id: String,
        /// Resume after a persisted sequence. Omit to start at the current head.
        #[arg(long)]
        after: Option<i64>,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..=60))]
        poll_seconds: u64,
    },
    List {
        project_id: String,
    },
    /// New registration always creates a new history/cursor partition. Input contains no secrets.
    Create {
        project_id: String,
        #[arg(long)]
        input: PathBuf,
    },
    Pause {
        project_id: String,
        source_id: String,
    },
    Resume {
        project_id: String,
        source_id: String,
    },
    Read {
        project_id: String,
        source_id: String,
        #[arg(long, default_value_t = 0)]
        after: i64,
        #[arg(long, default_value_t = 100)]
        limit: i64,
    },
    /// Inspect bounded capture ranges, gaps and listener stops; not a completeness guarantee.
    Captures {
        project_id: String,
        source_id: String,
        #[arg(long, default_value_t = 0)]
        after: i64,
        #[arg(long, default_value_t = 100)]
        limit: i64,
    },
    /// Import trusted-owner evidence into a manual source. JSON: {"records":[...]}.
    Ingest {
        project_id: String,
        source_id: String,
        #[arg(long)]
        input: PathBuf,
    },
    Backfill {
        project_id: String,
        source_id: String,
        #[arg(long)]
        start: String,
        #[arg(long)]
        end: String,
    },
}
#[derive(Subcommand)]
pub enum InboxCommand {
    List {
        project_id: String,
    },
    Send {
        project_id: String,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        message: String,
        #[arg(long, default_value = "queue")]
        delivery: String,
    },
    /// Resolve only after inspecting native history; `pending` explicitly authorizes a retry.
    Resolve {
        project_id: String,
        message_id: String,
        #[arg(long)]
        state: String,
        #[arg(long)]
        run_id: Option<String>,
    },
}
fn read_input(path: &PathBuf) -> Result<Value> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() || meta.len() > 1_048_576 {
        bail!("input must be a regular JSON file <= 1 MiB")
    }
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
pub async fn handle(client: &Client, base: &str, command: &Command) -> Result<bool> {
    let value:Value=match command {
        Command::Agents{command}=>match command {
            AgentCommand::List{project_id}=>get(client,&format!("{base}/v1/projects/{project_id}/agents")).await?,
            AgentCommand::Show{project_id,agent_id}=>{
                let list:Value=get(client,&format!("{base}/v1/projects/{project_id}/agents")).await?;
                json!({"agent":list["agents"].as_array().context("agents response invalid")?.iter().find(|a|a["id"].as_str()==Some(agent_id)).context("Agent not found")?})
            }
            AgentCommand::Create{project_id,name,default}=>post(client,&format!("{base}/v1/projects/{project_id}/agents"),&json!({"name":name,"state":"active","is_default":default,"capabilities":[]})).await?,
            AgentCommand::Update{project_id,agent_id,input}=>check_response(client.put(format!("{base}/v1/projects/{project_id}/agents/{agent_id}")).json(&read_input(input)?).send().await?).await?.json().await?,
            AgentCommand::Disable{project_id,agent_id}|AgentCommand::Enable{project_id,agent_id}|AgentCommand::Grant{project_id,agent_id,..}|AgentCommand::Revoke{project_id,agent_id,..}=>{
                let list:Value=get(client,&format!("{base}/v1/projects/{project_id}/agents")).await?;
                let a=list["agents"].as_array().context("agents response invalid")?.iter().find(|a|a["id"].as_str()==Some(agent_id)).context("Agent not found")?;
                let mut caps=if let Some(caps)=a["capabilities"].as_array() {caps.clone()} else {
                    // Freeze legacy project-inherited grants to an explicit snapshot on first owner edit.
                    let tools:Value=get(client,&format!("{base}/v1/projects/{project_id}/external-tools")).await?;
                    tools["tools"].as_array().context("tools response invalid")?.iter().filter(|t|t["granted"]==true).map(|t|t["id"].clone()).collect()
                };
                let mut state=a["state"].clone();
                match command {
                    AgentCommand::Disable{..}=>state=json!("disabled"),
                    AgentCommand::Enable{..}=>state=json!("active"),
                    AgentCommand::Grant{tool_id,..}=>if !caps.contains(&json!(tool_id)) {caps.push(json!(tool_id))},
                    AgentCommand::Revoke{tool_id,..}=>caps.retain(|cap|cap.as_str()!=Some(tool_id)),
                    _=>unreachable!()
                }
                check_response(client.put(format!("{base}/v1/projects/{project_id}/agents/{agent_id}")).json(&json!({"name":a["name"],"state":state,"is_default":a["is_default"],"capabilities":caps})).send().await?).await?.json().await?
            }
            AgentCommand::Delete{project_id,agent_id}=>{
                let list:Value=get(client,&format!("{base}/v1/projects/{project_id}/agents")).await?;
                let a=list["agents"].as_array().context("agents response invalid")?.iter().find(|a|a["id"].as_str()==Some(agent_id)).context("Agent not found")?;
                check_response(client.put(format!("{base}/v1/projects/{project_id}/agents/{agent_id}")).json(&json!({"name":a["name"],"state":"archived","is_default":false,"capabilities":[]})).send().await?).await?.json().await?
            }
        },
        Command::Sources{command}=>match command {
            SourceCommand::Watch{..}=>unreachable!("channel watch already handled"),
            SourceCommand::List{project_id}=>get(client,&format!("{base}/v1/projects/{project_id}/sources")).await?,
            SourceCommand::Create{project_id,input}=>post(client,&format!("{base}/v1/projects/{project_id}/sources"),&read_input(input)?).await?,
            SourceCommand::Pause{project_id,source_id}|SourceCommand::Resume{project_id,source_id}=>check_response(client.put(format!("{base}/v1/projects/{project_id}/sources/{source_id}")).json(&json!({"enabled":matches!(command,SourceCommand::Resume{..})})).send().await?).await?.json().await?,
            SourceCommand::Read{project_id,source_id,after,limit}=>get(client,&format!("{base}/v1/projects/{project_id}/sources/{source_id}/records?after={after}&limit={limit}")).await?,
            SourceCommand::Captures{project_id,source_id,after,limit}=>get(client,&format!("{base}/v1/projects/{project_id}/sources/{source_id}/captures?after={after}&limit={limit}")).await?,
            SourceCommand::Ingest{project_id,source_id,input}=>post(client,&format!("{base}/v1/projects/{project_id}/sources/{source_id}/records"),&read_input(input)?).await?,
            SourceCommand::Backfill{project_id,source_id,start,end}=>post(client,&format!("{base}/v1/projects/{project_id}/sources/{source_id}/backfill"),&json!({"start":start,"end":end})).await?,
        },
        Command::Inbox{command}=>match command {
            InboxCommand::List{project_id}=>get(client,&format!("{base}/v1/projects/{project_id}/inbox")).await?,
            InboxCommand::Send{project_id,agent,message,delivery}=>post(client,&format!("{base}/v1/projects/{project_id}/inbox"),&json!({"agent_id":agent,"content":message,"delivery":delivery})).await?,
            InboxCommand::Resolve{project_id,message_id,state,run_id}=>check_response(client.put(format!("{base}/v1/projects/{project_id}/inbox/{message_id}")).json(&json!({"state":state,"run_id":run_id})).send().await?).await?.json().await?,
        },
        _=>return Ok(false)
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(true)
}
