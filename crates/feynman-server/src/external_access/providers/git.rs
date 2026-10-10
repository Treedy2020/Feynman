use crate::external_access::cli_executor;
use feynman_core::{PlatformError, PlatformResult};
use serde_json::{Value, json};
use std::{io::Write, path::Path};

pub async fn read(host: &str, token: &str, repo: &str, cwd: &Path) -> PlatformResult<Value> {
    let remote = format!("https://{host}/{repo}.git");
    let helper = std::env::current_exe()
        .map_err(|_| PlatformError::Runtime("Git credential helper unavailable".into()))?;
    let helper = format!("'{}'", helper.to_string_lossy().replace('\'', "'\\''"));
    let args = vec![
        "-c".into(),
        "credential.helper=".into(),
        "-c".into(),
        "credential.useHttpPath=true".into(),
        "-c".into(),
        "http.followRedirects=false".into(),
        "-c".into(),
        "protocol.allow=never".into(),
        "-c".into(),
        "protocol.https.allow=always".into(),
        "ls-remote".into(),
        "--heads".into(),
        "--tags".into(),
        remote.clone(),
    ];
    let env = vec![
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
        ("GIT_ASKPASS".into(), helper),
        ("FEYNMAN_GIT_ASKPASS".into(), "1".into()),
        ("FEYNMAN_GIT_EXPECTED_REMOTE".into(), remote),
        ("FEYNMAN_GIT_TOKEN".into(), token.into()),
    ];
    let output = cli_executor::run(
        std::env::var_os("FEYNMAN_GIT_BIN").unwrap_or_else(|| "git".into()),
        &args,
        &env,
        cwd,
    )
    .await?;
    let text = std::str::from_utf8(&output)
        .map_err(|_| PlatformError::Runtime("invalid Git output".into()))?;
    let mut refs = Vec::new();
    for line in text.lines() {
        let (hash, name) = line
            .split_once('\t')
            .ok_or_else(|| PlatformError::Runtime("invalid Git refs".into()))?;
        if !matches!(hash.len(), 40 | 64)
            || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            || !(name.starts_with("refs/heads/") || name.starts_with("refs/tags/"))
            || name.contains(token)
            || name.chars().any(char::is_control)
        {
            return Err(PlatformError::Runtime("invalid Git refs".into()));
        }
        refs.push(json!({"object_id":hash,"ref":name}));
    }
    Ok(json!({"repository":repo,"refs":refs}))
}

pub fn askpass() -> anyhow::Result<()> {
    let prompt = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("missing credential prompt"))?;
    let url = prompt
        .split('\'')
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("untrusted credential prompt"))?;
    let actual = reqwest::Url::parse(url)?;
    let expected = reqwest::Url::parse(&std::env::var("FEYNMAN_GIT_EXPECTED_REMOTE")?)?;
    if actual.scheme() != expected.scheme()
        || actual.host_str() != expected.host_str()
        || actual.port() != expected.port()
        || actual.path() != expected.path()
        || actual.query().is_some()
        || actual.fragment().is_some()
        || actual.password().is_some()
    {
        anyhow::bail!("credential destination mismatch");
    }
    if prompt.starts_with("Username for ") {
        println!("oauth2");
    } else if prompt.starts_with("Password for ") {
        std::io::stdout().write_all(std::env::var("FEYNMAN_GIT_TOKEN")?.as_bytes())?;
    } else {
        anyhow::bail!("unsupported credential prompt");
    }
    Ok(())
}
