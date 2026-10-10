use super::*;
use feynman_core::{Notification, NotificationFeed};

impl SqliteRepository {
    pub(super) fn attention_feed(
        &self,
        principal: &Principal,
        after: i64,
        limit: i64,
    ) -> PlatformResult<NotificationFeed> {
        if after < 0 || !(1..=200).contains(&limit) {
            return Err(PlatformError::InvalidInput(
                "invalid notification cursor or limit".into(),
            ));
        }
        let c = self.connection()?;
        let head: i64 = c
            .query_row(
                "SELECT COALESCE(MAX(sequence),0) FROM attention_events WHERE tenant_id=?",
                [&principal.tenant_id],
                |r| r.get(0),
            )
            .map_err(storage_error)?;
        let unread: i64 = c.query_row("SELECT COUNT(*) FROM attention_events n WHERE n.tenant_id=? AND NOT EXISTS (SELECT 1 FROM attention_reads d WHERE d.tenant_id=n.tenant_id AND d.user_id=? AND d.notification_id=n.id)",
            params![principal.tenant_id,principal.user_id],|r|r.get(0)).map_err(storage_error)?;
        let mut q = c.prepare("SELECT n.sequence,n.id,p.id,p.name,n.run_id,n.kind,n.created_at,
            EXISTS(SELECT 1 FROM attention_reads d WHERE d.tenant_id=n.tenant_id AND d.user_id=? AND d.notification_id=n.id),
            CASE WHEN n.kind='approval_requested' THEN r.status!='waiting_approval' OR EXISTS (
                SELECT 1 FROM chat_events done JOIN chat_events original ON original.id=n.id
                WHERE done.run_id=n.run_id AND done.kind='approval_resolved' AND done.sequence>original.sequence
                AND json_extract(done.payload_json,'$.requestId')=json_extract(original.payload_json,'$.requestId')
            ) ELSE 0 END
            FROM attention_events n JOIN agent_runs r ON r.id=n.run_id AND r.tenant_id=n.tenant_id
            JOIN projects p ON p.id=r.project_id AND p.tenant_id=n.tenant_id
            WHERE n.tenant_id=? AND n.sequence>? ORDER BY n.sequence LIMIT ?").map_err(storage_error)?;
        let mut items = q
            .query_map(
                params![principal.user_id, principal.tenant_id, after, limit + 1],
                |r| {
                    Ok(Notification {
                        sequence: r.get(0)?,
                        id: r.get(1)?,
                        project_id: r.get(2)?,
                        project_name: r.get(3)?,
                        run_id: r.get(4)?,
                        kind: r.get(5)?,
                        created_at: parse_time(r.get(6)?)?,
                        read: r.get(7)?,
                        resolved: r.get(8)?,
                    })
                },
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        let has_more = items.len() > limit as usize;
        items.truncate(limit as usize);
        let next_after = items.last().map(|n| n.sequence).unwrap_or(after);
        Ok(NotificationFeed {
            items,
            next_after,
            head,
            unread,
            has_more,
        })
    }
    pub(super) fn attention_read(&self, principal: &Principal, id: &str) -> PlatformResult<()> {
        let c = self.connection()?;
        let exists: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM attention_events WHERE tenant_id=? AND id=?)",
                params![principal.tenant_id, id],
                |r| r.get(0),
            )
            .map_err(storage_error)?;
        if !exists {
            return Err(PlatformError::NotFound);
        }
        c.execute("INSERT OR IGNORE INTO attention_reads (tenant_id,user_id,notification_id) VALUES (?,?,?)",params![principal.tenant_id,principal.user_id,id]).map_err(storage_error)?;
        Ok(())
    }
}
