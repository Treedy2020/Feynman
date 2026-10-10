use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde_json::json;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Subcommand)]
pub enum AuthCommand {
    /// Ask native Codex for login status; cache presence alone is not authentication.
    Status,
    /// Native browser login by default; --device for a headless/device flow.
    Login {
        #[arg(long)]
        device: bool,
    },
    /// Logs out only the dedicated Feynman model identity, not your personal harness.
    Logout,
}

pub async fn handle(action: &AuthCommand) -> Result<()> {
    let home = feynman_local::LocalHome::from_env()?;
    home.initialize()?;
    let command = || {
        let mut command =
            Command::new(std::env::var_os("FEYNMAN_CODEX_BIN").unwrap_or_else(|| "codex".into()));
        command
            .env("CODEX_HOME", &home.codex_home)
            .env("CODEX_SQLITE_HOME", &home.codex_home)
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY")
            .env_remove("CODEX_ACCESS_TOKEN")
            .kill_on_drop(true);
        command
    };
    if !matches!(action, AuthCommand::Status) {
        let mut native = command();
        match action {
            AuthCommand::Login { device } => {
                native.arg("login");
                if *device {
                    native.arg("--device-auth");
                }
            }
            AuthCommand::Logout => {
                native.arg("logout");
            }
            AuthCommand::Status => unreachable!(),
        }
        // Native login owns user interaction. Keep its output off JSON stdout.
        native
            .stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = native
            .spawn()
            .context("could not start native Codex authentication")?;
        let mut lines = BufReader::new(
            child
                .stdout
                .take()
                .context("native login output unavailable")?,
        )
        .lines();
        while let Some(line) = lines.next_line().await? {
            eprintln!("{line}");
        }
        if !child.wait().await?.success() {
            bail!("native Codex authentication did not complete");
        }
    }
    let status = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        command()
            .args(["login", "status"])
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .context("Codex login status timed out")??;
    // Never expose native status text: API-key login may contain a key prefix.
    if !matches!(status.status.code(), Some(0 | 1)) {
        bail!("native Codex login status failed");
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "provider":"codex", "codex_home":home.codex_home,
            "authenticated":status.status.success(),
            "note":"Native login status is not proof of model entitlement; a completed inference turn verifies access. This is local model authentication, not a network Feynman account."
        }))?
    );
    Ok(())
}
