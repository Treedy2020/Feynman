use super::*;
use chrono::{DateTime, Duration};
use feynman_core::{
    AgentInput, InboxInput, InboxMessage, Observation, ObservationInput, ObservationSource,
    PlatformRepository, ProjectAgent, SourceInput,
};

impl ProjectManager {
    pub fn agents(&self, p: &Principal, project: &str) -> PlatformResult<Vec<ProjectAgent>> {
        self.repository.agents(p, project)
    }
    pub fn save_agent(
        &self,
        p: &Principal,
        project: &str,
        id: Option<&str>,
        input: AgentInput,
    ) -> PlatformResult<ProjectAgent> {
        let known = self.platform.list_project_tools(p, project)?;
        if input
            .capabilities
            .iter()
            .any(|cap| !known.iter().any(|t| &t.id == cap))
        {
            return Err(PlatformError::InvalidInput(
                "unknown Agent capability; inspect external-tools list".into(),
            ));
        }
        self.repository.save_agent(p, project, id, input)
    }
    pub fn sources(&self, p: &Principal, project: &str) -> PlatformResult<Vec<ObservationSource>> {
        self.repository.sources(p, project)
    }
    pub(super) fn source_permission(
        &self,
        p: &Principal,
        source: &ObservationSource,
    ) -> PlatformResult<()> {
        self.repository
            .resolve_agent(p, &source.project_id, Some(&source.agent_id))?;
        if source.kind.starts_with("dws_") {
            self.access
                .as_ref()
                .ok_or_else(|| PlatformError::Conflict("External Access not configured".into()))?
                .validate_dws_source(
                    p,
                    &source.project_id,
                    source
                        .connection_id
                        .as_deref()
                        .ok_or(PlatformError::Unauthorized)?,
                    source.arguments.clone(),
                )?;
        }
        if source.kind == "dws_events" {
            self.repository
                .check_agent_tool(p, &source.project_id, "dws.event.messages.listen")?;
        }
        Ok(())
    }
    pub fn create_source(
        &self,
        p: &Principal,
        project: &str,
        input: SourceInput,
    ) -> PlatformResult<ObservationSource> {
        // Even disabled DWS registrations require an explicit, currently authorized scope.
        if input.kind.starts_with("dws_") {
            let args = input.arguments.clone();
            if input.kind == "dws_events"
                && !matches!(
                    args["target_kind"].as_str(),
                    Some("group" | "open_dingtalk_id")
                )
            {
                return Err(PlatformError::InvalidInput("live events require a stable group or open_dingtalk_id target; numeric user history is supported separately".into()));
            }
            if args.as_object().is_none_or(|a| a.len() != 4)
                || args["start"]
                    .as_str()
                    .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
                    .is_none()
                || args["end"]
                    .as_str()
                    .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
                    .is_none()
            {
                return Err(PlatformError::InvalidInput(
                    "DWS source arguments need target_kind,target_id,start,end RFC3339".into(),
                ));
            }
            let a = self
                .repository
                .resolve_agent(p, project, input.agent_id.as_deref())?;
            let sample = ObservationSource {
                id: String::new(),
                project_id: project.into(),
                agent_id: a.id,
                name: input.name.clone(),
                kind: input.kind.clone(),
                connection_id: input.connection_id.clone(),
                arguments: args,
                enabled: input.enabled,
                poll_seconds: input.poll_seconds,
                cursor: Utc::now(),
                next_poll_at: Utc::now(),
                coverage: "unknown".into(),
                listener_state: "stopped".into(),
                listener_until: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };
            self.source_permission(p, &sample)?;
        }
        let mut source = self.repository.create_source(p, project, input)?;
        if source.kind.starts_with("dws_") {
            source.cursor =
                DateTime::parse_from_rfc3339(source.arguments["start"].as_str().unwrap())
                    .map_err(|_| PlatformError::InvalidInput("invalid start".into()))?
                    .with_timezone(&Utc);
            self.repository.save_source(p, &source)?;
        }
        Ok(source)
    }
    pub fn set_source_enabled(
        &self,
        p: &Principal,
        project: &str,
        id: &str,
        enabled: bool,
    ) -> PlatformResult<ObservationSource> {
        let mut s = self.repository.source(p, project, id)?;
        if enabled
            && matches!(s.listener_state.as_str(), "interrupted" | "failed")
            && s.listener_until.is_some_and(|until| until > Utc::now())
        {
            return Err(PlatformError::Conflict("previous bounded DWS consumer cleanup is uncertain; wait for listener_until before resuming".into()));
        }
        if enabled {
            self.source_permission(p, &s)?;
        }
        s.enabled = enabled;
        if enabled {
            s.listener_state = "stopped".into();
        }
        s.updated_at = Utc::now();
        self.repository.save_source_enabled(p, &s)?;
        Ok(s)
    }
    pub fn ingest(
        &self,
        p: &Principal,
        project: &str,
        id: &str,
        records: Vec<ObservationInput>,
    ) -> PlatformResult<usize> {
        let source = self.repository.source(p, project, id)?;
        if source.kind != "manual" {
            return Err(PlatformError::InvalidInput(
                "only manual sources accept owner imports".into(),
            ));
        }
        self.repository.record_observations(p, project, id, records)
    }
    pub fn observations(
        &self,
        p: &Principal,
        project: &str,
        id: &str,
        after: i64,
        limit: i64,
    ) -> PlatformResult<Vec<Observation>> {
        let source = self.repository.source(p, project, id)?;
        if source.kind != "manual" {
            self.source_permission(p, &source)?;
        }
        self.repository.observations(p, project, id, after, limit)
    }
    pub fn captures(
        &self,
        p: &Principal,
        project: &str,
        id: &str,
        after: i64,
        limit: i64,
    ) -> PlatformResult<Vec<Value>> {
        let source = self.repository.source(p, project, id)?;
        if source.kind != "manual" {
            self.source_permission(p, &source)?;
        }
        self.repository.captures(p, project, id, after, limit)
    }
    pub fn enqueue(
        &self,
        p: &Principal,
        project: &str,
        input: InboxInput,
    ) -> PlatformResult<InboxMessage> {
        self.repository.enqueue_input(p, project, input)
    }
    pub fn inbox(&self, p: &Principal, project: &str) -> PlatformResult<Vec<InboxMessage>> {
        self.repository.inbox(p, project)
    }
    pub fn resolve_delivery(
        &self,
        p: &Principal,
        project: &str,
        id: &str,
        state: &str,
        run: Option<String>,
    ) -> PlatformResult<InboxMessage> {
        let mut m = self
            .inbox(p, project)?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or(PlatformError::NotFound)?;
        if m.state != "unknown" && !(m.state == "pending" && state == "cancelled") {
            return Err(PlatformError::Conflict(
                "only unknown deliveries may be reconciled; pending may be cancelled".into(),
            ));
        }
        if !matches!(state, "pending" | "accepted" | "cancelled") {
            return Err(PlatformError::InvalidInput(
                "invalid reconciliation state".into(),
            ));
        }
        if state == "accepted" {
            let run_id = run.as_deref().ok_or_else(|| {
                PlatformError::InvalidInput("accepted reconciliation requires run_id".into())
            })?;
            self.platform.get_run_feed(p, project, run_id, 0)?;
            self.repository.check_delivery_run(p, &m, run_id)?;
        }
        self.repository.settle_input(p, &m, state, run.clone())?;
        m.state = state.into();
        m.run_id = run;
        Ok(m)
    }
    pub fn deliveries(&self, p: &Principal, project: &str, id: &str) -> PlatformResult<Value> {
        let schedule = self
            .schedules(p, project)?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or(PlatformError::NotFound)?;
        Ok(
            json!({"schedule":schedule,"window":self.repository.schedule_window(p,project,id)?,"occurrences":self.repository.occurrences(p,project,id)?,"inbox":self.inbox(p,project)?.into_iter().filter(|m|m.schedule_id.as_deref()==Some(id)).collect::<Vec<_>>()}),
        )
    }
    pub async fn backfill(
        &self,
        p: &Principal,
        project: &str,
        id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> PlatformResult<Value> {
        let source = self.repository.source(p, project, id)?;
        if !source.enabled || !source.kind.starts_with("dws_") {
            return Err(PlatformError::Conflict(
                "backfill requires an enabled DWS source".into(),
            ));
        }
        self.collect(p, &source, start, end, false).await
    }
    async fn collect(
        &self,
        p: &Principal,
        source: &ObservationSource,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        advance: bool,
    ) -> PlatformResult<Value> {
        self.source_permission(p, source)?;
        if start >= end || end > Utc::now() || end - start > Duration::hours(24) {
            return Err(PlatformError::InvalidInput(
                "history range must be past, positive, at most 24h per request".into(),
            ));
        }
        let mut args = source.arguments.clone();
        args["start"] = json!(start.to_rfc3339());
        args["end"] = json!(end.to_rfc3339());
        let original_start =
            DateTime::parse_from_rfc3339(source.arguments["start"].as_str().unwrap())
                .map_err(|_| PlatformError::InvalidInput("start".into()))?
                .with_timezone(&Utc);
        let original_end = DateTime::parse_from_rfc3339(source.arguments["end"].as_str().unwrap())
            .map_err(|_| PlatformError::InvalidInput("end".into()))?
            .with_timezone(&Utc);
        if start < original_start || end > original_end {
            return Err(PlatformError::InvalidInput(
                "backfill exceeds immutable source scope".into(),
            ));
        }
        let data = self
            .access
            .as_ref()
            .unwrap()
            .invoke_source(
                p,
                &source.project_id,
                source.connection_id.as_deref().unwrap(),
                args,
            )
            .await?;
        self.source_permission(p, source)?;
        let payload = data.get("data").unwrap_or(&data);
        let messages = payload
            .get("messages")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                PlatformError::Runtime(
                    "DWS messages response has no messages array; cursor retained".into(),
                )
            })?;
        let mut valid = true;
        let mut events = Vec::new();
        for body in messages {
            let id = body
                .get("messageId")
                .or_else(|| body.get("msgId"))
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty());
            let time = body
                .get("occurred_at")
                .or_else(|| body.get("sendTime"))
                .or_else(|| body.get("createTime"));
            let occurred = time.and_then(parse_dws_time);
            if let (Some(id), Some(occurred)) = (id, occurred) {
                if occurred >= start && occurred < end {
                    events.push(ObservationInput {
                        event_id: id.into(),
                        occurred_at: occurred,
                        body: body.clone(),
                    });
                } else {
                    valid = false
                }
            } else {
                valid = false
            }
        }
        let complete = valid
            && payload["complete"].as_bool() == Some(true)
            && payload["hasMore"].as_bool() != Some(true)
            && payload
                .get("failures")
                .is_none_or(|v| v.as_array().is_some_and(|a| a.is_empty()))
            && [
                "partial",
                "truncated",
                "truncatedByPageLimit",
                "truncatedByResultLimit",
            ]
            .iter()
            .all(|k| payload[*k].as_bool() != Some(true))
            && payload["failedCount"].as_i64().unwrap_or(0) == 0;
        if !self
            .repository
            .source(p, &source.project_id, &source.id)?
            .enabled
        {
            return Err(PlatformError::Conflict(
                "source paused before capture completed".into(),
            ));
        }
        let count =
            self.repository
                .record_observations(p, &source.project_id, &source.id, events)?;
        let mut next = self.repository.source(p, &source.project_id, &source.id)?;
        // This labels only the most recent bounded capture, never the whole source history.
        next.coverage = if complete {
            "last_capture_complete"
        } else {
            "partial"
        }
        .into();
        next.updated_at = Utc::now();
        if advance && complete && next.cursor == start {
            next.cursor = end;
        }
        next.next_poll_at = Utc::now() + Duration::seconds(next.poll_seconds);
        self.repository.save_source(p, &next)?;
        let report = json!({"source_id":source.id,"kind":if advance {"poll"}else{"backfill"},"observed_at":Utc::now(),"start":start,"end":end,"inserted":count,"complete":complete,"cursor":next.cursor});
        self.repository.record_capture(p, source, report.clone())?;
        Ok(report)
    }
    pub(super) async fn poll_sources(&self) -> PlatformResult<()> {
        let p = self.platform.authenticate(None).await?;
        for project in self.platform.list_projects(&p)? {
            for mut source in self.sources(&p, &project.id)? {
                if !source.enabled
                    || source.kind != "dws_messages"
                    || source.next_poll_at > Utc::now()
                {
                    continue;
                }
                let end_limit = source.arguments["end"]
                    .as_str()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or(source.cursor);
                let end = Utc::now()
                    .min(source.cursor + Duration::hours(1))
                    .min(end_limit);
                if end <= source.cursor {
                    continue;
                }
                if self
                    .collect(&p, &source, source.cursor, end, true)
                    .await
                    .is_err()
                {
                    self.repository.record_capture(&p,&source,json!({"kind":"poll_error","source_id":source.id,"start":source.cursor,"end":end,"observed_at":Utc::now(),"complete":false,"state":"gap"}))?;
                    source = self.repository.source(&p, &project.id, &source.id)?;
                    source.coverage = "gap".into();
                    source.next_poll_at = Utc::now() + Duration::seconds(source.poll_seconds);
                    source.updated_at = Utc::now();
                    self.repository.save_source(&p, &source)?;
                }
            }
        }
        Ok(())
    }
    pub(super) async fn dispatch_inbox(&self) -> PlatformResult<()> {
        let p = self.platform.authenticate(None).await?;
        for project in self.platform.list_projects(&p)? {
            let messages = self.inbox(&p, &project.id)?;
            if messages
                .iter()
                .any(|m| matches!(m.state.as_str(), "unknown" | "dispatching"))
            {
                continue;
            }
            let Some(message) = messages.into_iter().find(|m| m.state == "pending") else {
                continue;
            };
            if self
                .repository
                .resolve_agent(&p, &project.id, Some(&message.agent_id))
                .is_err()
            {
                continue;
            }
            if self
                .workspace(&p, &project.id)?
                .is_some_and(|w| w.state == "paused" || w.state == "pausing")
            {
                continue;
            }
            if let Some(id) = &message.schedule_id {
                let Some(schedule) = self
                    .schedules(&p, &project.id)?
                    .into_iter()
                    .find(|s| &s.id == id)
                else {
                    continue;
                };
                // One-shot clock disables itself on occurrence creation; its pending input remains deliverable.
                if !schedule.enabled && !schedule.exhausted {
                    continue;
                }
                let mut valid = true;
                for id in &schedule.source_ids {
                    let source = self.repository.source(&p, &project.id, id)?;
                    if !source.enabled || self.source_permission(&p, &source).is_err() {
                        valid = false;
                    }
                }
                if !valid {
                    continue;
                }
            }
            let active = self.platform.get_conversation(&p, &project.id)?.active_run;
            if active.is_some()
                && (message.delivery != "steer_if_active"
                    || active
                        .as_ref()
                        .is_some_and(|r| r.status != feynman_core::AgentRunStatus::Running))
            {
                continue;
            }
            if !self.repository.claim_input(&p, &message)? {
                continue;
            }
            let content = format!("[Feynman Inbox ID: {}]\n{}", message.id, message.prompt);
            let outcome = if active.is_some() {
                self.platform.steer_message(&p, &project.id, &content).await
            } else {
                self.platform
                    .send_message_to_agent(&p, &project.id, &message.agent_id, &content)
                    .await
                    .map(|r| Some(r.run.id))
            };
            match outcome {
                Ok(Some(run)) => {
                    self.repository
                        .settle_input(&p, &message, "accepted", Some(run))?
                }
                Ok(None) | Err(PlatformError::RunAlreadyActive) => self
                    .repository
                    .settle_input(&p, &message, "pending", None)?,
                Err(_) => self
                    .repository
                    .settle_input(&p, &message, "unknown", None)?,
            }
        }
        Ok(())
    }
}
pub(super) fn parse_dws_time(value: &Value) -> Option<DateTime<Utc>> {
    use chrono::TimeZone;
    if let Some(n) = value.as_i64() {
        return DateTime::<Utc>::from_timestamp_millis(n);
    }
    let text = value.as_str()?;
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|d| d.with_timezone(&Utc))
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
                .ok()
                .and_then(|n| {
                    chrono::FixedOffset::east_opt(8 * 3600)?
                        .from_local_datetime(&n)
                        .single()
                        .map(|d| d.with_timezone(&Utc))
                })
        })
}
