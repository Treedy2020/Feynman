use super::*;
use feynman_core::*;
use serde_json::json;
fn setup() -> (SqliteRepository, Principal, Project) {
    let r = SqliteRepository::open(":memory:").unwrap();
    let p = Principal {
        tenant_id: "local".into(),
        user_id: "local-user".into(),
    };
    let project = r.create_project(&p, "Automation").unwrap();
    (r, p, project)
}
fn source(r: &SqliteRepository, p: &Principal, project: &str) -> ObservationSource {
    r.create_source(
        p,
        project,
        SourceInput {
            agent_id: None,
            name: "Facts".into(),
            kind: "manual".into(),
            connection_id: None,
            arguments: Value::Null,
            enabled: true,
            poll_seconds: 60,
            start: None,
        },
    )
    .unwrap()
}
fn event(id: &str) -> ObservationInput {
    ObservationInput {
        event_id: id.into(),
        occurred_at: Utc::now(),
        body: json!({"fact":id,"access_token":"SECRET"}),
    }
}
#[test]
fn facts_are_isolated_by_registration_project_and_tenant_and_survive_reopen() {
    let path = std::env::temp_dir().join(format!("feynman-facts-{}.sqlite3", uuid::Uuid::new_v4()));
    let r = SqliteRepository::open(&path).unwrap();
    let p = Principal {
        tenant_id: "local".into(),
        user_id: "local-user".into(),
    };
    let project = r.create_project(&p, "Durable").unwrap();
    let a = source(&r, &p, &project.id);
    let b = source(&r, &p, &project.id);
    assert_ne!(a.id, b.id);
    assert_eq!(
        r.record_observations(&p, &project.id, &a.id, vec![event("same")])
            .unwrap(),
        1
    );
    assert_eq!(
        r.record_observations(&p, &project.id, &a.id, vec![event("same")])
            .unwrap(),
        0
    );
    assert_eq!(
        r.record_observations(&p, &project.id, &b.id, vec![event("same")])
            .unwrap(),
        1
    );
    let rows = r.observations(&p, &project.id, &a.id, 0, 100).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].body.get("access_token").is_none());
    assert!(
        r.observations(&p, &project.id, &a.id, rows[0].sequence, 100)
            .unwrap()
            .is_empty()
    );
    let other = r.create_project(&p, "Peer").unwrap();
    assert!(r.observations(&p, &other.id, &a.id, 0, 100).is_err());
    let outsider = Principal {
        tenant_id: "outsider".into(),
        user_id: "local-user".into(),
    };
    assert!(
        r.observations(&outsider, &project.id, &a.id, 0, 100)
            .is_err()
    );
    drop(r);
    let r = SqliteRepository::open(&path).unwrap();
    assert_eq!(
        r.observations(&p, &project.id, &a.id, 0, 100).unwrap()[0].event_id,
        "same"
    );
    let reader = SqliteRepository::open_existing(&path).unwrap();
    assert_eq!(
        reader
            .observations(&p, &project.id, &a.id, 0, 100)
            .unwrap()
            .len(),
        1
    );
    assert!(
        reader
            .record_observations(&p, &project.id, &a.id, vec![event("reader-write")])
            .is_err()
    );
    drop(reader);
    drop(r);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn agent_replacement_keeps_old_session_history_without_inheriting_permissions() {
    let (r, p, project) = setup();
    let old = r.agents(&p, &project.id).unwrap().remove(0);
    let old_session = r
        .get_or_create_agent_session(&p, &project, "codex")
        .unwrap();
    let old_conversation = r.get_or_create_conversation(&p, &project).unwrap();
    r.save_agent(
        &p,
        &project.id,
        Some(&old.id),
        AgentInput {
            name: "old".into(),
            state: "archived".into(),
            is_default: false,
            capabilities: vec![],
        },
    )
    .unwrap();
    assert!(r.check_agent_active(&p, &project.id).is_err());
    assert!(
        r.save_agent(
            &p,
            &project.id,
            Some(&old.id),
            AgentInput {
                name: "old".into(),
                state: "active".into(),
                is_default: true,
                capabilities: vec![]
            }
        )
        .is_err()
    );
    let new = r
        .save_agent(
            &p,
            &project.id,
            None,
            AgentInput {
                name: "new".into(),
                state: "active".into(),
                is_default: false,
                capabilities: vec![],
            },
        )
        .unwrap();
    assert_ne!(new.id, old.id);
    assert!(r.resolve_agent(&p, &project.id, None).is_err());
    assert!(r.resolve_agent(&p, &project.id, Some(&new.id)).is_ok());
    let project = r.get_project(&p, &project.id).unwrap();
    assert_ne!(
        r.get_or_create_agent_session(&p, &project, "codex")
            .unwrap()
            .id,
        old_session.id
    );
    assert_ne!(
        r.get_or_create_conversation(&p, &project).unwrap().id,
        old_conversation.id
    );
    r.grant_tool(&p, &project.id, "dws.aisearch.enterprise")
        .unwrap();
    assert!(
        r.check_agent_tool(&p, &project.id, "dws.aisearch.enterprise")
            .is_err()
    );
}
#[test]
fn fixed_whole_schedule_window_coalesces_and_freezes_after_claim() {
    let (r, p, project) = setup();
    let a = source(&r, &p, &project.id);
    let b = source(&r, &p, &project.id);
    let plan = r
        .create_schedule(
            &p,
            &project.id,
            CreateScheduleRequest {
                title: "Changes".into(),
                prompt: "Check evidence".into(),
                interval_seconds: None,
                at: None,
                enabled: true,
                agent_id: None,
                source_ids: vec![a.id.clone(), b.id.clone()],
                delivery: "queue".into(),
                collect_for_seconds: Some(30),
            },
        )
        .unwrap();
    r.record_observations(&p, &project.id, &a.id, vec![event("a1")])
        .unwrap();
    let now = Utc::now();
    assert_eq!(r.enqueue_schedule_inputs(now).unwrap(), 0);
    r.record_observations(&p, &project.id, &b.id, vec![event("b1")])
        .unwrap();
    assert_eq!(
        r.enqueue_schedule_inputs(now + chrono::Duration::seconds(29))
            .unwrap(),
        0
    );
    assert_eq!(
        r.enqueue_schedule_inputs(now + chrono::Duration::seconds(30))
            .unwrap(),
        1
    );
    let inbox = r.inbox(&p, &project.id).unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(r.occurrences(&p, &project.id, &plan.id).unwrap().len(), 1);
    assert!(r.claim_input(&p, &inbox[0]).unwrap());
    assert!(!r.claim_input(&p, &inbox[0]).unwrap());
    r.record_observations(&p, &project.id, &a.id, vec![event("a2")])
        .unwrap();
    r.enqueue_schedule_inputs(now + chrono::Duration::seconds(31))
        .unwrap();
    r.enqueue_schedule_inputs(now + chrono::Duration::seconds(61))
        .unwrap();
    assert_eq!(
        r.inbox(&p, &project.id).unwrap().len(),
        2,
        "dispatching input cannot be mutated"
    );
    r.recover_inbox().unwrap();
    assert_eq!(r.inbox(&p, &project.id).unwrap()[0].state, "unknown");
}
#[test]
fn timers_create_auditable_occurrences_but_only_one_pending_notice() {
    let (r, p, project) = setup();
    let now = Utc::now();
    let s = r
        .create_schedule(
            &p,
            &project.id,
            CreateScheduleRequest {
                title: "Clock".into(),
                prompt: "Inspect".into(),
                interval_seconds: Some(60),
                at: Some(now),
                enabled: true,
                agent_id: None,
                source_ids: vec![],
                delivery: "queue".into(),
                collect_for_seconds: None,
            },
        )
        .unwrap();
    assert_eq!(r.enqueue_schedule_inputs(now).unwrap(), 1);
    assert_eq!(r.enqueue_schedule_inputs(now).unwrap(), 0);
    assert_eq!(
        r.enqueue_schedule_inputs(now + chrono::Duration::hours(1))
            .unwrap(),
        1
    );
    assert_eq!(r.inbox(&p, &project.id).unwrap().len(), 1);
    assert_eq!(r.occurrences(&p, &project.id, &s.id).unwrap().len(), 2);
}

