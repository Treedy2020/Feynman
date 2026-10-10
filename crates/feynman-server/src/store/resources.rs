//! Generic versioned metadata journal; secrets are deliberately stored elsewhere.
use super::*;
use feynman_core::ResourceRecord;

pub(super) fn migrate(c: &Connection) -> PlatformResult<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS project_resources(tenant TEXT NOT NULL, project TEXT NOT NULL, kind TEXT NOT NULL, id TEXT NOT NULL, revision INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(tenant,project,kind,id));
      CREATE TABLE IF NOT EXISTS resource_revisions(tenant TEXT NOT NULL,project TEXT NOT NULL,kind TEXT NOT NULL,id TEXT NOT NULL,revision INTEGER NOT NULL,data TEXT NOT NULL,PRIMARY KEY(tenant,project,kind,id,revision));
      CREATE TABLE IF NOT EXISTS resource_audit(sequence INTEGER PRIMARY KEY,tenant TEXT NOT NULL,project TEXT NOT NULL,kind TEXT NOT NULL,object_id TEXT NOT NULL,revision INTEGER NOT NULL,at TEXT NOT NULL);
      CREATE VIRTUAL TABLE IF NOT EXISTS memory_search USING fts5(tenant UNINDEXED,project UNINDEXED,id UNINDEXED,text,tokenize='trigram');").map_err(storage_error)
}

