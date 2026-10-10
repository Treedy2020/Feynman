//! Push transport over persisted state. Broadcast is a wake-up, never the source of truth.
use super::*;
use axum::response::sse::{Event, KeepAlive, Sse};
use serde_json::Value;
use std::{collections::HashMap, convert::Infallible, time::Duration};

#[derive(Deserialize)]
pub(super) struct NotificationQuery {
    after: Option<i64>,
    limit: Option<i64>,
}
pub(super) async fn notifications(
    State(s): State<HttpState>,
    h: HeaderMap,
    Query(q): Query<NotificationQuery>,
) -> Result<Json<Value>, ApiError> {
    let p = principal(&s, &h).await?;
    Ok(Json(json!(s.service.notifications(
        &p,
        q.after.unwrap_or(0),
        q.limit.unwrap_or(100)
    )?)))
}
pub(super) async fn read_notification(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let p = principal(&s, &h).await?;
    s.service.read_notification(&p, &id)?;
    Ok(StatusCode::NO_CONTENT)
}
fn event(name: &str, value: Value, id: Option<String>) -> Event {
    let e = Event::default().event(name).data(value.to_string());
    if let Some(id) = id { e.id(id) } else { e }
}
fn snapshot(
    s: &HttpState,
    p: &Principal,
    project: &str,
) -> Result<(Value, HashMap<String, i64>), PlatformError> {
    let (conversation, feed) = s.service.stream_snapshot(p, project)?;
    let mut cursors = HashMap::new();
    if let Some(f) = &feed {
        cursors.insert(
            f.run.id.clone(),
            f.events.last().map(|e| e.sequence).unwrap_or(0),
        );
    }
    let inbox = s
        .manager
        .as_ref()
        .map(|m| m.inbox(p, project))
        .transpose()?
        .unwrap_or_default();
    Ok((
        json!({"conversation":conversation,"feed":feed,"inbox":inbox}),
        cursors,
    ))
}
pub(super) async fn project_stream(
    State(s): State<HttpState>,
    h: HeaderMap,
    Path(project): Path<String>,
) -> Result<Response, ApiError> {
    let p = principal(&s, &h).await?;
    let mut wake = s.service.subscribe_events(); // subscribe before snapshot to close the hand-off race
    let (initial, mut cursors) = snapshot(&s, &p, &project)?;
    let stream = async_stream::stream! {
        yield Ok::<_,Infallible>(event("snapshot",initial,None));
        let mut reconcile=tokio::time::interval(Duration::from_secs(1));
        let mut inbox_signature=String::new();
        loop {
            tokio::select! {
                received=wake.recv()=>match received {
                    Ok(e) if e.tenant_id==p.tenant_id=> {
                        if e.sequence<=cursors.get(&e.run_id).copied().unwrap_or(0) {continue;}
                        // Re-check project ownership; a tenant-wide broadcast is not authorization.
                        if let Ok(feed)=s.service.get_run_feed(&p,&project,&e.run_id,cursors.get(&e.run_id).copied().unwrap_or(0)) {
                            let need_snapshot=!cursors.contains_key(&e.run_id) || feed.events.iter().any(|x|matches!(x.kind.as_str(),"run_started"|"run_completed"|"run_failed"|"run_interrupted"|"approval_requested"|"approval_resolved"|"input_steered"));
                            if need_snapshot {
                                if let Ok((value,next))=snapshot(&s,&p,&project) {cursors.extend(next);yield Ok(event("snapshot",value,None));}
                            } else {
                                for item in feed.events {
                                    cursors.insert(item.run_id.clone(),item.sequence);
                                    let id=format!("{}:{}",item.run_id,item.sequence);
                                    yield Ok(event("run_event",json!({"run":feed.run,"event":item}),Some(id)));
                                }
                            }
                        }
                    },
                    Ok(_)=>{},
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=> {
                        if let Ok((value,next))=snapshot(&s,&p,&project) {cursors=next;yield Ok(event("snapshot",value,None));}
                    },
                    Err(_)=>break,
                },
                _=reconcile.tick()=> {
                    // Inbox mutations originate in the scheduler as well as HTTP; observe durable state.
                    if let Some(manager)=&s.manager {
                        if let Ok(inbox)=manager.inbox(&p,&project) {
                            let signature=serde_json::to_string(&inbox).unwrap_or_default();
                            if signature!=inbox_signature {inbox_signature=signature;yield Ok(event("inbox",json!({"inbox":inbox}),None));}
                        }
                    }
                }
            }
        }
    };
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response())
}
pub(super) async fn notification_stream(
    State(s): State<HttpState>,
    h: HeaderMap,
    Query(q): Query<NotificationQuery>,
) -> Result<Response, ApiError> {
    let p = principal(&s, &h).await?;
    let mut wake = s.service.subscribe_events();
    let head = s.service.notifications(&p, 0, 1)?.head;
    let last = h
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.parse::<i64>())
        .transpose()
        .map_err(|_| PlatformError::InvalidInput("invalid event cursor".into()))?;
    let mut after = last.or(q.after).unwrap_or(head); // initial connection does not toast old history
    if after < 0 || after > head {
        return Err(ApiError(PlatformError::InvalidInput(
            "event cursor outside current history".into(),
        )));
    }
    let stream = async_stream::stream! {
        yield Ok::<_,Infallible>(event("ready",json!({"head":head}),Some(after.to_string())));
        let mut reconcile=tokio::time::interval(Duration::from_secs(1));
        loop {
            match s.service.notifications(&p,after,100) {
                Ok(feed) if !feed.items.is_empty()=> {
                    after=feed.next_after;let more=feed.has_more;
                    yield Ok(event("notifications",json!(feed),Some(after.to_string())));
                    if more {continue;}
                },
                Ok(_)=>{},Err(_)=>break,
            }
            tokio::select! {received=wake.recv()=>{if matches!(received,Err(tokio::sync::broadcast::error::RecvError::Closed)){break;}},_=reconcile.tick()=>{}}
        }
    };
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response())
}
