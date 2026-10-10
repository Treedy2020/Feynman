use async_trait::async_trait;
use chrono::Utc;
use feynman_core::{
    ExternalToolAdapter, ExternalToolContext, ExternalToolDefinition, ExternalToolResult,
    PlatformError, PlatformResult,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

const TOOL_ID: &str = "dws.aisearch.enterprise";
const MCP_NAME: &str = "dws_search_enterprise";
const MAX_STDOUT_BYTES: usize = 1_000_000;
const MAX_STDERR_BYTES: usize = 16_384;
const PROCESS_TIMEOUT: Duration = Duration::from_secs(35);

#[derive(Default)]
pub struct DwsEnterpriseSearchAdapter {
    runner: Option<Box<dyn DwsCommandRunner>>,
    pub access_server: Option<String>,
}

impl DwsEnterpriseSearchAdapter {
    pub fn with_access_server(access_server: Option<String>) -> Self {
        Self {
            access_server,
            ..Self::default()
        }
    }
    #[cfg(test)]
    fn with_runner(runner: impl DwsCommandRunner + 'static) -> Self {
        Self {
            runner: Some(Box::new(runner)),
            access_server: None,
        }
    }

    fn runner(&self) -> &dyn DwsCommandRunner {
        self.runner.as_deref().unwrap_or(&ProcessDwsCommandRunner)
    }
}

#[async_trait]
impl ExternalToolAdapter for DwsEnterpriseSearchAdapter {
    fn definitions(&self) -> Vec<ExternalToolDefinition> {
        vec![ExternalToolDefinition {
            read_only: true,
            id: TOOL_ID.into(),
            mcp_name: MCP_NAME.into(),
            name: "DWS 企业内容搜索".into(),
            description: "只读搜索钉钉企业内部文档、消息、日程、待办、纪要、邮件等内容；只负责发现内容，不读取或修改命中的资源。显式时间词应传入 time_range，内容类型应传入 types。".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "queries": {
                        "type": "string",
                        "description": "内容主题关键词，多个主题用逗号分隔；不包含时间词和内容类型词。汇总查询可以省略。",
                        "maxLength": 512
                    },
                    "types": {
                        "type": "array",
                        "description": "内容类型；没有明确类型时传 [\"all\"]。all 不能与其他类型混用。",
                        "items": {
                            "type": "string",
                            "enum": ["all", "document", "im", "calendar", "todo", "minute", "report", "image", "link", "notable", "baike", "mail"]
                        },
                        "minItems": 1,
                        "maxItems": 12,
                        "default": ["all"]
                    },
                    "time_range": {
                        "type": "string",
                        "description": "仅用户明确给出时间范围时填写，例如 今天、本周、最近、过去一周。",
                        "maxLength": 100
                    }
                },
                "additionalProperties": false
            }),
        }]
    }

    async fn invoke(
        &self,
        context: &ExternalToolContext,
        tool_id: &str,
        arguments: Value,
    ) -> PlatformResult<ExternalToolResult> {
        if tool_id != TOOL_ID {
            return Err(PlatformError::InvalidInput(
                "unsupported DWS external tool".into(),
            ));
        }
        if let Some(base) = &self.access_server {
            parse_arguments(arguments.clone())?;
            return super::super::super::proxy::invoke(base, context, tool_id, arguments).await;
        }
        let arguments = parse_arguments(arguments)?;
        let args = command_args(&arguments);
        let data = self.runner().run(&args).await?;
        let _ = (&context.principal, &context.project_id);
        Ok(ExternalToolResult {
            source: TOOL_ID.into(),
            observed_at: Utc::now(),
            data,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DwsSearchArguments {
    queries: Option<String>,
    #[serde(default = "default_types")]
    types: Vec<DwsContentType>,
    time_range: Option<String>,
}

fn default_types() -> Vec<DwsContentType> {
    vec![DwsContentType::All]
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DwsContentType {
    All,
    Document,
    Im,
    Calendar,
    Todo,
    Minute,
    Report,
    Image,
    Link,
    Notable,
    Baike,
    Mail,
}

impl DwsContentType {
    fn as_cli(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Document => "document",
            Self::Im => "im",
            Self::Calendar => "calendar",
            Self::Todo => "todo",
            Self::Minute => "minute",
            Self::Report => "report",
            Self::Image => "image",
            Self::Link => "link",
            Self::Notable => "notable",
            Self::Baike => "baike",
            Self::Mail => "mail",
        }
    }
}

fn parse_arguments(value: Value) -> PlatformResult<DwsSearchArguments> {
    let arguments: DwsSearchArguments = serde_json::from_value(value)
        .map_err(|_| PlatformError::InvalidInput("invalid DWS search arguments".into()))?;
    if arguments.types.is_empty()
        || (arguments.types.contains(&DwsContentType::All) && arguments.types.len() != 1)
    {
        return Err(PlatformError::InvalidInput(
            "DWS types must be non-empty; 'all' cannot be combined with other types".into(),
        ));
    }
    if arguments
        .queries
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(PlatformError::InvalidInput(
            "DWS queries must be omitted or contain a non-empty search term".into(),
        ));
    }
    if arguments
        .queries
        .as_ref()
        .is_some_and(|value| value.chars().count() > 512)
        || arguments
            .time_range
            .as_ref()
            .is_some_and(|value| value.chars().count() > 100)
        || arguments.types.len() > 12
    {
        return Err(PlatformError::InvalidInput(
            "DWS search arguments exceed their size limits".into(),
        ));
    }
    Ok(arguments)
}

fn command_args(arguments: &DwsSearchArguments) -> Vec<String> {
    let mut args = vec![
        "aisearch".into(),
        "enterprise".into(),
        "--types".into(),
        arguments
            .types
            .iter()
            .map(|item| item.as_cli())
            .collect::<Vec<_>>()
            .join(","),
    ];
    if let Some(queries) = &arguments.queries {
        args.extend(["--queries".into(), queries.clone()]);
    }
    if let Some(time_range) = &arguments.time_range {
        args.extend(["--time-range".into(), time_range.clone()]);
    }
    args.extend(["--format".into(), "json".into()]);
    args
}

pub(super) fn validated_args(value: Value) -> PlatformResult<Vec<String>> {
    Ok(command_args(&parse_arguments(value)?))
}

#[async_trait]
trait DwsCommandRunner: Send + Sync {
    async fn run(&self, args: &[String]) -> PlatformResult<Value>;
}

struct ProcessDwsCommandRunner;

#[async_trait]
impl DwsCommandRunner for ProcessDwsCommandRunner {
    async fn run(&self, args: &[String]) -> PlatformResult<Value> {
        if std::env::var("FEYNMAN_ALLOW_LEGACY_DWS_PROFILE").as_deref() != Ok("1") {
            return Err(PlatformError::Conflict("DWS requires an External Access server and explicit Connection binding; legacy host profile is disabled".into()));
        }
        let executable: OsString =
            std::env::var_os("FEYNMAN_DWS_BIN").unwrap_or_else(|| OsString::from("dws"));
        let mut child = Command::new(executable)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| PlatformError::Runtime("DWS CLI could not be started".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PlatformError::Runtime("DWS stdout unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| PlatformError::Runtime("DWS stderr unavailable".into()))?;
        let output = tokio::time::timeout(PROCESS_TIMEOUT, async {
            let (status, stdout, stderr) = tokio::join!(
                child.wait(),
                read_capped(stdout, MAX_STDOUT_BYTES),
                read_capped(stderr, MAX_STDERR_BYTES)
            );
            let status = status?;
            let (stdout, stdout_truncated) = stdout?;
            let _ = stderr?;
            Ok::<_, std::io::Error>((status, stdout, stdout_truncated))
        })
        .await
        .map_err(|_| PlatformError::Runtime("DWS search timed out".into()))?
        .map_err(|_| PlatformError::Runtime("DWS output could not be read".into()))?;
        let (status, stdout, stdout_truncated) = output;
        if stdout_truncated {
            return Err(PlatformError::Runtime(
                "DWS search result exceeded the 1 MiB response limit".into(),
            ));
        }
        if !status.success() {
            return Err(PlatformError::Runtime(format!(
                "DWS search failed with exit code {}. Check DWS authorization on the Feynman server host.",
                status.code().unwrap_or(-1)
            )));
        }
        serde_json::from_slice(&stdout)
            .map_err(|_| PlatformError::Runtime("DWS returned invalid JSON".into()))
    }
}

async fn read_capped(
    mut reader: impl AsyncRead + Unpin,
    max_bytes: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut captured = Vec::with_capacity(max_bytes.min(16 * 1024));
    let mut buffer = [0_u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let available = max_bytes.saturating_sub(captured.len());
        let accepted = count.min(available);
        captured.extend_from_slice(&buffer[..accepted]);
        truncated |= accepted < count;
    }
    Ok((captured, truncated))
}

#[cfg(test)]
mod tests {
    use super::{DwsCommandRunner, DwsEnterpriseSearchAdapter, parse_arguments};
    use async_trait::async_trait;
    use feynman_core::{ExternalToolAdapter, ExternalToolContext, PlatformError, PlatformResult};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    struct FakeRunner {
        args: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl DwsCommandRunner for FakeRunner {
        async fn run(&self, args: &[String]) -> PlatformResult<Value> {
            *self.args.lock().unwrap() = args.to_vec();
            Ok(json!({"results":[]}))
        }
    }

    #[test]
    fn search_arguments_are_allowlisted_and_typed() {
        let args = parse_arguments(json!({
            "queries":"project; $(touch /tmp/not-run)",
            "types":["document","mail"],
            "time_range":"本周"
        }))
        .unwrap();
        assert_eq!(
            super::command_args(&args),
            [
                "aisearch",
                "enterprise",
                "--types",
                "document,mail",
                "--queries",
                "project; $(touch /tmp/not-run)",
                "--time-range",
                "本周",
                "--format",
                "json"
            ]
        );
        assert!(parse_arguments(json!({"types":["all","mail"]})).is_err());
        assert!(parse_arguments(json!({"command":"dws chat send"})).is_err());
        assert!(matches!(
            parse_arguments(json!({"types":["unknown"]})),
            Err(PlatformError::InvalidInput(_))
        ));
        assert!(matches!(
            parse_arguments(json!({"queries":"x".repeat(513)})),
            Err(PlatformError::InvalidInput(_))
        ));
        assert!(matches!(
            parse_arguments(json!({"time_range":"x".repeat(101)})),
            Err(PlatformError::InvalidInput(_))
        ));
    }

    #[tokio::test]
    async fn adapter_calls_only_the_dws_enterprise_search_command() {
        let args = Arc::new(Mutex::new(Vec::new()));
        let adapter = DwsEnterpriseSearchAdapter::with_runner(FakeRunner { args: args.clone() });
        let definition = adapter.definitions().remove(0);
        assert_eq!(definition.id, "dws.aisearch.enterprise");
        let context = ExternalToolContext {
            principal: feynman_core::Principal {
                user_id: "user".into(),
                tenant_id: "tenant".into(),
            },
            project_id: "project".into(),
        };
        let result = adapter
            .invoke(
                &context,
                "dws.aisearch.enterprise",
                json!({"queries":"release","types":["im"]}),
            )
            .await
            .unwrap();
        assert_eq!(result.source, "dws.aisearch.enterprise");
        assert_eq!(result.data, json!({"results":[]}));
        assert_eq!(
            *args.lock().unwrap(),
            [
                "aisearch",
                "enterprise",
                "--types",
                "im",
                "--queries",
                "release",
                "--format",
                "json"
            ]
        );
    }
}
