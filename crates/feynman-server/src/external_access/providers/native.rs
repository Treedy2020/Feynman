//! Native authentication semantics behind one fixed bridge; no arbitrary CLI escape hatch.
use crate::external_access::store::private_dir;
use feynman_core::{AccessProvider, CliProviderDescriptor, PlatformError, PlatformResult};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub fn descriptors() -> Vec<CliProviderDescriptor> {
    [
        (
            AccessProvider::Slate,
            "dip-slate",
            vec!["browser_pkce", "owner_keychain_import", "owner_token_stdin"],
            vec![
                "slate.identity.read",
                "slate.tasks.read",
                "slate.projects.read",
            ],
        ),
        (
            AccessProvider::Drlee,
            "drlee",
            vec!["native_browser", "owner_account_import"],
            vec![
                "drlee.identity.read",
                "drlee.channels.read",
                "drlee.messages.read",
                "drlee.knowledge.read",
            ],
        ),
    ]
    .into_iter()
    .map(|(provider, executable, auth, caps)| CliProviderDescriptor {
        provider,
        display_name: match provider {
            AccessProvider::Slate => "Slate",
            _ => "Drlee",
        }
        .into(),
        summary: match provider {
            AccessProvider::Slate => "读取账号身份、我的任务和可见项目。",
            _ => "读取账号身份、渠道、限定渠道消息和个人知识空间。",
        }
        .into(),
        source_kinds: vec![],
        executable: executable.into(),
        authentication: auth.into_iter().map(str::to_owned).collect(),
        capabilities: caps.into_iter().map(str::to_owned).collect(),
        delivery: "server_proxy".into(),
    })
    .collect()
}

pub async fn run(mut payload: Value, cwd: &Path) -> PlatformResult<Value> {
    private_dir(cwd)?;
    payload["binary"] =
        json!(std::env::var("FEYNMAN_DRLEE_BIN").unwrap_or_else(|_| "drlee".into()));
    let mut cmd = tokio::process::Command::new(
        std::env::var_os("FEYNMAN_NATIVE_PYTHON").unwrap_or_else(|| "python3".into()),
    );
    cmd.env_clear()
        .args(["-c", feynman_local::NATIVE_CLI])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in [
        "PATH",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "https_proxy",
        "http_proxy",
        "all_proxy",
        "no_proxy",
    ] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    cmd.env("HOME", cwd)
        .env("XDG_CONFIG_HOME", cwd.join("config"));
    let mut child = cmd.spawn().map_err(|_| unavailable())?;
    let mut input = child.stdin.take().ok_or_else(unavailable)?;
    let mut out = child.stdout.take().ok_or_else(unavailable)?.take(1_048_577);
    let result = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        input.write_all(payload.to_string().as_bytes()).await?;
        drop(input);
        let mut bytes = vec![];
        out.read_to_end(&mut bytes).await?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, bytes))
    })
    .await
    .map_err(|_| unavailable())?
    .map_err(|_| unavailable())?;
    if !result.0.success() || result.1.len() > 1_048_576 {
        return Err(unavailable());
    }
    serde_json::from_slice(&result.1).map_err(|_| unavailable())
}
fn unavailable() -> PlatformError {
    PlatformError::Runtime("native authenticated CLI unavailable".into())
}
pub fn identity(provider: AccessProvider, data: &Value) -> PlatformResult<String> {
    let value = if provider == AccessProvider::Drlee {
        &data["identity"]
    } else {
        &data["data"]["id"]
    };
    let id = value
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 128 && !v.chars().any(char::is_control))
        .ok_or_else(unavailable)?;
    Ok(id.to_owned())
}
