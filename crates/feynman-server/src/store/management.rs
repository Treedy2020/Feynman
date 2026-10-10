use super::*;
use feynman_core::{
    CreateScheduleRequest, CreateTaskRequest, ProjectSchedule, ProjectTask, ProjectWorkspace,
};

pub(super) fn migrate(connection: &Connection) -> PlatformResult<()> {
    connection
        .execute_batch(
            "
        CREATE TABLE IF NOT EXISTS project_workspaces (
            tenant_id TEXT NOT NULL,
            project_id TEXT NOT NULL REFERENCES projects(id),
            metadata_json TEXT NOT NULL,
            PRIMARY KEY (tenant_id, project_id)
        );
        CREATE TABLE IF NOT EXISTS project_tasks (
            id TEXT PRIMARY KEY,
            tenant_id TEXT NOT NULL,
            project_id TEXT NOT NULL REFERENCES projects(id),
            user_id TEXT NOT NULL,
            title TEXT NOT NULL,
            prompt TEXT NOT NULL,
            status TEXT NOT NULL,
            schedule_id TEXT,
            occurrence TEXT,
            run_id TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(schedule_id, occurrence)
        );
        CREATE INDEX IF NOT EXISTS queued_tasks ON project_tasks(tenant_id, project_id, status);
        CREATE TABLE IF NOT EXISTS project_schedules (
            id TEXT PRIMARY KEY,
            tenant_id TEXT NOT NULL,
            project_id TEXT NOT NULL REFERENCES projects(id),
            user_id TEXT NOT NULL,
            metadata_json TEXT NOT NULL
        );
    ",
        )
        .map_err(storage_error)
}

fn validate_work(title: &str, prompt: &str) -> PlatformResult<()> {
    if title.trim().is_empty()
        || title.len() > 200
        || prompt.trim().is_empty()
        || prompt.len() > 32_768
    {
        return Err(PlatformError::InvalidInput(
            "title must be 1..200 bytes and prompt 1..32768 bytes".into(),
        ));
    }
    Ok(())
}

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<ProjectTask> {
    Ok(ProjectTask {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        prompt: row.get(3)?,
        status: row.get(4)?,
        schedule_id: row.get(5)?,
        run_id: row.get(6)?,
        created_at: parse_time(row.get(7)?)?,
        updated_at: parse_time(row.get(8)?)?,
    })
}
const TASK_COLUMNS: &str =
    "id, project_id, title, prompt, status, schedule_id, run_id, created_at, updated_at";

