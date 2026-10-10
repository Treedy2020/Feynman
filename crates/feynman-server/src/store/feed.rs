//! Feed reads existing source histories; no mailbox copy or read acknowledgement.
use super::*;
use feynman_core::{Observation, ObservationFeedQuery};

impl SqliteRepository {
    pub fn observation_feed(
        &self,
        p: &Principal,
        project: &str,
        allowed: &[String],
        q: &ObservationFeedQuery,
    ) -> PlatformResult<(Vec<Observation>, i64, bool)> {
        self.get_project(p, project)?;
        if q.after < 0 || !(1..=200).contains(&q.limit) {
            return Err(PlatformError::InvalidInput(
                "after >= 0, limit 1..200".into(),
            ));
        }
        let sources = serde_json::to_string(allowed).map_err(storage_error)?;
        let c = self.connection()?;
        if q.head {
            let latest: i64 = c.query_row(
                "SELECT COALESCE(MAX(sequence),0) FROM observations WHERE tenant=? AND project=? AND source IN (SELECT value FROM json_each(?))",
                params![p.tenant_id, project, sources], |r| r.get(0),
            ).map_err(storage_error)?;
            return Ok((Vec::new(), latest, false));
        }
        // Permission filtering is in SQL, before LIMIT, cursors and body decoding.
        let mut s = c.prepare("SELECT sequence,source,event,occurred,observed,body FROM observations WHERE tenant=? AND project=? AND source IN (SELECT value FROM json_each(?)) AND sequence>? ORDER BY sequence LIMIT ?").map_err(storage_error)?;
        let mut records = s
            .query_map(
                params![p.tenant_id, project, sources, q.after, q.limit + 1],
                |r| {
                    Ok(Observation {
                        sequence: r.get(0)?,
                        source_id: r.get(1)?,
                        event_id: r.get(2)?,
                        occurred_at: parse_time(r.get(3)?)?,
                        observed_at: parse_time(r.get(4)?)?,
                        body: serde_json::from_str(&r.get::<_, String>(5)?).map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                5,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?,
                    })
                },
            )
            .map_err(storage_error)?
            .map(|r| r.map_err(storage_error))
            .collect::<PlatformResult<Vec<_>>>()?;
        let has_more = records.len() > q.limit as usize;
        records.truncate(q.limit as usize);
        let cursor = records.last().map_or(q.after, |r| r.sequence);
        Ok((records, cursor, has_more))
    }
}
