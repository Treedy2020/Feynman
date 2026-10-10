//! Bounded, non-shell native CLI execution. Never return raw stderr.
use feynman_core::{PlatformError, PlatformResult};
use std::{ffi::OsString, path::Path, process::Stdio};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

pub(crate) fn command(
    binary: OsString,
    args: &[String],
    env: &[(String, String)],
    cwd: &Path,
) -> PlatformResult<Command> {
    use super::security::ExecutionPolicy;
    super::security::FixedCliExecution.validate(args, env)?;
    let mut command = Command::new(binary);
    command
        .env_clear()
        .args(args)
        .envs(env.iter().cloned())
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Operator network settings are allowed; inherited credentials and CLI config are not.
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
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "TMPDIR",
        "SYSTEMROOT",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    Ok(command)
}
pub async fn run(
    binary: OsString,
    args: &[String],
    env: &[(String, String)],
    cwd: &Path,
) -> PlatformResult<Vec<u8>> {
    let mut child = command(binary, args, env, cwd)?
        .spawn()
        .map_err(|_| PlatformError::Runtime("authenticated CLI unavailable".into()))?;
    let out = child
        .stdout
        .take()
        .ok_or_else(|| PlatformError::Runtime("CLI stdout unavailable".into()))?;
    let err = child
        .stderr
        .take()
        .ok_or_else(|| PlatformError::Runtime("CLI stderr unavailable".into()))?;
    async fn capped(mut stream: impl AsyncRead + Unpin) -> std::io::Result<(Vec<u8>, bool)> {
        let mut result = Vec::new();
        let mut buffer = [0u8; 8192];
        let mut overflow = false;
        loop {
            let count = stream.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            let limit = (1_048_576usize).saturating_sub(result.len());
            result.extend_from_slice(&buffer[..count.min(limit)]);
            overflow |= count > limit;
        }
        Ok((result, overflow))
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(35), async {
        let (status, out, err) = tokio::join!(child.wait(), capped(out), capped(err));
        Ok::<_, std::io::Error>((status?, out?, err?))
    })
    .await
    .map_err(|_| PlatformError::Runtime("authenticated CLI timed out".into()))?
    .map_err(|_| PlatformError::Runtime("authenticated CLI failed".into()))?;
    if !result.0.success() || result.1.1 || result.2.1 {
        return Err(PlatformError::Runtime(
            "authenticated CLI failed or exceeded output limit".into(),
        ));
    }
    Ok(result.1.0)
}
