use super::*;
use clap::ValueEnum;

#[derive(Clone, Copy, ValueEnum)]
pub enum Provider {
    Dws,
    Gitlab,
    Git,
    Slate,
    Drlee,
}
impl Provider {
    fn wire(self) -> &'static str {
        match self {
            Self::Dws => "dws",
            Self::Gitlab => "gitlab",
            Self::Git => "git",
            Self::Slate => "slate",
            Self::Drlee => "drlee",
        }
    }
}
#[derive(Subcommand)]
pub enum AccessCommand {
    Providers,
    /// Owner-defined exact DWS sources and time window. Does not grant a tool.
    DwsScope {
        project_id: String,
        #[arg(long)]
        scope_json: String,
    },
    Connections {
        #[command(subcommand)]
        command: ConnectionCommand,
    },
    Status {
        project_id: String,
    },
    Request {
        project_id: String,
        #[arg(long, value_enum)]
        provider: Provider,
        #[arg(long)]
        capability: String,
        #[arg(long)]
        reason: String,
    },
    /// Bind an account to one project. Does not grant a business operation.
    Bind {
        project_id: String,
        #[arg(long, value_enum)]
        provider: Provider,
        #[arg(long)]
        connection: String,
        #[arg(long)]
        repository: Option<String>,
        #[arg(long)]
        resource_id: Option<String>,
        #[arg(long, default_value_t = 86400)]
        ttl_seconds: i64,
    },
    Unbind {
        project_id: String,
        #[arg(long, value_enum)]
        provider: Provider,
    },
}
#[derive(Subcommand)]
pub enum ConnectionCommand {
    List,
    /// Explicitly copy the selected native glab login, then verify it in the Broker.
    ImportGitlab {
        #[arg(long)]
        hostname: String,
    },
    /// Authorize Slate in the browser using its native PKCE loopback flow.
    LoginSlate {
        #[arg(long)]
        hostname: String,
    },
    /// Explicitly reuse one Slate Keychain profile; the Broker verifies this host.
    ImportSlate {
        #[arg(long)]
        hostname: String,
        #[arg(long, default_value = "prod")]
        profile: String,
    },
    /// Copy exactly one native Drlee account including refresh state (never mount its home).
    ImportDrlee {
        #[arg(long)]
        hostname: String,
        #[arg(long)]
        source_home: Option<std::path::PathBuf>,
        #[arg(long)]
        handle: Option<String>,
    },
    /// Authorize a fresh private Drlee account through its native browser flow.
    LoginDrlee {
        #[arg(long)]
        hostname: String,
    },
    /// Import an already-issued Slate session, not an upstream StarX token.
    RegisterSlate {
        #[arg(long)]
        hostname: String,
        #[arg(long, required = true)]
        token_stdin: bool,
    },
    /// Read a GitLab token only from stdin; never from argv, chat or environment.
    RegisterGitlab {
        #[arg(long)]
        hostname: String,
        #[arg(long, required = true)]
        token_stdin: bool,
    },
    /// Promote a freshly verified DWS authorization into an explicit Connection.
    RegisterDws {
        project_id: String,
        authorization_id: String,
    },
    /// Disable new calls locally, not a claim of Provider token revocation.
    Disable {
        connection_id: String,
    },
}

