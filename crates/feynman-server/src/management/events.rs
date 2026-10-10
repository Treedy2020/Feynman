//! Long-lived DWS event consumers. History collection is NOT used to simulate listeners.
use super::*;
use feynman_core::{ObservationInput, ObservationSource};
use tokio::{io::AsyncReadExt, sync::oneshot, task::JoinHandle};

pub(super) struct ListenerJob {
    stop: oneshot::Sender<()>,
    handle: JoinHandle<()>,
}
impl ProjectManager {
    pub(super) async fn sync_events(self: &Arc<Self>) {
        let Ok(p) = self.platform.authenticate(None).await else {
            return;
        };
        let Ok(projects) = self.platform.list_projects(&p) else {
            return;
        };
        let mut jobs = self.event_jobs.lock().await;
        // Finished handles are reaped; failed sources require an explicit owner resume.
        jobs.retain(|_, job| !job.handle.is_finished());
        for project in projects {
            let Ok(sources) = self.sources(&p, &project.id) else {
                continue;
            };
            for s in sources {
                if s.kind != "dws_events"
                    || !s.enabled
                    || jobs.contains_key(&s.id)
                    || matches!(s.listener_state.as_str(), "failed" | "interrupted")
                {
                    continue;
                }
                if jobs.len() >= 16 || self.source_permission(&p, &s).is_err() {
                    continue;
                }
                let end = s.arguments["end"]
                    .as_str()
                    .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok());
                if end.is_none_or(|end| end <= Utc::now()) {
                    continue;
                }
                let (tx, rx) = oneshot::channel();
                let manager = self.clone();
                let actor = p.clone();
                let id = s.id.clone();
                let handle = tokio::spawn(async move {
                    manager.listen(actor, s, rx).await;
                });
                jobs.insert(id, ListenerJob { stop: tx, handle });
            }
        }
    }
    pub async fn close_sources(&self) {
        let jobs = std::mem::take(&mut *self.event_jobs.lock().await);
        for (_, job) in jobs {
            let _ = job.stop.send(());
            let _ = job.handle.await;
        }
    }
    async fn listen(
        &self,
        p: Principal,
        source: ObservationSource,
        mut stop: oneshot::Receiver<()>,
    ) {
        let result = self.listen_inner(&p, &source, &mut stop).await;
        if let Ok(mut current) = self.repository.source(&p, &source.project_id, &source.id) {
            current.listener_state = if result.is_ok() { "stopped" } else { "failed" }.into();
            current.coverage = "gap".into();
            current.updated_at = Utc::now();
            if result.is_ok() {
                current.listener_until = None;
            }
            let _=self.repository.record_capture(&p,&current,json!({"kind":"listener","source_id":current.id,"start":source.updated_at,"end":Utc::now(),"observed_at":Utc::now(),"complete":false,"state":current.listener_state}));
            let _ = self.repository.save_source(&p, &current);
        }
    }
    async fn listen_inner(
        &self,
        p: &Principal,
        source: &ObservationSource,
        stop: &mut oneshot::Receiver<()>,
    ) -> PlatformResult<()> {
        let broker = self.access.as_ref().ok_or(PlatformError::Unauthorized)?;
        let mut current = self.repository.source(p, &source.project_id, &source.id)?;
        current.listener_state = "starting".into();
        current.coverage = "gap".into();
        current.listener_until = Some(Utc::now() + chrono::Duration::minutes(11));
        current.updated_at = Utc::now();
        self.repository.save_source(p, &current)?;
        let mut child = broker.open_events(p, source).await?;
        let mut out = child
            .stdout
            .take()
            .ok_or_else(|| PlatformError::Runtime("event stdout missing".into()))?;
        let mut err = child
            .stderr
            .take()
            .ok_or_else(|| PlatformError::Runtime("event stderr missing".into()))?;
        let started = tokio::time::Instant::now();
        let mut ready = false;
        let mut out_buffer = Vec::new();
        let mut err_buffer = Vec::new();
        let mut buffer = [0u8; 8192];
        let mut err_bytes = [0u8; 8192];
        let mut check = tokio::time::interval(std::time::Duration::from_secs(1));
        let mut refresh = tokio::time::interval(std::time::Duration::from_secs(30));
        let mut out_closed = false;
        let mut err_closed = false;
        // All exits after spawn go through the graceful cleanup below, including SQL errors.
        let result=async { loop {
            tokio::select! {
                _=&mut *stop=>break Ok(()),
                _=check.tick()=>{
                    let source_now=self.repository.source(p,&source.project_id,&source.id);
                    let allowed=source_now.as_ref().is_ok_and(|s|s.enabled && self.source_permission(p,s).is_ok());
                    let until=source.arguments["end"].as_str().and_then(|s|chrono::DateTime::parse_from_rfc3339(s).ok());
                    if !allowed || until.is_none_or(|end|end<=Utc::now()) {break Ok(())}
                    if started.elapsed()>=std::time::Duration::from_secs(600) {break Ok(())}
                    if !ready && started.elapsed()>std::time::Duration::from_secs(30) {break Err(PlatformError::Runtime("DWS listener did not report ready".into()))}
                    if out_closed && err_closed {
                        break if ready && out_buffer.is_empty() {Ok(())}else{Err(PlatformError::Runtime("listener ended without a complete ready stream".into()))}
                    }
                },
                _=refresh.tick()=>if broker.verify_event_identity(p,source).await.is_err(){break Err(PlatformError::Conflict("DWS event authorization changed".into()))},
                n=err.read(&mut err_bytes),if !err_closed=>{
                    let Ok(n)=n else{break Err(PlatformError::Runtime("event stderr unavailable".into()))};
                    if n==0 {err_closed=true;continue}
                    err_buffer.extend_from_slice(&err_bytes[..n]);
                    if err_buffer.len()>65536 {break Err(PlatformError::Runtime("event status frame too large".into()))}
                    while let Some(i)=err_buffer.iter().position(|b|*b==b'\n') {
                        let line=err_buffer.drain(..=i).collect::<Vec<_>>();
                        if String::from_utf8_lossy(&line).starts_with("[event] ready ") {
                            ready=true;let mut s=self.repository.source(p,&source.project_id,&source.id)?;s.listener_state="live".into();s.updated_at=Utc::now();self.repository.save_source(p,&s)?;
                        }
                    }
                },
                n=out.read(&mut buffer),if !out_closed=>{
                    let Ok(n)=n else{break Err(PlatformError::Runtime("event stdout unavailable".into()))};
                    if n==0 {out_closed=true;continue}
                    out_buffer.extend_from_slice(&buffer[..n]);
                    if out_buffer.len()>65536 {break Err(PlatformError::Runtime("event frame too large".into()))}
                }
            }
            if ready {
                while let Some(i)=out_buffer.iter().position(|b|*b==b'\n') {
                    let line=out_buffer.drain(..=i).collect::<Vec<_>>();
                    let mut body=serde_json::from_slice::<Value>(&line).map_err(|_|PlatformError::Runtime("invalid event frame; coverage remains a gap".into()))?;
                    crate::external_access::scrub_auth_fields(&mut body);
                    let id=body["message_id"].as_str().filter(|s|!s.is_empty());
                    let time=body.get("create_time").or_else(||body.get("event_time")).or_else(||body.get("timestamp")).and_then(super::automation::parse_dws_time);
                    let start=source.arguments["start"].as_str().and_then(|s|chrono::DateTime::parse_from_rfc3339(s).ok());let end=source.arguments["end"].as_str().and_then(|s|chrono::DateTime::parse_from_rfc3339(s).ok());
                    let target_ok=match source.arguments["target_kind"].as_str() {Some("group")=>body["conversation_id"]==source.arguments["target_id"],Some("open_dingtalk_id")=>body["sender_open_dingtalk_id"]==source.arguments["target_id"],_=>false};
                    if let (Some(id),Some(time),Some(start),Some(end))=(id,time,start,end) {
                        if target_ok && time>=start && time<end && self.source_permission(p,source).is_ok() && self.repository.source(p,&source.project_id,&source.id)?.enabled {
                            self.repository.record_observations(p,&source.project_id,&source.id,vec![ObservationInput{event_id:id.into(),occurred_at:time,body:body.clone()}])?;
                        }
                    }
                }
            }
        }}.await;
        // SIGTERM is required for DWS subscription cleanup; never send SIGKILL.
        if let Some(pid) = child.id() {
            let _ = tokio::process::Command::new("/bin/kill")
                .args(["-TERM", &pid.to_string()])
                .status()
                .await;
        }
        match tokio::time::timeout(std::time::Duration::from_secs(10), child.wait()).await {
            Ok(Ok(status)) if result.is_ok() && !status.success() && status.code().is_some() => {
                return Err(PlatformError::Runtime(
                    "DWS listener failed; owner resume required".into(),
                ));
            }
            Ok(Ok(_)) => {}
            _ => {
                return Err(PlatformError::Runtime(
                    "DWS listener cleanup unconfirmed; bounded consumer may still run".into(),
                ));
            }
        }
        result
    }
}
