//! A bounded snapshot format, containing no original Git database or remote config.
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Read, Write},
    path::{Component, Path},
    process::{Command, Stdio},
};

pub const MAX_BYTES: usize = 32 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedFile {
    pub path: String,
    pub executable: bool,
    pub content_base64: String,
    pub sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedInput {
    pub base_sha: String,
    pub files: Vec<SeedFile>,
    pub facts: serde_json::Value,
    pub excluded_prefixes: Vec<String>,
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.contains(['\\', '\0'])
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        && !path.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.eq_ignore_ascii_case(".git")
        })
}
impl SeedInput {
    pub fn validate(&self) -> Result<()> {
        if !matches!(self.base_sha.len(), 40 | 64)
            || !self.base_sha.bytes().all(|b| b.is_ascii_hexdigit())
            || self.files.is_empty()
            || self.files.len() > 10000
        {
            bail!("invalid baseline snapshot");
        }
        if !self.facts.is_object()
            || self
                .facts
                .get("cutoff")
                .and_then(serde_json::Value::as_str)
                .is_none()
        {
            bail!("facts must be an object with an explicit cutoff");
        }
        let cutoff = chrono::DateTime::parse_from_rfc3339(self.facts["cutoff"].as_str().unwrap())
            .context("facts cutoff must be RFC3339 with a timezone")?;
        if let Some(messages) = self.facts.get("messages") {
            for message in messages
                .as_array()
                .context("facts messages must be an array")?
            {
                let timestamp = message
                    .get("created_at")
                    .and_then(serde_json::Value::as_str)
                    .context("each message needs an explicit created_at")?;
                if chrono::DateTime::parse_from_rfc3339(timestamp)
                    .context("message created_at must be RFC3339")?
                    > cutoff
                {
                    bail!("message exceeds the frozen facts cutoff");
                }
            }
        }
        for forbidden in [
            "expected_plan",
            "target_tag",
            "reference_diff",
            "golden",
            "answers",
        ] {
            if self.facts.get(forbidden).is_some() {
                bail!("evaluator answers must not enter runtime facts");
            }
        }
        let mut paths = HashSet::new();
        let mut size = serde_json::to_vec(&self.facts)?.len();
        for file in &self.files {
            if !safe_path(&file.path) || !paths.insert(&file.path) {
                bail!("unsafe or duplicate snapshot path");
            }
            let bytes = STANDARD
                .decode(&file.content_base64)
                .context("invalid file encoding")?;
            size += bytes.len();
            if size > MAX_BYTES || hash(&bytes) != file.sha256 {
                bail!("snapshot size/hash validation failed");
            }
        }
        Ok(())
    }
}
fn git(repository: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .args(["--no-replace-objects", "-C"])
        .arg(repository)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env("GIT_OPTIONAL_LOCKS", "0");
    command
}
pub fn prepare(
    repository: &Path,
    sha: &str,
    facts_path: &Path,
    output: &Path,
    excludes: &[String],
) -> Result<serde_json::Value> {
    if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("base-sha must be a full commit SHA");
    }
    let resolved = git(repository)
        .args(["rev-parse", "--verify", &format!("{sha}^{{commit}}")])
        .output()?;
    if !resolved.status.success() {
        bail!("baseline commit unavailable");
    }
    let base_sha = String::from_utf8(resolved.stdout)?.trim().to_owned();
    let facts_metadata = std::fs::symlink_metadata(facts_path)?;
    if !facts_metadata.file_type().is_file() || facts_metadata.len() > 4 * 1024 * 1024 {
        bail!("facts must be a bounded regular JSON file");
    }
    let facts = serde_json::from_slice(&std::fs::read(facts_path)?)?;
    let tree = git(repository)
        .args(["ls-tree", "-r", "-z", &base_sha])
        .output()?;
    if !tree.status.success() {
        bail!("cannot read baseline tree");
    }
    let mut batch = git(repository)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut writer = batch.stdin.take().context("Git stdin unavailable")?;
    let mut reader = BufReader::new(batch.stdout.take().context("Git stdout unavailable")?);
    let result = (|| -> Result<SeedInput> {
        let mut files = Vec::new();
        let mut total = 0usize;
        for entry in tree.stdout.split(|b| *b == 0).filter(|v| !v.is_empty()) {
            let entry = std::str::from_utf8(entry).context("snapshot paths must be UTF-8")?;
            let (header, path) = entry.split_once('\t').context("invalid Git tree")?;
            if !safe_path(path) {
                bail!("unsafe baseline path");
            }
            if excludes.iter().any(|prefix| {
                path == prefix || path.starts_with(&format!("{}/", prefix.trim_end_matches('/')))
            }) {
                continue;
            }
            let fields: Vec<_> = header.split_whitespace().collect();
            if fields.len() != 3 || fields[1] != "blob" || !matches!(fields[0], "100644" | "100755")
            {
                bail!("links/submodules require separately audited preparation: {path}");
            }
            let name = Path::new(path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if matches!(name, ".env" | "auth.json" | "credentials.json")
                || name.ends_with(".pem")
                || name.ends_with(".key")
            {
                bail!("credential-like file requires explicit exclusion: {path}");
            }
            writeln!(writer, "{}", fields[2])?;
            writer.flush()?;
            let mut header = String::new();
            reader.read_line(&mut header)?;
            let info: Vec<_> = header.split_whitespace().collect();
            if info.len() != 3 || info[0] != fields[2] || info[1] != "blob" {
                bail!("Git blob unavailable");
            }
            let size: usize = info[2].parse()?;
            total = total.checked_add(size).context("snapshot overflow")?;
            if total > MAX_BYTES {
                bail!("snapshot exceeds 32 MiB");
            }
            let mut bytes = vec![0; size];
            reader.read_exact(&mut bytes)?;
            let mut newline = [0];
            reader.read_exact(&mut newline)?;
            if bytes.starts_with(b"version https://git-lfs.github.com/spec/v1") {
                bail!("LFS pointer requires separately audited content: {path}");
            }
            files.push(SeedFile {
                path: path.into(),
                executable: fields[0] == "100755",
                sha256: hash(&bytes),
                content_base64: STANDARD.encode(bytes),
            });
        }
        Ok(SeedInput {
            base_sha,
            files,
            facts,
            excluded_prefixes: excludes.to_vec(),
        })
    })();
    drop(writer);
    if result.is_err() {
        let _ = batch.kill();
    }
    let exit = batch.wait()?;
    let seed = result?;
    if !exit.success() {
        bail!("Git snapshot preparation failed");
    }
    seed.validate()?;
    let bytes = serde_json::to_vec(&seed)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(output)
        .context("output must be a new file; existing fixtures are not overwritten")?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(
        serde_json::json!({"base_sha":seed.base_sha,"file_count":seed.files.len(),"snapshot_sha256":hash(&bytes),"output":output,"excluded_prefixes":excludes,"requires_fact_and_secret_audit":true}),
    )
}
