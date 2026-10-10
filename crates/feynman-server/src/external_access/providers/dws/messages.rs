//! Typed message observation. Owner scope and business grant are independent gates.
use chrono::{DateTime, FixedOffset};
use feynman_core::{
    ExternalToolAdapter, ExternalToolContext, ExternalToolDefinition, ExternalToolResult,
    PlatformError, PlatformResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const TOOL_ID: &str = "dws.chat.messages.read";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Group,
    User,
    OpenDingtalkId,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub kind: TargetKind,
    pub id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub targets: Vec<Target>,
    pub start: String,
    pub end: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    target_kind: TargetKind,
    target_id: String,
    start: String,
    end: String,
}

fn invalid() -> PlatformError {
    PlatformError::InvalidInput("DWS message target/time must be within the owner-configured scope; RFC3339 times are required".into())
}
fn time(value: &str) -> PlatformResult<DateTime<FixedOffset>> {
    if value.len() > 50 {
        return Err(invalid());
    }
    DateTime::parse_from_rfc3339(value).map_err(|_| invalid())
}
fn valid_target(target: &Target) -> bool {
    let id = &target.id;
    !id.is_empty()
        && id.len() <= 512
        && match target.kind {
            TargetKind::Group => {
                id.starts_with("cid")
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"+/=_-".contains(&b))
            }
            TargetKind::User => id.bytes().all(|b| b.is_ascii_digit()),
            TargetKind::OpenDingtalkId => id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        }
}
impl Scope {
    pub fn validate(&self) -> PlatformResult<()> {
        if self.targets.is_empty()
            || self.targets.len() > 32
            || !self.targets.iter().all(valid_target)
            || time(&self.start)? >= time(&self.end)?
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn command(&self, value: Value) -> PlatformResult<Vec<String>> {
        self.validate()?;
        let args: Arguments = serde_json::from_value(value).map_err(|_| invalid())?;
        let target = Target {
            kind: args.target_kind,
            id: args.target_id,
        };
        if !self.targets.contains(&target)
            || time(&args.start)? < time(&self.start)?
            || time(&args.end)? > time(&self.end)?
            || time(&args.start)? >= time(&args.end)?
        {
            return Err(invalid());
        }
        let flag = match target.kind {
            TargetKind::Group => "--group",
            TargetKind::User => "--user",
            TargetKind::OpenDingtalkId => "--open-dingtalk-id",
        };
        Ok(vec![
            "chat".into(),
            "+chat-messages".into(),
            flag.into(),
            target.id,
            "--start".into(),
            args.start,
            "--end".into(),
            args.end,
            "--order".into(),
            "asc".into(),
            "--page-all".into(),
            "--page-limit".into(),
            "50".into(),
            "--max-items".into(),
            "1000".into(),
            "--format".into(),
            "json".into(),
        ])
    }
}

pub struct MessagesAdapter {
    pub server: Option<String>,
}
#[async_trait::async_trait]
impl ExternalToolAdapter for MessagesAdapter {
    fn definitions(&self) -> Vec<ExternalToolDefinition> {
        vec![ExternalToolDefinition {
            id: TOOL_ID.into(), mcp_name: "dws_read_messages".into(), name: "DWS 受限消息读取".into(), read_only: true,
            description: "读取用户为本项目明确授权的群/单聊与时间区间；范围 [start,end)。最多 50 页/1000 条，检查 complete、hasMore、failures，部分结果不得称完整。不下载附件、不接受任意 CLI 参数、不返回凭证。历史编辑/撤回不能由时间范围恢复；盲测使用冻结事实而非实时工具。".into(),
            input_schema: json!({"type":"object","properties":{
                "target_kind":{"type":"string","enum":["group","user","open_dingtalk_id"]},"target_id":{"type":"string","maxLength":512},
                "start":{"type":"string","description":"RFC3339，含时区"},"end":{"type":"string","description":"RFC3339，不含该时刻"}},
                "required":["target_kind","target_id","start","end"],"additionalProperties":false}),
        },ExternalToolDefinition{id:"dws.event.messages.listen".into(),mcp_name:"dws_event_capability".into(),name:"DWS 托管消息监听权限".into(),read_only:true,description:"Capability for user-managed dws_events sources. Inspect source status through feynman_observations. This tool cannot create a subscription or start an arbitrary event process.".into(),input_schema:json!({"type":"object","properties":{},"additionalProperties":false})}]
    }
    async fn invoke(
        &self,
        context: &ExternalToolContext,
        tool: &str,
        args: Value,
    ) -> PlatformResult<ExternalToolResult> {
        if tool == "dws.event.messages.listen" {
            if args != json!({}) {
                return Err(invalid());
            }
            return Ok(ExternalToolResult {
                source: tool.into(),
                observed_at: chrono::Utc::now(),
                data: json!({"execution":"managed_sources_only","next_step":"Owner creates/enables a dws_events source via feynman sources; inspect its actual listener_state through feynman_observations."}),
            });
        }
        if tool != TOOL_ID {
            return Err(invalid());
        }
        let server = self
            .server
            .as_deref()
            .ok_or_else(|| PlatformError::Conflict("External Access server required".into()))?;
        crate::external_access::proxy::invoke(server, context, tool, args).await
    }
}