#[test]
fn background_capture_cannot_reenable_a_paused_source_or_mutate_its_registration() {
    let (r, p, project) = setup();
    let stale = source(&r, &p, &project.id);
    let mut paused = stale.clone();
    paused.enabled = false;
    r.save_source_enabled(&p, &paused).unwrap();
    r.save_source(&p, &stale).unwrap();
    assert!(!r.source(&p, &project.id, &stale.id).unwrap().enabled);
    let mut mutated = stale.clone();
    mutated.arguments = json!({"new":"scope"});
    assert!(r.save_source(&p, &mutated).is_err());
}

#[test]
fn listener_recovery_preserves_bounded_lease_and_requires_owner_resume() {
    let (r, p, project) = setup();
    let mut s = source(&r, &p, &project.id);
    s.listener_state = "live".into();
    s.listener_until = Some(Utc::now() + chrono::Duration::minutes(11));
    r.save_source(&p, &s).unwrap();
    r.recover_listeners().unwrap();
    let recovered = r.source(&p, &project.id, &s.id).unwrap();
    assert_eq!(recovered.listener_state, "interrupted");
    assert_eq!(recovered.coverage, "gap");
    assert_eq!(recovered.listener_until, s.listener_until);
    // Opening another repository/MCP does not perform server-owned listener recovery.
    automation::migrate(&r.connection().unwrap()).unwrap();
    assert_eq!(
        r.source(&p, &project.id, &s.id).unwrap().listener_state,
        "interrupted"
    );
}

