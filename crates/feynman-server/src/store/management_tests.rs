use super::*;
use feynman_core::{CreateScheduleRequest, CreateTaskRequest};

fn setup() -> (SqliteRepository, Principal, Project) {
    let repository = SqliteRepository::open(":memory:").unwrap();
    let principal = Principal {
        tenant_id: "local".into(),
        user_id: "local-user".into(),
    };
    let project = repository
        .create_project(&principal, "Manager test")
        .unwrap();
    (repository, principal, project)
}
fn task_request() -> CreateTaskRequest {
    CreateTaskRequest {
        title: "Implement".into(),
        prompt: "Fix and test".into(),
    }
}

#[test]
fn a_busy_projects_backlog_does_not_starve_another_project() {
    let (repository, principal, project) = setup();
    let peer = repository.create_project(&principal, "Peer").unwrap();
    for _ in 0..100 {
        repository
            .create_task(&principal, &project.id, task_request())
            .unwrap();
    }
    let peer_task = repository
        .create_task(&principal, &peer.id, task_request())
        .unwrap();
    let session = repository
        .get_or_create_agent_session(&principal, &project, "codex")
        .unwrap();
    let conversation = repository
        .get_or_create_conversation(&principal, &project)
        .unwrap();
    repository
        .submit_message(&principal, &project, &conversation, &session, "busy", None)
        .unwrap();
    let queued = repository.queued_tasks().unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].1.id, peer_task.id);
}

#[test]
fn tasks_are_claimed_with_the_run_atomically_and_never_replayed() {
    let (repository, principal, project) = setup();
    let task = repository
        .create_task(&principal, &project.id, task_request())
        .unwrap();
    let second = repository
        .create_task(&principal, &project.id, task_request())
        .unwrap();
    let session = repository
        .get_or_create_agent_session(&principal, &project, "codex")
        .unwrap();
    let conversation = repository
        .get_or_create_conversation(&principal, &project)
        .unwrap();
    let started = repository
        .submit_message(
            &principal,
            &project,
            &conversation,
            &session,
            "work",
            Some(&task.id),
        )
        .unwrap();
    assert_eq!(
        repository
            .task(&principal, &project.id, &task.id)
            .unwrap()
            .run_id,
        Some(started.run.id.clone())
    );
    assert!(matches!(
        repository.submit_message(
            &principal,
            &project,
            &conversation,
            &session,
            "work",
            Some(&second.id)
        ),
        Err(PlatformError::RunAlreadyActive)
    ));
    assert_eq!(
        repository
            .task(&principal, &project.id, &second.id)
            .unwrap()
            .status,
        "queued"
    );
    assert!(
        repository
            .cancel_task(&principal, &project.id, &task.id)
            .is_err()
    );
    repository
        .finish_run(&principal, &started.run.id, AgentRunStatus::Completed, None)
        .unwrap();
    assert_eq!(
        repository
            .task(&principal, &project.id, &task.id)
            .unwrap()
            .status,
        "completed"
    );
    assert!(matches!(
        repository.submit_message(
            &principal,
            &project,
            &conversation,
            &session,
            "work",
            Some(&task.id)
        ),
        Err(PlatformError::Conflict(_))
    ));
    let next = repository
        .submit_message(
            &principal,
            &project,
            &conversation,
            &session,
            "work",
            Some(&second.id),
        )
        .unwrap();
    repository.recover_after_server_restart().unwrap();
    assert_eq!(
        repository
            .task(&principal, &project.id, &second.id)
            .unwrap()
            .status,
        "interrupted"
    );
    assert_eq!(
        repository
            .get_run_feed(&principal, &project.id, &next.run.id, 0)
            .unwrap()
            .run
            .status,
        AgentRunStatus::Interrupted
    );
    assert!(repository.queued_tasks().unwrap().is_empty());
}

