//! Project/Agent scoped durable inbox and observation partitions.
use super::*;
use feynman_core::*;
use serde_json::json;

pub(super) fn migrate(c: &Connection) -> PlatformResult<()> {
    // Rebuild only the two legacy uniqueness constraints; IDs and dependent rows survive.
    let has_agent: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('agent_sessions') WHERE name='agent_id')", [], |r| r.get(0)).map_err(storage_error)?;
    if !has_agent {
        c.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE;
          CREATE TABLE sessions_next(id TEXT PRIMARY KEY,tenant_id TEXT NOT NULL REFERENCES tenants(id),project_id TEXT NOT NULL REFERENCES projects(id),runtime TEXT NOT NULL,status TEXT NOT NULL,last_heartbeat_at TEXT,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,agent_id TEXT NOT NULL,UNIQUE(tenant_id,project_id,agent_id));
          INSERT INTO sessions_next SELECT s.*,p.main_agent_id FROM agent_sessions s JOIN projects p ON p.id=s.project_id;
          DROP TABLE agent_sessions; ALTER TABLE sessions_next RENAME TO agent_sessions;
          CREATE TABLE conversations_next(id TEXT PRIMARY KEY,tenant_id TEXT NOT NULL REFERENCES tenants(id),project_id TEXT NOT NULL REFERENCES projects(id),main_agent_id TEXT NOT NULL,runtime_thread_id TEXT,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,UNIQUE(tenant_id,project_id,main_agent_id));
          INSERT INTO conversations_next SELECT * FROM conversations;
          DROP TABLE conversations; ALTER TABLE conversations_next RENAME TO conversations;
          COMMIT; PRAGMA foreign_keys=ON;").map_err(storage_error)?;
        let broken: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
                [],
                |r| r.get(0),
            )
            .map_err(storage_error)?;
        if broken {
            return Err(PlatformError::Storage(
                "agent migration foreign key check failed".into(),
            ));
        }
    }
    c.execute_batch("CREATE TABLE IF NOT EXISTS project_agents(id TEXT PRIMARY KEY,tenant TEXT NOT NULL,project TEXT NOT NULL,data TEXT NOT NULL,state TEXT NOT NULL);
      CREATE UNIQUE INDEX IF NOT EXISTS one_live_agent ON project_agents(tenant,project) WHERE state!='archived';
      CREATE TABLE IF NOT EXISTS observation_sources(id TEXT PRIMARY KEY,tenant TEXT NOT NULL,owner TEXT NOT NULL,project TEXT NOT NULL,agent TEXT NOT NULL,data TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS observations(sequence INTEGER PRIMARY KEY AUTOINCREMENT,tenant TEXT NOT NULL,project TEXT NOT NULL,source TEXT NOT NULL REFERENCES observation_sources(id),event TEXT NOT NULL,occurred TEXT NOT NULL,observed TEXT NOT NULL,body TEXT NOT NULL,UNIQUE(source,event));
      CREATE INDEX IF NOT EXISTS source_history ON observations(tenant,project,source,sequence);
      CREATE TABLE IF NOT EXISTS agent_inbox(id TEXT PRIMARY KEY,tenant TEXT NOT NULL,owner TEXT NOT NULL,project TEXT NOT NULL,agent TEXT NOT NULL,schedule TEXT,state TEXT NOT NULL,data TEXT NOT NULL);
      CREATE UNIQUE INDEX IF NOT EXISTS one_dispatching ON agent_inbox(tenant,project,agent) WHERE state='dispatching';
      CREATE TABLE IF NOT EXISTS schedule_windows(schedule TEXT PRIMARY KEY,tenant TEXT NOT NULL,project TEXT NOT NULL,opened TEXT NOT NULL,closes TEXT NOT NULL,start_sequence INTEGER NOT NULL,end_sequence INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS schedule_watermarks(schedule TEXT PRIMARY KEY,sequence INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS schedule_occurrences(id TEXT PRIMARY KEY,tenant TEXT NOT NULL,project TEXT NOT NULL,schedule TEXT NOT NULL,agent TEXT NOT NULL,inbox TEXT NOT NULL,start_sequence INTEGER,end_sequence INTEGER,opened TEXT NOT NULL,closed TEXT NOT NULL,UNIQUE(schedule,opened));
      CREATE TABLE IF NOT EXISTS automation_audit(id TEXT PRIMARY KEY,tenant TEXT NOT NULL,project TEXT NOT NULL,action TEXT NOT NULL,object_id TEXT NOT NULL,at TEXT NOT NULL);").map_err(storage_error)?;
    c.execute_batch("CREATE TABLE IF NOT EXISTS source_captures(sequence INTEGER PRIMARY KEY AUTOINCREMENT,tenant TEXT NOT NULL,project TEXT NOT NULL,source TEXT NOT NULL REFERENCES observation_sources(id),data TEXT NOT NULL);").map_err(storage_error)?;
    let projects = {
        let mut s = c.prepare("SELECT id,tenant_id,main_agent_id,created_at FROM projects WHERE main_agent_id!='' AND NOT EXISTS(SELECT 1 FROM project_agents a WHERE a.project=projects.id)").map_err(storage_error)?;
        s.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(storage_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_error)?
    };
    for (project, tenant, id, created) in projects {
        let agent = ProjectAgent {
            id: id.clone(),
            project_id: project.clone(),
            name: "main".into(),
            state: "active".into(),
            is_default: true,
            capabilities: None,
            created_at: DateTime::parse_from_rfc3339(&created)
                .map_err(storage_error)?
                .with_timezone(&Utc),
            updated_at: Utc::now(),
        };
        c.execute(
            "INSERT INTO project_agents VALUES(?,?,?,?,?)",
            params![
                id,
                tenant,
                project,
                serde_json::to_string(&agent).map_err(storage_error)?,
                "active"
            ],
        )
        .map_err(storage_error)?;
    }
    // Pin legacy schedules at migration, before an owner can replace the Agent.
    let legacy = {
        let mut s=c.prepare("SELECT s.id,s.metadata_json,p.main_agent_id FROM project_schedules s JOIN projects p ON p.id=s.project_id").map_err(storage_error)?;
        s.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(storage_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_error)?
    };
    for (id, data, agent) in legacy {
        let mut schedule: ProjectSchedule = decode(data)?;
        if schedule.agent_id.is_none() {
            schedule.agent_id = Some(agent);
            c.execute(
                "UPDATE project_schedules SET metadata_json=? WHERE id=?",
                params![encode(&schedule)?, id],
            )
            .map_err(storage_error)?;
        }
    }
    Ok(())
}
fn encode<T: serde::Serialize>(v: &T) -> PlatformResult<String> {
    serde_json::to_string(v).map_err(storage_error)
}
fn decode<T: serde::de::DeserializeOwned>(s: String) -> PlatformResult<T> {
    serde_json::from_str(&s).map_err(storage_error)
}
fn audit(
    c: &Connection,
    p: &Principal,
    project: &str,
    action: &str,
    id: &str,
) -> PlatformResult<()> {
    c.execute(
        "INSERT INTO automation_audit VALUES(?,?,?,?,?,?)",
        params![
            opaque_id("audit"),
            p.tenant_id,
            project,
            action,
            id,
            Utc::now().to_rfc3339()
        ],
    )
    .map_err(storage_error)?;
    Ok(())
}
pub(super) fn live_agent(
    c: &Connection,
    tenant: &str,
    project: &str,
) -> PlatformResult<ProjectAgent> {
    let data = c
        .query_row(
            "SELECT data FROM project_agents WHERE tenant=? AND project=? AND state!='archived'",
            params![tenant, project],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_error)?
        .ok_or_else(|| {
            PlatformError::Conflict("project has no Agent; create one through the owner CLI".into())
        })?;
    decode(data)
}
pub(super) fn check_active(
    c: &Connection,
    tenant: &str,
    project: &str,
) -> PlatformResult<ProjectAgent> {
    let a = live_agent(c, tenant, project)?;
    if a.state != "active" {
        return Err(PlatformError::Conflict("Agent disabled".into()));
    }
    Ok(a)
}

impl SqliteRepository {
    pub fn agents(&self, p: &Principal, project: &str) -> PlatformResult<Vec<ProjectAgent>> {
        self.get_project(p, project)?;
        let c = self.connection()?;
        let mut s = c
            .prepare("SELECT data FROM project_agents WHERE tenant=? AND project=? ORDER BY rowid")
            .map_err(storage_error)?;
        s.query_map(params![p.tenant_id, project], |r| r.get::<_, String>(0))
            .map_err(storage_error)?
            .map(|r| decode(r.map_err(storage_error)?))
            .collect()
    }
    pub fn resolve_agent(
        &self,
        p: &Principal,
        project: &str,
        id: Option<&str>,
    ) -> PlatformResult<ProjectAgent> {
        self.get_project(p, project)?;
        let agent = check_active(&*self.connection()?, &p.tenant_id, project)?;
        if id.is_some_and(|id| id != agent.id) || (id.is_none() && !agent.is_default) {
            return Err(PlatformError::Conflict(
                "explicit active Agent required; no matching default route".into(),
            ));
        }
        Ok(agent)
    }
    pub fn save_agent(
        &self,
        p: &Principal,
        project: &str,
        id: Option<&str>,
        input: AgentInput,
    ) -> PlatformResult<ProjectAgent> {
        self.get_project(p, project)?;
        if input.name.trim().is_empty()
            || input.name.len() > 200
            || !matches!(input.state.as_str(), "active" | "disabled" | "archived")
            || input.capabilities.len() > 64
        {
            return Err(PlatformError::InvalidInput(
                "invalid Agent name/state/capabilities".into(),
            ));
        }
        let mut c = self.connection()?;
        let t = c.transaction().map_err(storage_error)?;
        let previous: Option<ProjectAgent> = if let Some(id) = id {
            Some(decode(
                t.query_row(
                    "SELECT data FROM project_agents WHERE tenant=? AND project=? AND id=?",
                    params![p.tenant_id, project, id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(storage_error)?
                .ok_or(PlatformError::NotFound)?,
            )?)
        } else {
            None
        };
        if previous.as_ref().is_some_and(|a| a.state == "archived") {
            return Err(PlatformError::Conflict(
                "archived identities cannot be reactivated".into(),
            ));
        }
        let active: bool=t.query_row("SELECT EXISTS(SELECT 1 FROM agent_runs WHERE tenant_id=? AND project_id=? AND status IN ('queued','running','waiting_approval'))",params![p.tenant_id,project],|r| r.get(0)).map_err(storage_error)?;
        let reduction = previous.as_ref().is_some_and(|a| {
            input.name.trim() == a.name
                && (!input.is_default || a.is_default)
                && (input.state == a.state || input.state == "disabled")
                && a.capabilities
                    .as_ref()
                    .is_none_or(|old| input.capabilities.iter().all(|cap| old.contains(cap)))
        });
        if active && !reduction {
            return Err(PlatformError::Conflict("finish the active Run before expanding permissions or changing Agent identity; revocation/disable remains allowed".into()));
        }
        let unresolved: bool=t.query_row("SELECT EXISTS(SELECT 1 FROM agent_inbox WHERE tenant=? AND project=? AND state IN ('dispatching','unknown'))",params![p.tenant_id,project],|r| r.get(0)).map_err(storage_error)?;
        if unresolved && !reduction {
            return Err(PlatformError::Conflict(
                "resolve unknown/dispatching deliveries before changing Agent".into(),
            ));
        }
        let now = Utc::now();
        let agent = ProjectAgent {
            id: previous
                .as_ref()
                .map(|a| a.id.clone())
                .unwrap_or_else(|| opaque_id("agt")),
            project_id: project.into(),
            name: input.name.trim().into(),
            state: input.state,
            is_default: input.is_default,
            capabilities: Some(input.capabilities),
            created_at: previous.map(|a| a.created_at).unwrap_or(now),
            updated_at: now,
        };
        t.execute("INSERT INTO project_agents VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data,state=excluded.state",params![agent.id,p.tenant_id,project,encode(&agent)?,agent.state]).map_err(map_constraint_error)?;
        t.execute(
            "UPDATE projects SET main_agent_id=?,updated_at=? WHERE id=? AND tenant_id=?",
            params![
                if agent.state == "archived" {
                    ""
                } else {
                    &agent.id
                },
                now.to_rfc3339(),
                project,
                p.tenant_id
            ],
        )
        .map_err(storage_error)?;
        if agent.state == "archived" {
            t.execute("UPDATE project_tasks SET status='cancelled',updated_at=? WHERE tenant_id=? AND project_id=? AND status='queued'",params![now.to_rfc3339(),p.tenant_id,project]).map_err(storage_error)?;
            let records = read_inbox(&t, p, project)?;
            for mut message in records
                .into_iter()
                .filter(|m| m.agent_id == agent.id && m.state == "pending")
            {
                message.state = "cancelled".into();
                message.updated_at = now;
                save_inbox(&t, p, &message)?;
            }
        }
        audit(&t, p, project, "agent_saved", &agent.id)?;
        t.commit().map_err(storage_error)?;
        Ok(agent)
    }
    pub fn sources(&self, p: &Principal, project: &str) -> PlatformResult<Vec<ObservationSource>> {
        self.get_project(p, project)?;
        let c = self.connection()?;
        let mut s=c.prepare("SELECT data FROM observation_sources WHERE tenant=? AND owner=? AND project=? ORDER BY rowid").map_err(storage_error)?;
        s.query_map(params![p.tenant_id, p.user_id, project], |r| {
            r.get::<_, String>(0)
        })
        .map_err(storage_error)?
        .map(|r| decode(r.map_err(storage_error)?))
        .collect()
    }
    pub fn source(
        &self,
        p: &Principal,
        project: &str,
        id: &str,
    ) -> PlatformResult<ObservationSource> {
        self.sources(p, project)?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or(PlatformError::NotFound)
    }
    pub fn create_source(
        &self,
        p: &Principal,
        project: &str,
        input: SourceInput,
    ) -> PlatformResult<ObservationSource> {
        let a = self.resolve_agent(p, project, input.agent_id.as_deref())?;
        if input.name.trim().is_empty()
            || input.name.len() > 200
            || !matches!(
                input.kind.as_str(),
                "manual" | "dws_messages" | "dws_events"
            )
            || !(60..=86400).contains(&input.poll_seconds)
            || encode(&input.arguments)?.len() > 8192
        {
            return Err(PlatformError::InvalidInput(
                "invalid source definition".into(),
            ));
        }
        if input.kind.starts_with("dws_") && input.connection_id.is_none() {
            return Err(PlatformError::InvalidInput(
                "DWS source requires a pinned connection_id".into(),
            ));
        }
        if input.kind == "manual"
            && (input.connection_id.is_some()
                || (!input.arguments.is_null() && input.arguments != serde_json::json!({})))
        {
            return Err(PlatformError::InvalidInput(
                "manual source has no connection/arguments".into(),
            ));
        }
        let now = Utc::now();
        let source = ObservationSource {
            id: opaque_id("src"),
            project_id: project.into(),
            agent_id: a.id,
            name: input.name.trim().into(),
            kind: input.kind,
            connection_id: input.connection_id,
            arguments: input.arguments,
            enabled: input.enabled,
            poll_seconds: input.poll_seconds,
            cursor: input.start.unwrap_or(now),
            next_poll_at: now,
            coverage: "unknown".into(),
            listener_state: "stopped".into(),
            listener_until: None,
            created_at: now,
            updated_at: now,
        };
        self.connection()?
            .execute(
                "INSERT INTO observation_sources VALUES(?,?,?,?,?,?)",
                params![
                    source.id,
                    p.tenant_id,
                    p.user_id,
                    project,
                    source.agent_id,
                    encode(&source)?
                ],
            )
            .map_err(storage_error)?;
        audit(
            &*self.connection()?,
            p,
            project,
            "source_created",
            &source.id,
        )?;
        Ok(source)
    }
    pub fn save_source(&self, p: &Principal, source: &ObservationSource) -> PlatformResult<()> {
        self.save_source_state(p, source, false)
    }
    pub fn save_source_enabled(
        &self,
        p: &Principal,
        source: &ObservationSource,
    ) -> PlatformResult<()> {
        self.save_source_state(p, source, true)
    }
    fn save_source_state(
        &self,
        p: &Principal,
        source: &ObservationSource,
        toggle: bool,
    ) -> PlatformResult<()> {
        self.source(p, &source.project_id, &source.id)?;
        let mut c = self.connection()?;
        let t = c.transaction().map_err(storage_error)?;
        let previous:ObservationSource=decode(t.query_row("SELECT data FROM observation_sources WHERE id=? AND tenant=? AND owner=? AND project=?",params![source.id,p.tenant_id,p.user_id,source.project_id],|r|r.get::<_,String>(0)).map_err(storage_error)?)?;
        if previous.agent_id != source.agent_id
            || previous.kind != source.kind
            || previous.connection_id != source.connection_id
            || previous.arguments != source.arguments
            || previous.created_at != source.created_at
        {
            return Err(PlatformError::Conflict(
                "source registration is immutable; create a new source partition".into(),
            ));
        }
        let mut next = source.clone();
        if !toggle {
            next.enabled = previous.enabled;
        }
        t.execute("UPDATE observation_sources SET data=? WHERE id=? AND tenant=? AND owner=? AND project=?",params![encode(&next)?,source.id,p.tenant_id,p.user_id,source.project_id]).map_err(storage_error)?;
        if toggle {
            audit(
                &t,
                p,
                &source.project_id,
                if next.enabled {
                    "source_enabled"
                } else {
                    "source_paused"
                },
                &source.id,
            )?;
        }
        t.commit().map_err(storage_error)?;
        Ok(())
    }
    pub fn record_observations(
        &self,
        p: &Principal,
        project: &str,
        id: &str,
        events: Vec<ObservationInput>,
    ) -> PlatformResult<usize> {
        self.source(p, project, id)?;
        if events.len() > 1000 {
            return Err(PlatformError::InvalidInput(
                "at most 1000 events per ingest".into(),
            ));
        }
        let mut c = self.connection()?;
        let t = c.transaction().map_err(storage_error)?;
        let source: ObservationSource = decode(
            t.query_row(
                "SELECT data FROM observation_sources WHERE id=? AND tenant=? AND project=?",
                params![id, p.tenant_id, project],
                |r| r.get::<_, String>(0),
            )
            .map_err(storage_error)?,
        )?;
        if source.kind.starts_with("dws_") && !source.enabled {
            return Err(PlatformError::Conflict(
                "source was paused before persistence".into(),
            ));
        }
        let mut count = 0;
        for mut e in events {
            if e.event_id.is_empty() || e.event_id.len() > 512 || encode(&e.body)?.len() > 65536 {
                return Err(PlatformError::InvalidInput(
                    "invalid observation size or event_id".into(),
                ));
            }
            crate::external_access::scrub_auth_fields(&mut e.body);
            count+=t.execute("INSERT OR IGNORE INTO observations(tenant,project,source,event,occurred,observed,body) VALUES(?,?,?,?,?,?,?)",params![p.tenant_id,project,id,e.event_id,e.occurred_at.to_rfc3339(),Utc::now().to_rfc3339(),encode(&e.body)?]).map_err(storage_error)?;
        }
        if count > 0 {
            audit(&t, p, project, "observations_ingested", id)?;
        }
        t.commit().map_err(storage_error)?;
        Ok(count)
    }
    pub fn observations(
        &self,
        p: &Principal,
        project: &str,
        id: &str,
        after: i64,
        limit: i64,
    ) -> PlatformResult<Vec<Observation>> {
        self.source(p, project, id)?;
        if after < 0 || !(1..=200).contains(&limit) {
            return Err(PlatformError::InvalidInput(
                "after >= 0, limit 1..200".into(),
            ));
        }
        let c = self.connection()?;
        let mut s=c.prepare("SELECT sequence,event,occurred,observed,body FROM observations WHERE tenant=? AND project=? AND source=? AND sequence>? ORDER BY sequence LIMIT ?").map_err(storage_error)?;
        s.query_map(params![p.tenant_id, project, id, after, limit], |r| {
            Ok(Observation {
                sequence: r.get(0)?,
                source_id: id.into(),
                event_id: r.get(1)?,
                occurred_at: parse_time(r.get(2)?)?,
                observed_at: parse_time(r.get(3)?)?,
                body: serde_json::from_str(&r.get::<_, String>(4)?).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
            })
        })
        .map_err(storage_error)?
        .map(|r| r.map_err(storage_error))
        .collect()
    }
    pub fn enqueue_input(
        &self,
        p: &Principal,
        project: &str,
        input: InboxInput,
    ) -> PlatformResult<InboxMessage> {
        let a = self.resolve_agent(p, project, input.agent_id.as_deref())?;
        if input.content.trim().is_empty()
            || input.content.len() > 32768
            || !matches!(input.delivery.as_str(), "queue" | "steer_if_active")
        {
            return Err(PlatformError::InvalidInput(
                "invalid content/delivery".into(),
            ));
        }
        let now = Utc::now();
        let message = InboxMessage {
            id: opaque_id("inbox"),
            project_id: project.into(),
            agent_id: a.id,
            schedule_id: None,
            prompt: input.content,
            delivery: input.delivery,
            state: "pending".into(),
            run_id: None,
            created_at: now,
            updated_at: now,
        };
        save_inbox(&*self.connection()?, p, &message)?;
        Ok(message)
    }
    pub fn inbox(&self, p: &Principal, project: &str) -> PlatformResult<Vec<InboxMessage>> {
        self.get_project(p, project)?;
        read_inbox(&*self.connection()?, p, project)
    }
    pub fn claim_input(&self, p: &Principal, message: &InboxMessage) -> PlatformResult<bool> {
        let mut c = self.connection()?;
        let t = c.transaction().map_err(storage_error)?;
        let a = check_active(&t, &p.tenant_id, &message.project_id)?;
        if a.id != message.agent_id {
            return Ok(false);
        }
        let current: Option<String> = t
            .query_row(
                "SELECT state FROM agent_inbox WHERE tenant=? AND project=? AND id=?",
                params![p.tenant_id, message.project_id, message.id],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        if current.as_deref() != Some("pending") {
            return Ok(false);
        }
        let mut claimed = message.clone();
        claimed.state = "dispatching".into();
        claimed.updated_at = Utc::now();
        save_inbox(&t, p, &claimed)?;
        audit(&t, p, &message.project_id, "dispatching", &message.id)?;
        t.commit().map_err(storage_error)?;
        Ok(true)
    }
    pub fn settle_input(
        &self,
        p: &Principal,
        message: &InboxMessage,
        state: &str,
        run: Option<String>,
    ) -> PlatformResult<()> {
        let mut next = message.clone();
        next.state = state.into();
        next.run_id = run;
        next.updated_at = Utc::now();
        let c = self.connection()?;
        save_inbox(&c, p, &next)?;
        audit(&c, p, &message.project_id, state, &message.id)
    }
    pub fn check_delivery_run(
        &self,
        p: &Principal,
        m: &InboxMessage,
        run: &str,
    ) -> PlatformResult<()> {
        self.get_project(p, &m.project_id)?;
        let c = self.connection()?;
        let matches:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM agent_runs r JOIN conversations c ON c.id=r.conversation_id WHERE r.tenant_id=? AND r.project_id=? AND r.id=? AND c.main_agent_id=?)",params![p.tenant_id,m.project_id,run,m.agent_id],|r|r.get(0)).map_err(storage_error)?;
        if !matches {
            return Err(PlatformError::Conflict(
                "Run is not owned by this delivery's Agent".into(),
            ));
        }
        Ok(())
    }
    pub fn recover_inbox(&self) -> PlatformResult<()> {
        let c = self.connection()?;
        let mut s = c
            .prepare("SELECT owner,tenant,data FROM agent_inbox WHERE state='dispatching'")
            .map_err(storage_error)?;
        let rows = s
            .query_map([], |r| {
                Ok((
                    Principal {
                        user_id: r.get(0)?,
                        tenant_id: r.get(1)?,
                    },
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        drop(s);
        for (p, data) in rows {
            let mut m: InboxMessage = decode(data)?;
            m.state = "unknown".into();
            m.updated_at = Utc::now();
            save_inbox(&c, &p, &m)?;
            audit(&c, &p, &m.project_id, "restart_unknown", &m.id)?;
        }
        Ok(())
    }
    pub fn recover_listeners(&self) -> PlatformResult<()> {
        let c = self.connection()?;
        let rows = {
            let mut s = c
                .prepare("SELECT id,data FROM observation_sources")
                .map_err(storage_error)?;
            s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(storage_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_error)?
        };
        for (id, data) in rows {
            let mut source: ObservationSource = decode(data)?;
            if matches!(source.listener_state.as_str(), "starting" | "live") {
                source.listener_state = "interrupted".into();
                source.coverage = "gap".into();
                c.execute(
                    "UPDATE observation_sources SET data=? WHERE id=?",
                    params![encode(&source)?, id],
                )
                .map_err(storage_error)?;
            }
        }
        Ok(())
    }
    pub fn record_capture(
        &self,
        p: &Principal,
        source: &ObservationSource,
        data: Value,
    ) -> PlatformResult<()> {
        self.source(p, &source.project_id, &source.id)?;
        self.connection()?
            .execute(
                "INSERT INTO source_captures(tenant,project,source,data) VALUES(?,?,?,?)",
                params![p.tenant_id, source.project_id, source.id, encode(&data)?],
            )
            .map_err(storage_error)?;
        Ok(())
    }
    pub fn captures(
        &self,
        p: &Principal,
        project: &str,
        id: &str,
        after: i64,
        limit: i64,
    ) -> PlatformResult<Vec<Value>> {
        self.source(p, project, id)?;
        if after < 0 || !(1..=200).contains(&limit) {
            return Err(PlatformError::InvalidInput(
                "after >= 0, limit 1..200".into(),
            ));
        }
        let c = self.connection()?;
        let mut s=c.prepare("SELECT sequence,data FROM source_captures WHERE tenant=? AND project=? AND source=? AND sequence>? ORDER BY sequence LIMIT ?").map_err(storage_error)?;
        s.query_map(params![p.tenant_id, project, id, after, limit], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(storage_error)?
        .map(|r| {
            let (sequence, data) = r.map_err(storage_error)?;
            let mut data: Value = decode(data)?;
            data["sequence"] = json!(sequence);
            Ok(data)
        })
        .collect()
    }
    pub fn occurrences(
        &self,
        p: &Principal,
        project: &str,
        schedule: &str,
    ) -> PlatformResult<Vec<Value>> {
        self.get_project(p, project)?;
        let c = self.connection()?;
        let mut s=c.prepare("SELECT id,agent,inbox,start_sequence,end_sequence,opened,closed FROM schedule_occurrences WHERE tenant=? AND project=? AND schedule=? ORDER BY rowid").map_err(storage_error)?;
        s.query_map(params![p.tenant_id,project,schedule],|r|Ok(serde_json::json!({"id":r.get::<_,String>(0)?,"agent_id":r.get::<_,String>(1)?,"inbox_id":r.get::<_,String>(2)?,"start_sequence":r.get::<_,Option<i64>>(3)?,"end_sequence":r.get::<_,Option<i64>>(4)?,"opened_at":r.get::<_,String>(5)?,"closed_at":r.get::<_,String>(6)?}))).map_err(storage_error)?.map(|r|r.map_err(storage_error)).collect()
    }
}
fn read_inbox(c: &Connection, p: &Principal, project: &str) -> PlatformResult<Vec<InboxMessage>> {
    let mut s = c
        .prepare(
            "SELECT data FROM agent_inbox WHERE tenant=? AND owner=? AND project=? ORDER BY rowid",
        )
        .map_err(storage_error)?;
    s.query_map(params![p.tenant_id, p.user_id, project], |r| {
        r.get::<_, String>(0)
    })
    .map_err(storage_error)?
    .map(|r| decode(r.map_err(storage_error)?))
    .collect()
}
impl SqliteRepository {
    pub fn schedule_window(
        &self,
        p: &Principal,
        project: &str,
        schedule: &str,
    ) -> PlatformResult<Option<Value>> {
        self.get_project(p, project)?;
        self.connection()?.query_row("SELECT opened,closes,start_sequence,end_sequence FROM schedule_windows WHERE tenant=? AND project=? AND schedule=?",params![p.tenant_id,project,schedule],|r|Ok(json!({"opened_at":r.get::<_,String>(0)?,"closes_at":r.get::<_,String>(1)?,"start_sequence":r.get::<_,i64>(2)?,"end_sequence":r.get::<_,i64>(3)?}))).optional().map_err(storage_error)
    }
}
pub(super) fn save_inbox(c: &Connection, p: &Principal, m: &InboxMessage) -> PlatformResult<()> {
    c.execute("INSERT INTO agent_inbox VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET state=excluded.state,data=excluded.data",params![m.id,p.tenant_id,p.user_id,m.project_id,m.agent_id,m.schedule_id,m.state,encode(m)?]).map_err(storage_error)?;
    Ok(())
}
