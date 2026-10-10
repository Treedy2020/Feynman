use super::*;
use feynman_core::{ObservationChannel, ObservationFeedPage, ObservationFeedQuery};

impl ProjectManager {
    pub fn observation_feed(
        &self,
        p: &Principal,
        project: &str,
        agent: Option<&str>,
        q: &ObservationFeedQuery,
    ) -> PlatformResult<ObservationFeedPage> {
        if let Some(agent) = agent {
            self.repository.resolve_agent(p, project, Some(agent))?;
        }
        let all = self.sources(p, project)?;
        if let Some(id) = &q.source_id {
            let source = all
                .iter()
                .find(|s| &s.id == id)
                .ok_or(PlatformError::NotFound)?;
            if agent.is_some_and(|id| source.agent_id != id) {
                return Err(PlatformError::Unauthorized);
            }
            if source.kind != "manual" {
                self.source_permission(p, source)?;
            }
        }
        let mut allowed = Vec::new();
        let mut channels = Vec::new();
        for source in all {
            if q.source_id.as_ref().is_some_and(|id| id != &source.id)
                || agent.is_some_and(|id| source.agent_id != id)
            {
                continue;
            }
            if source.kind != "manual" && self.source_permission(p, &source).is_err() {
                continue;
            }
            allowed.push(source.id.clone());
            channels.push(ObservationChannel {
                source_id: source.id,
                name: source.name,
                kind: source.kind,
                enabled: source.enabled,
                listener_state: source.listener_state,
                coverage: source.coverage,
                poll_seconds: source.poll_seconds,
            });
        }
        let (records, next_after, has_more) =
            self.repository.observation_feed(p, project, &allowed, q)?;
        Ok(ObservationFeedPage {
            records,
            next_after,
            has_more,
            channels,
        })
    }
}