#[test]
fn scheduled_occurrences_coalesce_and_one_shots_survive_restart_without_replay() {
    let (repository, principal, project) = setup();
    let now = Utc::now();
    let schedule = repository
        .create_schedule(
            &principal,
            &project.id,
            CreateScheduleRequest {
                title: "Observe".into(),
                prompt: "Check facts".into(),
                interval_seconds: Some(60),
                at: Some(now - chrono::Duration::hours(2)),
                enabled: false,
                agent_id: None,
                source_ids: vec![],
                delivery: "queue".into(),
                collect_for_seconds: None,
            },
        )
        .unwrap();
    assert_eq!(repository.enqueue_schedule_inputs(now).unwrap(), 0);
    repository
        .set_schedule_enabled(&principal, &project.id, &schedule.id, true)
        .unwrap();
    assert_eq!(repository.enqueue_schedule_inputs(now).unwrap(), 1);
    assert_eq!(repository.enqueue_schedule_inputs(now).unwrap(), 0);
    assert_eq!(
        repository
            .enqueue_schedule_inputs(now + chrono::Duration::hours(1))
            .unwrap(),
        1
    );
    let inbox = repository.inbox(&principal, &project.id).unwrap();
    assert_eq!(
        inbox.len(),
        1,
        "pending occurrences share one bounded notification"
    );
    assert_eq!(
        repository
            .occurrences(&principal, &project.id, &schedule.id)
            .unwrap()
            .len(),
        2
    );
    repository
        .settle_input(&principal, &inbox[0], "cancelled", None)
        .unwrap();
    assert_eq!(
        repository
            .enqueue_schedule_inputs(now + chrono::Duration::hours(2))
            .unwrap(),
        1
    );
    assert_eq!(repository.inbox(&principal, &project.id).unwrap().len(), 2);
    repository
        .set_schedule_enabled(&principal, &project.id, &schedule.id, false)
        .unwrap();
    let once = repository
        .create_schedule(
            &principal,
            &project.id,
            CreateScheduleRequest {
                title: "One shot".into(),
                prompt: "Report".into(),
                interval_seconds: None,
                at: Some(now),
                enabled: true,
                agent_id: None,
                source_ids: vec![],
                delivery: "queue".into(),
                collect_for_seconds: None,
            },
        )
        .unwrap();
    assert_eq!(repository.enqueue_schedule_inputs(now).unwrap(), 1);
    repository.recover_after_server_restart().unwrap();
    repository.recover_inbox().unwrap();
    assert_eq!(
        repository
            .enqueue_schedule_inputs(now + chrono::Duration::days(1))
            .unwrap(),
        0
    );
    let once = repository
        .list_schedules(&principal, &project.id)
        .unwrap()
        .into_iter()
        .find(|s| s.id == once.id)
        .unwrap();
    assert!(!once.enabled);
    assert!(once.last_inbox_id.is_some());
    assert!(
        once.last_task_id.is_none(),
        "Inbox IDs must not be mislabeled as Task IDs"
    );
}

#[test]
fn management_queries_are_project_and_tenant_scoped() {
    let (repository, principal, project) = setup();
    let task = repository
        .create_task(&principal, &project.id, task_request())
        .unwrap();
    let other = repository.create_project(&principal, "Other").unwrap();
    let outsider = Principal {
        tenant_id: "outsider".into(),
        user_id: "local-user".into(),
    };
    assert!(matches!(
        repository.task(&principal, &other.id, &task.id),
        Err(PlatformError::NotFound)
    ));
    assert!(matches!(
        repository.list_tasks(&outsider, &project.id),
        Err(PlatformError::NotFound)
    ));
    assert!(matches!(
        repository.workspace(&outsider, &project.id),
        Err(PlatformError::NotFound)
    ));
    assert!(matches!(
        repository.list_schedules(&outsider, &project.id),
        Err(PlatformError::NotFound)
    ));
    assert!(
        repository
            .create_schedule(
                &principal,
                &project.id,
                CreateScheduleRequest {
                    title: "Bad".into(),
                    prompt: "Test".into(),
                    interval_seconds: Some(1),
                    at: None,
                    enabled: true,
                    agent_id: None,
                    source_ids: vec![],
                    delivery: "queue".into(),
                    collect_for_seconds: None,
                }
            )
            .is_err()
    );
    assert!(
        repository
            .create_task(
                &principal,
                &project.id,
                CreateTaskRequest {
                    title: " ".into(),
                    prompt: "Test".into()
                }
            )
            .is_err()
    );
}