#[test]
fn legacy_migration_preserves_runs_sessions_messages_and_pins_the_old_schedule_target() {
    let path = std::env::temp_dir().join(format!(
        "feynman-agent-migration-{}.sqlite3",
        uuid::Uuid::new_v4()
    ));
    let r = SqliteRepository::open(&path).unwrap();
    let p = Principal {
        tenant_id: "local".into(),
        user_id: "local-user".into(),
    };
    let project = r.create_project(&p, "Legacy").unwrap();
    let session = r
        .get_or_create_agent_session(&p, &project, "codex")
        .unwrap();
    let conversation = r.get_or_create_conversation(&p, &project).unwrap();
    let run = r
        .submit_message(
            &p,
            &project,
            &conversation,
            &session,
            "Preserved old message",
            None,
        )
        .unwrap()
        .run;
    let schedule = r
        .create_schedule(
            &p,
            &project.id,
            CreateScheduleRequest {
                title: "Old".into(),
                prompt: "Old target".into(),
                interval_seconds: Some(60),
                at: None,
                enabled: false,
                agent_id: None,
                source_ids: vec![],
                delivery: "queue".into(),
                collect_for_seconds: None,
            },
        )
        .unwrap();
    r.connection().unwrap().execute_batch("PRAGMA foreign_keys=OFF;
      CREATE TABLE sessions_legacy AS SELECT id,tenant_id,project_id,runtime,status,last_heartbeat_at,created_at,updated_at FROM agent_sessions;
      DROP TABLE agent_sessions;ALTER TABLE sessions_legacy RENAME TO agent_sessions;
      CREATE TABLE conversations_legacy AS SELECT * FROM conversations;
      DROP TABLE conversations;ALTER TABLE conversations_legacy RENAME TO conversations;
      DROP TABLE project_agents;
      UPDATE project_schedules SET metadata_json=json_remove(metadata_json,'$.agent_id');").unwrap();
    drop(r);
    let r = SqliteRepository::open(&path).unwrap();
    assert_eq!(
        r.get_or_create_agent_session(&p, &project, "codex")
            .unwrap()
            .id,
        session.id
    );
    assert_eq!(
        r.get_or_create_conversation(&p, &project).unwrap().id,
        conversation.id
    );
    assert_eq!(
        r.get_run_feed(&p, &project.id, &run.id, 0)
            .unwrap()
            .run
            .user_message_id,
        run.user_message_id
    );
    assert_eq!(
        r.conversation_snapshot(&p, &project.id).unwrap().messages[0].content,
        "Preserved old message"
    );
    r.recover_after_server_restart().unwrap();
    r.save_agent(
        &p,
        &project.id,
        Some(&project.main_agent_id),
        AgentInput {
            name: "main".into(),
            state: "archived".into(),
            is_default: false,
            capabilities: vec![],
        },
    )
    .unwrap();
    let replacement = r
        .save_agent(
            &p,
            &project.id,
            None,
            AgentInput {
                name: "new".into(),
                state: "active".into(),
                is_default: true,
                capabilities: vec![],
            },
        )
        .unwrap();
    assert_ne!(replacement.id, project.main_agent_id);
    assert_eq!(
        r.list_schedules(&p, &project.id).unwrap()[0]
            .agent_id
            .as_deref(),
        Some(project.main_agent_id.as_str())
    );
    assert!(
        r.set_schedule_enabled(&p, &project.id, &schedule.id, true)
            .is_err()
    );
    drop(r);
    std::fs::remove_file(path).unwrap();
}
