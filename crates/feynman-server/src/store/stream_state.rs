//! Snapshot contents and replay cursors must refer to the same transaction.
use super::*;
pub(super) fn read_snapshot(
    c: &Connection,
    p: &Principal,
    id: &str,
) -> PlatformResult<ConversationSnapshot> {
    let conversation=c.query_row("SELECT id,tenant_id,project_id,main_agent_id,runtime_thread_id,created_at,updated_at FROM conversations WHERE tenant_id=? AND id=?",params![p.tenant_id,id],conversation_from_row).optional().map_err(storage_error)?.ok_or(PlatformError::NotFound)?;
    let mut q=c.prepare("SELECT id,tenant_id,conversation_id,run_id,role,content,status,created_at,completed_at FROM conversation_messages WHERE tenant_id=? AND conversation_id=? ORDER BY rowid").map_err(storage_error)?;
    let messages = q
        .query_map(params![p.tenant_id, id], message_from_row)
        .map_err(storage_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_error)?;
    let mut q=c.prepare("SELECT id,tenant_id,project_id,conversation_id,agent_session_id,user_message_id,assistant_message_id,status,runtime_turn_id,created_at,started_at,completed_at,error FROM agent_runs WHERE tenant_id=? AND conversation_id=? ORDER BY rowid").map_err(storage_error)?;
    let runs = q
        .query_map(params![p.tenant_id, id], run_from_row)
        .map_err(storage_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_error)?;
    let active_run = runs
        .iter()
        .rev()
        .find(|r| {
            matches!(
                r.status,
                AgentRunStatus::Queued | AgentRunStatus::Running | AgentRunStatus::WaitingApproval
            )
        })
        .cloned();
    Ok(ConversationSnapshot {
        conversation,
        messages,
        runs,
        active_run,
    })
}
pub(super) fn read_feed(
    c: &Connection,
    p: &Principal,
    project: &str,
    run: &str,
    after: i64,
) -> PlatformResult<RunFeed> {
    let record=c.query_row("SELECT id,tenant_id,project_id,conversation_id,agent_session_id,user_message_id,assistant_message_id,status,runtime_turn_id,created_at,started_at,completed_at,error FROM agent_runs WHERE tenant_id=? AND project_id=? AND id=?",params![p.tenant_id,project,run],run_from_row).optional().map_err(storage_error)?.ok_or(PlatformError::NotFound)?;
    let mut q=c.prepare("SELECT id,tenant_id,run_id,sequence,kind,payload_json,created_at FROM chat_events WHERE tenant_id=? AND run_id=? AND sequence>? ORDER BY sequence").map_err(storage_error)?;
    let events = q
        .query_map(params![p.tenant_id, run, after], event_from_row)
        .map_err(storage_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_error)?;
    Ok(RunFeed {
        run: record,
        events,
    })
}
pub(super) fn write_event(
    c: &Connection,
    p: &Principal,
    run: &str,
    kind: &str,
    payload: Value,
) -> PlatformResult<ChatEvent> {
    let exists: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_runs WHERE tenant_id=? AND id=?)",
            params![p.tenant_id, run],
            |r| r.get(0),
        )
        .map_err(storage_error)?;
    if !exists {
        return Err(PlatformError::NotFound);
    }
    let sequence = c
        .query_row(
            "SELECT COALESCE(MAX(sequence),0)+1 FROM chat_events WHERE run_id=?",
            [run],
            |r| r.get(0),
        )
        .map_err(storage_error)?;
    let event = ChatEvent {
        id: opaque_id("evt"),
        tenant_id: p.tenant_id.clone(),
        run_id: run.into(),
        sequence,
        kind: kind.into(),
        payload,
        created_at: Utc::now(),
    };
    c.execute("INSERT INTO chat_events (id,tenant_id,run_id,sequence,kind,payload_json,created_at) VALUES (?,?,?,?,?,?,?)",params![event.id,event.tenant_id,event.run_id,event.sequence,event.kind,serde_json::to_string(&event.payload).map_err(storage_error)?,event.created_at.to_rfc3339()]).map_err(storage_error)?;
    if matches!(
        kind,
        "run_completed" | "run_failed" | "run_interrupted" | "approval_requested"
    ) {
        c.execute(
            "INSERT INTO attention_events (id,tenant_id,run_id,kind,created_at) VALUES (?,?,?,?,?)",
            params![
                event.id,
                event.tenant_id,
                event.run_id,
                event.kind,
                event.created_at.to_rfc3339()
            ],
        )
        .map_err(storage_error)?;
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn concurrent_text_snapshots_match_their_event_cursors_and_attention_is_scoped() {
        let path =
            std::env::temp_dir().join(format!("feynman-stream-{}.sqlite3", uuid::Uuid::new_v4()));
        let repo = Arc::new(SqliteRepository::open(&path).unwrap());
        let p = Principal {
            tenant_id: "local".into(),
            user_id: "owner".into(),
        };
        let other = Principal {
            tenant_id: "other".into(),
            user_id: "other".into(),
        };
        let colleague = Principal {
            tenant_id: p.tenant_id.clone(),
            user_id: "second-viewer".into(),
        };
        let project = repo.create_project(&p, "Concurrent snapshots").unwrap();
        let session = repo
            .get_or_create_agent_session(&p, &project, "fixture")
            .unwrap();
        let c = repo.get_or_create_conversation(&p, &project).unwrap();
        let run = repo
            .submit_message(&p, &project, &c, &session, "Inspect only", None)
            .unwrap()
            .run;
        let writer = repo.clone();
        let principal = p.clone();
        let run_id = run.id.clone();
        let task = std::thread::spawn(move || {
            for _ in 0..100 {
                writer
                    .append_assistant_delta(&principal, &run_id, "x")
                    .unwrap();
                std::thread::yield_now();
            }
        });
        for _ in 0..100 {
            let (snapshot, feed) = repo.stream_snapshot(&p, &project.id).unwrap();
            let text = feed
                .unwrap()
                .events
                .iter()
                .map(|e| e.payload["text"].as_str().unwrap_or(""))
                .collect::<String>();
            assert_eq!(snapshot.messages.last().unwrap().content, text);
        }
        task.join().unwrap();
        let n = repo
            .append_run_event(
                &p,
                &run.id,
                "approval_requested",
                serde_json::json!({"requestId":"1"}),
            )
            .unwrap();
        assert_eq!(repo.notifications(&p, 0, 100).unwrap().items.len(), 1);
        assert!(repo.notifications(&other, 0, 100).unwrap().items.is_empty());
        assert!(matches!(
            repo.read_notification(&other, &n.id),
            Err(PlatformError::NotFound)
        ));
        repo.read_notification(&p, &n.id).unwrap();
        assert_eq!(repo.notifications(&p, 0, 100).unwrap().unread, 0);
        assert_eq!(repo.notifications(&colleague, 0, 100).unwrap().unread, 1);
        assert!(repo.stream_snapshot(&other, &project.id).is_err());
        drop(repo);
        let _ = std::fs::remove_file(path);
    }
}