impl SqliteRepository {
    pub fn resource_owners(&self) -> PlatformResult<Vec<Principal>> {
        let c = self.connection()?;
        let mut s=c.prepare("SELECT DISTINCT tenant,json_extract(data,'$.owner_user') FROM project_resources WHERE kind='lease'").map_err(storage_error)?;
        s.query_map([], |r| {
            Ok(Principal {
                tenant_id: r.get(0)?,
                user_id: r.get(1)?,
            })
        })
        .map_err(storage_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_error)
    }
    pub fn resources(
        &self,
        p: &Principal,
        project: &str,
        kind: &str,
    ) -> PlatformResult<Vec<ResourceRecord>> {
        self.get_project(p, project)?;
        let c = self.connection()?;
        let mut s=c.prepare("SELECT id,revision,data FROM project_resources WHERE tenant=? AND project=? AND kind=? ORDER BY rowid").map_err(storage_error)?;
        s.query_map(params![p.tenant_id, project, kind], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(storage_error)?
        .map(|r| {
            let (id, revision, data) = r.map_err(storage_error)?;
            Ok(ResourceRecord {
                id,
                revision,
                data: serde_json::from_str(&data).map_err(storage_error)?,
            })
        })
        .collect()
    }
    pub fn resource(
        &self,
        p: &Principal,
        project: &str,
        kind: &str,
        id: &str,
        revision: Option<i64>,
    ) -> PlatformResult<ResourceRecord> {
        self.get_project(p, project)?;
        let c = self.connection()?;
        let (r,data):(i64,String)=if let Some(r)=revision {
            c.query_row("SELECT revision,data FROM resource_revisions WHERE tenant=? AND project=? AND kind=? AND id=? AND revision=?",params![p.tenant_id,project,kind,id,r],|r|Ok((r.get(0)?,r.get(1)?)))
        }else{
            c.query_row("SELECT revision,data FROM project_resources WHERE tenant=? AND project=? AND kind=? AND id=?",params![p.tenant_id,project,kind,id],|r|Ok((r.get(0)?,r.get(1)?)))
        }.optional().map_err(storage_error)?.ok_or(PlatformError::NotFound)?;
        Ok(ResourceRecord {
            id: id.into(),
            revision: r,
            data: serde_json::from_str(&data).map_err(storage_error)?,
        })
    }
    pub fn put_resource(
        &self,
        p: &Principal,
        project: &str,
        kind: &str,
        id: &str,
        expected: i64,
        data: Value,
    ) -> PlatformResult<ResourceRecord> {
        self.get_project(p, project)?;
        let encoded = serde_json::to_string(&data).map_err(storage_error)?;
        if encoded.len() > 131072 {
            return Err(PlatformError::InvalidInput("resource too large".into()));
        }
        let mut c = self.connection()?;
        let t = c.transaction().map_err(storage_error)?;
        let current=t.query_row("SELECT revision FROM project_resources WHERE tenant=? AND project=? AND kind=? AND id=?",params![p.tenant_id,project,kind,id],|r|r.get::<_,i64>(0)).optional().map_err(storage_error)?.unwrap_or(0);
        if current != expected {
            return Err(PlatformError::Conflict(
                "resource revision changed; reload before editing".into(),
            ));
        }
        let revision = current + 1;
        t.execute("INSERT INTO project_resources VALUES(?,?,?,?,?,?) ON CONFLICT(tenant,project,kind,id) DO UPDATE SET revision=excluded.revision,data=excluded.data",params![p.tenant_id,project,kind,id,revision,encoded]).map_err(storage_error)?;
        t.execute(
            "INSERT INTO resource_revisions VALUES(?,?,?,?,?,?)",
            params![p.tenant_id, project, kind, id, revision, encoded],
        )
        .map_err(storage_error)?;
        t.execute("INSERT INTO resource_audit(tenant,project,kind,object_id,revision,at) VALUES(?,?,?,?,?,?)",params![p.tenant_id,project,kind,id,revision,Utc::now().to_rfc3339()]).map_err(storage_error)?;
        if kind == "memory" {
            t.execute(
                "DELETE FROM memory_search WHERE tenant=? AND project=? AND id=?",
                params![p.tenant_id, project, id],
            )
            .map_err(storage_error)?;
            if data["content"]["state"] != "deleted" {
                let text = format!(
                    "{}\n{}\n{}",
                    data["content"]["title"].as_str().unwrap_or(""),
                    data["content"]["body"].as_str().unwrap_or(""),
                    data["content"]["tags"]
                );
                t.execute(
                    "INSERT INTO memory_search VALUES(?,?,?,?)",
                    params![p.tenant_id, project, id, text],
                )
                .map_err(storage_error)?;
            }
        }
        t.commit().map_err(storage_error)?;
        Ok(ResourceRecord {
            id: id.into(),
            revision,
            data,
        })
    }
    pub fn memory_candidates(
        &self,
        p: &Principal,
        project: &str,
        query: &str,
    ) -> PlatformResult<Vec<String>> {
        self.get_project(p, project)?;
        if query.len() > 400 {
            return Err(PlatformError::InvalidInput("query too long".into()));
        }
        let c = self.connection()?;
        let sql = if query.contains(['%', '_', '\\']) {
            "SELECT id FROM memory_search WHERE tenant=? AND project=? AND text LIKE ? ESCAPE '\\' ORDER BY id LIMIT 10000"
        } else {
            "SELECT id FROM memory_search WHERE tenant=? AND project=? AND text LIKE ? ORDER BY id LIMIT 10000"
        };
        let mut s = c.prepare(sql).map_err(storage_error)?;
        let literal = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        s.query_map(params![p.tenant_id, project, format!("%{literal}%")], |r| {
            r.get(0)
        })
        .map_err(storage_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn principal() -> Principal {
        Principal {
            tenant_id: "local".into(),
            user_id: "local-user".into(),
        }
    }

    #[test]
    fn revisions_are_atomic_and_stale_writers_cannot_overwrite() {
        let path = std::env::temp_dir().join(format!(
            "feynman-resources-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let repo = SqliteRepository::open(&path).unwrap();
        let other = SqliteRepository::open(&path).unwrap();
        let p = principal();
        let project = repo.create_project(&p, "Versions").unwrap();
        repo.put_resource(
            &p,
            &project.id,
            "profile",
            "debug",
            0,
            json!({"value":"old"}),
        )
        .unwrap();
        repo.put_resource(
            &p,
            &project.id,
            "profile",
            "debug",
            1,
            json!({"value":"new"}),
        )
        .unwrap();
        assert!(matches!(
            other.put_resource(
                &p,
                &project.id,
                "profile",
                "debug",
                1,
                json!({"value":"stale"})
            ),
            Err(PlatformError::Conflict(_))
        ));
        assert_eq!(
            repo.resource(&p, &project.id, "profile", "debug", Some(1))
                .unwrap()
                .data["value"],
            "old"
        );
        assert_eq!(
            other
                .resource(&p, &project.id, "profile", "debug", None)
                .unwrap()
                .data["value"],
            "new"
        );
        drop(other);
        drop(repo);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn chinese_and_literal_search_preserve_project_and_revision_boundaries() {
        let path = std::env::temp_dir().join(format!(
            "feynman-memory-search-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let repo = SqliteRepository::open(&path).unwrap();
        let p = principal();
        let project = repo.create_project(&p, "Search").unwrap();
        let other = repo.create_project(&p, "Other").unwrap();
        let data = json!({"content":{"title":"杨睿仪反馈", "body":"复现试验 100% 和字段 a_b", "tags":[], "state":"open"}});
        repo.put_resource(&p, &project.id, "memory", "memory_test", 0, data.clone())
            .unwrap();
        for query in ["杨睿仪", "杨睿", "100%", "a_b"] {
            assert_eq!(
                repo.memory_candidates(&p, &project.id, query).unwrap(),
                vec!["memory_test"]
            );
            assert!(
                repo.memory_candidates(&p, &other.id, query)
                    .unwrap()
                    .is_empty()
            );
        }
        let mut deleted = data;
        deleted["content"]["state"] = json!("deleted");
        repo.put_resource(&p, &project.id, "memory", "memory_test", 1, deleted)
            .unwrap();
        assert!(
            repo.memory_candidates(&p, &project.id, "")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            repo.resource(&p, &project.id, "memory", "memory_test", Some(1))
                .unwrap()
                .data["content"]["state"],
            "open"
        );
        drop(repo);
        std::fs::remove_file(path).unwrap();
    }
}