pub async fn handle(base: &str, command: &AccessCommand) -> Result<()> {
    let url = reqwest::Url::parse(base)?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("External Access currently requires the trusted loopback HTTP server");
    }
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        // A business read may include two bounded CLI processes: verify, then read.
        .timeout(std::time::Duration::from_secs(90))
        .build()?;
    let value:Value=match command {
        AccessCommand::DwsScope {project_id,scope_json} => {
            let scope: Value = serde_json::from_str(scope_json).context("scope must be JSON")?;
            check_response(client.put(format!("{base}/v1/projects/{project_id}/access/dws-scope")).json(&scope).send().await?).await?.json().await?
        },
        AccessCommand::Providers=>get(&client,&format!("{base}/v1/access/providers")).await?,
        AccessCommand::Connections {command}=>match command {
            ConnectionCommand::List=>get(&client,&format!("{base}/v1/access/connections")).await?,
            ConnectionCommand::ImportGitlab{hostname} => native_register(&client,base,"gitlab",hostname,json!({"operation":"import_gitlab","binary":std::env::var("FEYNMAN_GLAB_BIN").unwrap_or_else(|_| "glab".into())})).await?,
            ConnectionCommand::LoginSlate{hostname} => native_register(&client,base,"slate",hostname,json!({"operation":"login_slate"})).await?,
            ConnectionCommand::ImportSlate{hostname,profile} => native_register(&client,base,"slate",hostname,json!({"operation":"import_slate","profile":profile})).await?,
            ConnectionCommand::ImportDrlee{hostname,source_home,handle} => {
                let home = source_home.clone().or_else(|| std::env::var_os("DRLEE_HOME").map(std::path::PathBuf::from)).or_else(|| std::env::var_os("HOME").map(|p|std::path::PathBuf::from(p).join(".drlee"))).context("provide --source-home")?;
                native_register(&client,base,"drlee",hostname,json!({"operation":"import_drlee","source_home":home,"handle":handle})).await?
            },
            ConnectionCommand::LoginDrlee{hostname} => native_register(&client,base,"drlee",hostname,json!({"operation":"login_drlee","binary":std::env::var("FEYNMAN_DRLEE_BIN").unwrap_or_else(|_|"drlee".into())})).await?,
            ConnectionCommand::RegisterSlate{hostname,token_stdin:_} => {
                use std::io::Read;
                let mut token=String::new();std::io::stdin().take(8193).read_to_string(&mut token)?;
                let token=token.trim_end_matches(['\n','\r']);
                if token.is_empty() || token.len()>8192 {bail!("stdin must contain one bounded Slate session");}
                post(&client,&format!("{base}/v1/access/connections"),&json!({"provider":"slate","hostname":hostname,"token":token})).await?
            },
            ConnectionCommand::RegisterGitlab {hostname,token_stdin:_}=>{
                use std::io::Read;
                let mut token=String::new();std::io::stdin().take(8193).read_to_string(&mut token).context("could not read credential from stdin")?;
                let token=token.trim_end_matches(['\n','\r']);
                if token.is_empty() || token.len()>8192 {bail!("stdin must contain one bounded credential");}
                post(&client,&format!("{base}/v1/access/connections"),&json!({"provider":"gitlab","hostname":hostname,"token":token})).await?
            }
            ConnectionCommand::RegisterDws {project_id,authorization_id}=>post(&client,&format!("{base}/v1/access/connections"),&json!({"provider":"dws","project_id":project_id,"authorization_id":authorization_id})).await?,
            ConnectionCommand::Disable {connection_id}=>post(&client,&format!("{base}/v1/access/connections/{connection_id}/disable"),&json!({})).await?,
        },
        AccessCommand::Status {project_id}=>get(&client,&format!("{base}/v1/projects/{project_id}/access")).await?,
        AccessCommand::Request {project_id,provider,capability,reason}=>post(&client,&format!("{base}/v1/projects/{project_id}/access/requests"),&json!({"provider":provider.wire(),"capability":capability,"reason":reason})).await?,
        AccessCommand::Bind {project_id,provider,connection,repository,resource_id,ttl_seconds}=>post(&client,&format!("{base}/v1/projects/{project_id}/access/bindings"),&json!({"provider":provider.wire(),"connection_id":connection,"repository":repository,"resource_id":resource_id,"ttl_seconds":ttl_seconds})).await?,
        AccessCommand::Unbind {project_id,provider}=>{
            check_response(client.delete(format!("{base}/v1/projects/{project_id}/access/bindings/{}",provider.wire())).send().await?).await?;
            json!({"unbound":true})
        },
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

async fn native_register(
    client: &Client,
    base: &str,
    provider: &str,
    hostname: &str,
    mut payload: Value,
) -> Result<Value> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // Endpoints are owner choices, never Agent-supplied login destinations.
    if hostname.len() > 253
        || !hostname.contains('.')
        || !hostname
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    {
        bail!("provide a DNS hostname, without scheme or path");
    }
    payload["hostname"] = json!(hostname.to_ascii_lowercase());
    let mut cmd = tokio::process::Command::new(
        std::env::var_os("FEYNMAN_NATIVE_PYTHON").unwrap_or_else(|| "python3".into()),
    );
    cmd.args(["-c", feynman_local::NATIVE_CLI])
        .env_clear()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true);
    for key in [
        "HOME",
        "PATH",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "https_proxy",
        "http_proxy",
        "all_proxy",
        "no_proxy",
        "XDG_CONFIG_HOME",
    ] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    let mut child = cmd
        .spawn()
        .context("native authorization helper unavailable")?;
    let mut input = child.stdin.take().context("native stdin unavailable")?;
    let mut output = child
        .stdout
        .take()
        .context("native stdout unavailable")?
        .take(16385);
    let (status, bytes) = tokio::time::timeout(std::time::Duration::from_secs(330), async {
        input.write_all(payload.to_string().as_bytes()).await?;
        drop(input);
        let mut bytes = Vec::new();
        output.read_to_end(&mut bytes).await?;
        Ok::<_, std::io::Error>((child.wait().await?, bytes))
    })
    .await
    .context("native authorization timed out")??;
    if !status.success() || bytes.len() > 16384 {
        bail!("native authorization incomplete; no Connection registered");
    }
    let value: Value =
        serde_json::from_slice(&bytes).context("invalid private native authorization response")?;
    let token = value["credential"]
        .as_str()
        .context("private native credential missing")?;
    post(
        client,
        &format!("{base}/v1/access/connections"),
        &json!({"provider":provider,"hostname":hostname.to_ascii_lowercase(),"token":token}),
    )
    .await
}
