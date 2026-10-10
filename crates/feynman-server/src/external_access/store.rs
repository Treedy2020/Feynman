use feynman_core::{
    ExternalAccessRequest, ExternalConnection, PlatformError, PlatformResult, Principal,
    ProjectCliBinding,
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredConnection {
    pub public: ExternalConnection,
    pub dws_source: Option<(String, String)>,
}

pub struct AccessStore {
    db: Mutex<Connection>,
    root: PathBuf,
}
fn storage() -> PlatformError {
    PlatformError::Storage("External Access storage failed".into())
}

pub fn private_dir(path: &Path) -> PlatformResult<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(storage());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(path).map_err(|_| storage())?;
        }
        Err(_) => return Err(storage()),
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| storage())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(storage());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(storage());
        }
    }
    Ok(())
}

fn private_file(path: &Path) -> PlatformResult<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| storage())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16_384 {
        return Err(storage());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(storage());
        }
    }
    Ok(())
}

impl AccessStore {
    pub fn open(root: PathBuf) -> PlatformResult<Self> {
        private_dir(&root)?;
        let path = root.join("access.sqlite3");
        if !path.exists() {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(&path).map_err(|_| storage())?;
        }
        // DB grows past the secret-file size cap; validate type and mode separately.
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| storage())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(storage());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(storage());
            }
        }
        let db = Connection::open(path).map_err(|_| storage())?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS access_items(kind TEXT NOT NULL,id TEXT NOT NULL,tenant TEXT NOT NULL,owner TEXT NOT NULL,project TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(kind,id,tenant,owner));").map_err(|_| storage())?;
        Ok(Self {
            db: Mutex::new(db),
            root,
        })
    }
    pub fn save<T: Serialize>(
        &self,
        principal: &Principal,
        kind: &str,
        id: &str,
        project: &str,
        value: &T,
    ) -> PlatformResult<()> {
        let data = serde_json::to_string(value).map_err(|_| storage())?;
        self.db.lock().map_err(|_| storage())?.execute("INSERT INTO access_items VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(kind,id,tenant,owner) DO UPDATE SET data=excluded.data,project=excluded.project", params![kind,id,principal.tenant_id,principal.user_id,project,data]).map_err(|_| storage())?;
        Ok(())
    }
    pub fn list<T: DeserializeOwned>(
        &self,
        principal: &Principal,
        kind: &str,
        project: Option<&str>,
    ) -> PlatformResult<Vec<T>> {
        let db = self.db.lock().map_err(|_| storage())?;
        let mut query = db.prepare("SELECT data FROM access_items WHERE kind=?1 AND tenant=?2 AND owner=?3 AND (?4 IS NULL OR project=?4) ORDER BY rowid").map_err(|_| storage())?;
        let rows = query
            .query_map(
                params![kind, principal.tenant_id, principal.user_id, project],
                |row| row.get::<_, String>(0),
            )
            .map_err(|_| storage())?;
        rows.map(|row| serde_json::from_str(&row.map_err(|_| storage())?).map_err(|_| storage()))
            .collect()
    }
    pub fn connection(&self, principal: &Principal, id: &str) -> PlatformResult<StoredConnection> {
        self.list::<StoredConnection>(principal, "connection", None)?
            .into_iter()
            .find(|c| c.public.id == id)
            .ok_or(PlatformError::NotFound)
    }
    pub fn bindings(
        &self,
        principal: &Principal,
        project: &str,
    ) -> PlatformResult<Vec<ProjectCliBinding>> {
        self.list(principal, "binding", Some(project))
    }
    pub fn requests(
        &self,
        principal: &Principal,
        project: &str,
    ) -> PlatformResult<Vec<ExternalAccessRequest>> {
        self.list(principal, "request", Some(project))
    }
    pub fn secret(&self, id: &str) -> PlatformResult<String> {
        let path = self.secret_path(id)?;
        private_file(&path)?;
        std::fs::read_to_string(path).map_err(|_| storage())
    }
    fn secret_path(&self, id: &str) -> PlatformResult<PathBuf> {
        if !id.starts_with("connection_")
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || id.len() > 100
        {
            return Err(PlatformError::NotFound);
        }
        Ok(self.root.join(format!("{id}.secret")))
    }
    pub fn write_secret(&self, id: &str, token: &str) -> PlatformResult<()> {
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(self.secret_path(id)?).map_err(|_| storage())?;
        file.write_all(token.as_bytes()).map_err(|_| storage())?;
        file.sync_all().map_err(|_| storage())?;
        Ok(())
    }
    pub fn unbind(
        &self,
        principal: &Principal,
        project: &str,
        provider: feynman_core::AccessProvider,
    ) -> PlatformResult<()> {
        let id = format!("{project}:{provider:?}");
        self.db.lock().map_err(|_| storage())?.execute("DELETE FROM access_items WHERE kind='binding' AND id=?1 AND tenant=?2 AND owner=?3",params![id,principal.tenant_id,principal.user_id]).map_err(|_| storage())?;
        Ok(())
    }
}

impl super::security::CredentialStore for AccessStore {
    fn put(&self, r: &super::security::CredentialRef, secret: &str) -> PlatformResult<()> {
        self.write_secret(&r.connection_id, secret)
    }
    fn read(&self, r: &super::security::CredentialRef) -> PlatformResult<String> {
        self.secret(&r.connection_id)
    }
}
impl super::security::AuditSink for AccessStore {
    fn record(
        &self,
        p: &Principal,
        project: &str,
        action: &str,
        object: Option<&str>,
    ) -> PlatformResult<()> {
        self.save(p,"audit",&feynman_core::opaque_id("audit"),project,&serde_json::json!({"action":action,"project_id":project,"connection_id":object,"at":chrono::Utc::now()}))
    }
}
