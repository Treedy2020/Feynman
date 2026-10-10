use super::AuthenticatedCliProvider;
use crate::external_access::{cli_executor, store::private_dir};
use async_trait::async_trait;
use feynman_core::{AccessProvider, CliProviderDescriptor, PlatformError, PlatformResult};
use serde_json::{Value, json};
use std::path::PathBuf;

pub struct GitlabProvider {
    pub root: PathBuf,
}
impl GitlabProvider {
    pub async fn api(&self, hostname: &str, token: &str, endpoint: &str) -> PlatformResult<Value> {
        private_dir(&self.root)?;
        let args = vec![
            "api".into(),
            "--hostname".into(),
            hostname.into(),
            "--method".into(),
            "GET".into(),
            endpoint.into(),
        ];
        let env = vec![
            ("GITLAB_TOKEN".into(), token.into()),
            ("GLAB_CONFIG_DIR".into(), self.root.to_string_lossy().into()),
            ("GLAB_ENABLE_CI_AUTOLOGIN".into(), "false".into()),
            ("GLAB_CHECK_UPDATE".into(), "false".into()),
            ("GITLAB_HOST".into(), hostname.into()),
        ];
        let output = cli_executor::run(
            std::env::var_os("FEYNMAN_GLAB_BIN").unwrap_or_else(|| "glab".into()),
            &args,
            &env,
            &self.root,
        )
        .await?;
        serde_json::from_slice(&output)
            .map_err(|_| PlatformError::Runtime("GitLab returned invalid JSON".into()))
    }
    pub async fn read(
        &self,
        host: &str,
        token: &str,
        repo: &str,
        tool: &str,
    ) -> PlatformResult<Value> {
        let encoded: String = repo
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.') {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        let endpoint = if tool == "gitlab.repo.read" {
            format!("projects/{encoded}")
        } else {
            format!("projects/{encoded}/merge_requests?state=opened&per_page=50")
        };
        let data = self.api(host, token, &endpoint).await?;
        let project_fields = [
            "id",
            "path_with_namespace",
            "web_url",
            "description",
            "default_branch",
        ];
        let mr_fields = [
            "iid",
            "title",
            "state",
            "web_url",
            "source_branch",
            "target_branch",
        ];
        let project = |item: &Value, fields: &[&str]| -> Value {
            Value::Object(
                fields
                    .iter()
                    .filter_map(|key| item.get(*key).map(|v| (key.to_string(), v.clone())))
                    .collect(),
            )
        };
        let mut result = if tool == "gitlab.repo.read" {
            project(&data, &project_fields)
        } else {
            json!(
                data.as_array()
                    .ok_or_else(|| PlatformError::Runtime("invalid GitLab MR response".into()))?
                    .iter()
                    .map(|item| project(item, &mr_fields))
                    .collect::<Vec<_>>()
            )
        };
        redact(&mut result, token);
        Ok(result)
    }
}
pub fn redact(value: &mut Value, token: &str) {
    match value {
        Value::String(s) => {
            *s = s.replace(token, "[redacted]");
        }
        Value::Object(o) => {
            for v in o.values_mut() {
                redact(v, token);
            }
        }
        Value::Array(a) => {
            for v in a {
                redact(v, token);
            }
        }
        _ => {}
    }
}
#[async_trait]
impl AuthenticatedCliProvider for GitlabProvider {
    fn descriptor(&self) -> CliProviderDescriptor {
        CliProviderDescriptor {
            provider: AccessProvider::Gitlab,
            display_name: "GitLab".into(),
            summary: "读取指定仓库的元信息和开放合并请求。".into(),
            source_kinds: vec![],
            executable: "glab".into(),
            authentication: vec!["owner_token_stdin".into()],
            capabilities: vec!["gitlab.repo.read".into(), "gitlab.mr.read".into()],
            delivery: "server_proxy".into(),
        }
    }
    async fn verify(&self, host: &str, token: &str) -> PlatformResult<String> {
        let data = self.api(host, token, "user").await?;
        let id = data
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| PlatformError::Runtime("GitLab identity not verified".into()))?;
        let username = data
            .get("username")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 128 && !s.contains(token))
            .ok_or_else(|| PlatformError::Runtime("GitLab identity not verified".into()))?;
        Ok(format!("{id}:{username}"))
    }
}