impl SqliteRepository {
    pub fn workspace(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<Option<ProjectWorkspace>> {
        self.get_project(principal, project_id)?;
        let json: Option<String> = self.connection()?.query_row(
            "SELECT metadata_json FROM project_workspaces WHERE tenant_id = ? AND project_id = ?",
            params![principal.tenant_id, project_id], |row| row.get(0),
        ).optional().map_err(storage_error)?;
        json.map(|value| serde_json::from_str(&value).map_err(storage_error))
            .transpose()
    }

    pub fn save_workspace(
        &self,
        principal: &Principal,
        workspace: &ProjectWorkspace,
    ) -> PlatformResult<()> {
        self.get_project(principal, &workspace.project_id)?;
        self.connection()?.execute(
            "INSERT INTO project_workspaces VALUES (?, ?, ?) ON CONFLICT(tenant_id, project_id) DO UPDATE SET metadata_json = excluded.metadata_json",
            params![principal.tenant_id, workspace.project_id, serde_json::to_string(workspace).map_err(storage_error)?],
        ).map_err(storage_error)?;
        Ok(())
    }

    pub fn create_task(
        &self,
        principal: &Principal,
        project_id: &str,
        request: CreateTaskRequest,
    ) -> PlatformResult<ProjectTask> {
        self.get_project(principal, project_id)?;
        validate_work(&request.title, &request.prompt)?;
        let now = Utc::now();
        let task = ProjectTask {
            id: opaque_id("task"),
            project_id: project_id.into(),
            title: request.title.trim().into(),
            prompt: request.prompt.trim().into(),
            status: "queued".into(),
            schedule_id: None,
            run_id: None,
            created_at: now,
            updated_at: now,
        };
        insert_task(&*self.connection()?, principal, &task, None)?;
        Ok(task)
    }

    pub fn list_tasks(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<Vec<ProjectTask>> {
        self.get_project(principal, project_id)?;
        let connection = self.connection()?;
        let mut statement = connection.prepare(&format!("SELECT {TASK_COLUMNS} FROM project_tasks WHERE tenant_id = ? AND project_id = ? ORDER BY created_at")).map_err(storage_error)?;
        statement
            .query_map(params![principal.tenant_id, project_id], task_from_row)
            .map_err(storage_error)?
            .map(|row| row.map_err(storage_error))
            .collect()
    }

    pub fn task(
        &self,
        principal: &Principal,
        project_id: &str,
        id: &str,
    ) -> PlatformResult<ProjectTask> {
        self.get_project(principal, project_id)?;
        self.connection()?.query_row(&format!("SELECT {TASK_COLUMNS} FROM project_tasks WHERE tenant_id = ? AND project_id = ? AND id = ?"),
            params![principal.tenant_id, project_id, id], task_from_row).optional().map_err(storage_error)?.ok_or(PlatformError::NotFound)
    }

    pub fn cancel_task(
        &self,
        principal: &Principal,
        project_id: &str,
        id: &str,
    ) -> PlatformResult<ProjectTask> {
        self.task(principal, project_id, id)?;
        let changed = self.connection()?.execute(
            "UPDATE project_tasks SET status = 'cancelled', updated_at = ? WHERE tenant_id = ? AND project_id = ? AND id = ? AND status = 'queued'",
            params![Utc::now().to_rfc3339(), principal.tenant_id, project_id, id],
        ).map_err(storage_error)?;
        if changed == 0 {
            return Err(PlatformError::Conflict(
                "only queued tasks can be cancelled; running work is not cancelled implicitly"
                    .into(),
            ));
        }
        self.task(principal, project_id, id)
    }

    pub fn queued_tasks(&self) -> PlatformResult<Vec<(Principal, ProjectTask)>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(&format!("SELECT {TASK_COLUMNS}, tenant_id, user_id FROM project_tasks t
            WHERE t.rowid IN (SELECT MIN(q.rowid) FROM project_tasks q WHERE q.status = 'queued' GROUP BY q.tenant_id, q.project_id)
              AND NOT EXISTS (SELECT 1 FROM agent_runs r WHERE r.tenant_id = t.tenant_id AND r.project_id = t.project_id AND r.status IN ('queued', 'running', 'waiting_approval'))
            ORDER BY created_at LIMIT 64")).map_err(storage_error)?;
        statement
            .query_map([], |row| {
                Ok((
                    Principal {
                        tenant_id: row.get(9)?,
                        user_id: row.get(10)?,
                    },
                    task_from_row(row)?,
                ))
            })
            .map_err(storage_error)?
            .map(|row| row.map_err(storage_error))
            .collect()
    }

    pub fn create_schedule(
        &self,
        principal: &Principal,
        project_id: &str,
        request: CreateScheduleRequest,
    ) -> PlatformResult<ProjectSchedule> {
        self.get_project(principal, project_id)?;
        validate_work(&request.title, &request.prompt)?;
        let agent = self.resolve_agent(principal, project_id, request.agent_id.as_deref())?;
        if !matches!(request.delivery.as_str(), "queue" | "steer_if_active") {
            return Err(PlatformError::InvalidInput(
                "invalid delivery strategy".into(),
            ));
        }
        if request.source_ids.len() > 32
            || request
                .source_ids
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != request.source_ids.len()
        {
            return Err(PlatformError::InvalidInput(
                "source_ids must be unique, at most 32".into(),
            ));
        }
        for id in &request.source_ids {
            if self.source(principal, project_id, id)?.agent_id != agent.id {
                return Err(PlatformError::Conflict(
                    "source belongs to another Agent".into(),
                ));
            }
        }
        if !request.source_ids.is_empty()
            && (request.interval_seconds.is_some()
                || request.at.is_some()
                || !request
                    .collect_for_seconds
                    .is_some_and(|n| (1..=86400).contains(&n)))
        {
            return Err(PlatformError::InvalidInput(
                "event schedule needs collect_for_seconds 1..86400 and no clock trigger".into(),
            ));
        }
        if request.source_ids.is_empty() && request.collect_for_seconds.is_some() {
            return Err(PlatformError::InvalidInput(
                "collect_for_seconds requires sources".into(),
            ));
        }
        if request.interval_seconds.is_none()
            && request.at.is_none()
            && request.source_ids.is_empty()
        {
            return Err(PlatformError::InvalidInput(
                "provide interval_seconds or an RFC3339 at time".into(),
            ));
        }
        if request
            .interval_seconds
            .is_some_and(|value| !(60..=31_536_000).contains(&value))
        {
            return Err(PlatformError::InvalidInput(
                "interval_seconds must be 60..31536000".into(),
            ));
        }
        let now = Utc::now();
        let schedule = ProjectSchedule {
            id: opaque_id("sched"),
            project_id: project_id.into(),
            title: request.title.trim().into(),
            prompt: request.prompt.trim().into(),
            interval_seconds: request.interval_seconds,
            next_due_at: request.at.unwrap_or_else(|| {
                now + chrono::Duration::seconds(request.interval_seconds.unwrap_or(60))
            }),
            enabled: request.enabled,
            last_task_id: None,
            last_inbox_id: None,
            created_at: now,
            updated_at: now,
            agent_id: Some(agent.id),
            source_ids: request.source_ids,
            delivery: request.delivery,
            collect_for_seconds: request.collect_for_seconds,
            exhausted: false,
        };
        self.connection()?
            .execute(
                "INSERT INTO project_schedules VALUES (?, ?, ?, ?, ?)",
                params![
                    schedule.id,
                    principal.tenant_id,
                    project_id,
                    principal.user_id,
                    serde_json::to_string(&schedule).map_err(storage_error)?
                ],
            )
            .map_err(storage_error)?;
        Ok(schedule)
    }

    pub fn list_schedules(
        &self,
        principal: &Principal,
        project_id: &str,
    ) -> PlatformResult<Vec<ProjectSchedule>> {
        self.get_project(principal, project_id)?;
        let connection = self.connection()?;
        let mut statement = connection.prepare("SELECT metadata_json FROM project_schedules WHERE tenant_id = ? AND project_id = ? ORDER BY rowid").map_err(storage_error)?;
        statement
            .query_map(params![principal.tenant_id, project_id], |row| {
                row.get::<_, String>(0)
            })
            .map_err(storage_error)?
            .map(|row| serde_json::from_str(&row.map_err(storage_error)?).map_err(storage_error))
            .collect()
    }

    pub fn set_schedule_enabled(
        &self,
        principal: &Principal,
        project_id: &str,
        id: &str,
        enabled: bool,
    ) -> PlatformResult<ProjectSchedule> {
        let mut schedule = self
            .list_schedules(principal, project_id)?
            .into_iter()
            .find(|record| record.id == id)
            .ok_or(PlatformError::NotFound)?;
        schedule.enabled = enabled;
        if !enabled {
            schedule.exhausted = false;
        }
        if enabled {
            self.resolve_agent(principal, project_id, schedule.agent_id.as_deref())?;
        }
        schedule.updated_at = Utc::now();
        // Resuming an overdue schedule produces one occurrence, never a catch-up storm.
        self.connection()?.execute("UPDATE project_schedules SET metadata_json = ? WHERE tenant_id = ? AND project_id = ? AND id = ?",
            params![serde_json::to_string(&schedule).map_err(storage_error)?, principal.tenant_id, project_id, id]).map_err(storage_error)?;
        Ok(schedule)
    }
}

fn insert_task(
    connection: &Connection,
    principal: &Principal,
    task: &ProjectTask,
    occurrence: Option<&str>,
) -> PlatformResult<()> {
    connection
        .execute(
            "INSERT INTO project_tasks VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                task.id,
                principal.tenant_id,
                task.project_id,
                principal.user_id,
                task.title,
                task.prompt,
                task.status,
                task.schedule_id,
                occurrence,
                task.run_id,
                task.created_at.to_rfc3339(),
                task.updated_at.to_rfc3339()
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}
