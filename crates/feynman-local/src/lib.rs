//! Local filesystem policy shared by the CLI and server, not domain state.
pub mod evaluation;
/// Fixed native CLI bridge shared by the owner CLI and credential Broker.
pub const NATIVE_CLI: &str = include_str!("native_cli.py");
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const DEFAULT_CONFIG: &str = "# Feynman-owned Codex configuration. Personal harness config is not imported.\napproval_policy = \"on-request\"\ncli_auth_credentials_store = \"file\"\n";

#[derive(Debug, Clone, Serialize)]
pub struct LocalHome {
    pub home: PathBuf,
    pub database: PathBuf,
    pub codex_home: PathBuf,
    pub workspace_root: PathBuf,
    pub private_root: PathBuf,
}

impl LocalHome {
    pub fn from_env() -> Result<Self> {
        Self::resolve(
            std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")),
            std::env::var_os("FEYNMAN_HOME"),
            std::env::var_os("FEYNMAN_DATA_DIR"),
            std::env::var_os("FEYNMAN_DB"),
            std::env::var_os("FEYNMAN_WORKSPACE_ROOT"),
            &std::env::current_dir()?,
        )
    }

    fn resolve(
        user_home: Option<OsString>,
        home: Option<OsString>,
        legacy_home: Option<OsString>,
        database: Option<OsString>,
        workspace: Option<OsString>,
        cwd: &Path,
    ) -> Result<Self> {
        let absolute = |value: OsString| -> Result<PathBuf> {
            if value.is_empty() {
                bail!("Feynman path settings must not be empty");
            }
            let value = PathBuf::from(value);
            Ok(if value.is_absolute() {
                value
            } else {
                cwd.join(value)
            })
        };
        let home = home.map(absolute).transpose()?;
        let legacy = legacy_home.map(absolute).transpose()?;
        if let (Some(home), Some(legacy)) = (&home, &legacy) {
            if home != legacy {
                bail!(
                    "FEYNMAN_HOME and legacy FEYNMAN_DATA_DIR select different directories; set only one"
                );
            }
        }
        let user_home = user_home.map(absolute).transpose()?;
        let home = home
            .or(legacy)
            .or_else(|| user_home.as_ref().map(|p| p.join(".feynman")))
            .context("set FEYNMAN_HOME or provide HOME/USERPROFILE")?;
        if home.parent().is_none()
            || user_home.as_ref() == Some(&home)
            || home
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            bail!(
                "Feynman home must be a dedicated directory, not a filesystem/user root or a path containing '..'"
            );
        }
        Ok(Self {
            database: database
                .map(absolute)
                .transpose()?
                .unwrap_or_else(|| home.join("rust-platform.sqlite3")),
            codex_home: home.join("codex"),
            workspace_root: workspace
                .map(absolute)
                .transpose()?
                .unwrap_or_else(|| home.join("workspaces")),
            private_root: home.join("private"),
            home,
        })
    }

    /// Never imports auth, personal configuration or old project registries implicitly.
    pub fn initialize(&self) -> Result<()> {
        private_directory(&self.home)?;
        private_directory(&self.codex_home)?;
        private_directory(&self.private_root)?;
        let config = self.codex_home.join("config.toml");
        if !regular_file_exists(&config)? {
            create_private_file(&config, DEFAULT_CONFIG.as_bytes())?;
        }
        // A linked credential cache would couple logouts and refresh writes to the
        // user's harness, defeating the owned-home contract.
        regular_file_exists(&self.codex_home.join("auth.json"))?;
        Ok(())
    }

    pub fn check_legacy_registry(&self, cwd: &Path, explicitly_selected: bool) -> Result<()> {
        let old_registry = cwd.join(".feynman/rust-platform.sqlite3");
        if !explicitly_selected && !self.database.exists() && old_registry.is_file() {
            bail!(
                "an existing working-directory registry was found at {}; refusing an implicit switch to an empty user registry. Select the old directory with FEYNMAN_HOME and restore its Codex session history explicitly",
                old_registry.display()
            );
        }
        Ok(())
    }

    /// Explicit, local-only credential import. Never overwrites a different cache.
    pub fn import_codex_auth(&self, source_home: &Path) -> Result<()> {
        let source = source_home.join("auth.json");
        if !regular_file_exists(&source)? {
            bail!(
                "source Codex home has no regular auth.json; keyring login cannot be imported this way"
            );
        }
        if fs::metadata(&source)?.len() > 1024 * 1024 {
            bail!("source auth cache exceeds the import size limit");
        }
        let bytes = fs::read(&source).context("read source Codex auth cache")?;
        if !matches!(
            serde_json::from_slice::<serde_json::Value>(&bytes),
            Ok(serde_json::Value::Object(_))
        ) {
            bail!("source auth cache must be a JSON object");
        }
        self.initialize()?;
        let destination = self.codex_home.join("auth.json");
        if regular_file_exists(&destination)? {
            if fs::read(&destination)? == bytes {
                return Ok(());
            }
            bail!("Feynman already has a different auth cache; refusing to overwrite it");
        }
        create_private_file(&destination, &bytes)
    }

    pub fn auth_cache_present(&self) -> bool {
        regular_file_exists(&self.codex_home.join("auth.json")).unwrap_or(false)
    }
}

fn regular_file_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => bail!(
            "expected a regular file, not a link or directory: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

pub fn private_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                bail!(
                    "Feynman directory must not be a symlink or file: {}",
                    path.display()
                );
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o077 != 0 {
                    bail!(
                        "Feynman directory must be private (0700); adjust permissions explicitly: {}",
                        path.display()
                    );
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder
                .create(path)
                .with_context(|| format!("create private directory {}", path.display()))?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn create_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        // Atomic publication with no replacement, including a racing symlink.
        fs::hard_link(&temporary, path).context("publish new private file without overwriting")?;
        Ok(())
    })();
    let _ = fs::remove_file(&temporary);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(root: &Path) -> LocalHome {
        LocalHome::resolve(
            Some(root.join("user").into_os_string()),
            Some(root.join("feynman").into_os_string()),
            None,
            None,
            None,
            root,
        )
        .unwrap()
    }

    #[test]
    fn defaults_are_user_scoped_and_do_not_depend_on_cwd() {
        for cwd in ["/project/a", "/project/b"] {
            let home = LocalHome::resolve(
                Some("/users/test".into()),
                None,
                None,
                None,
                None,
                Path::new(cwd),
            )
            .unwrap();
            assert_eq!(home.home, Path::new("/users/test/.feynman"));
            assert_eq!(home.codex_home, home.home.join("codex"));
            assert_eq!(home.database, home.home.join("rust-platform.sqlite3"));
        }
    }

    #[test]
    fn explicit_paths_and_legacy_alias_are_preserved() {
        let home = LocalHome::resolve(
            None,
            None,
            Some("data".into()),
            Some("registry.db".into()),
            Some("work".into()),
            Path::new("/test"),
        )
        .unwrap();
        assert_eq!(home.home, Path::new("/test/data"));
        assert_eq!(home.database, Path::new("/test/registry.db"));
        assert_eq!(home.workspace_root, Path::new("/test/work"));
        assert!(
            LocalHome::resolve(
                None,
                Some("a".into()),
                Some("b".into()),
                None,
                None,
                Path::new("/test")
            )
            .is_err()
        );
        assert!(
            LocalHome::resolve(None, Some("".into()), None, None, None, Path::new("/test"))
                .is_err()
        );
        assert!(
            LocalHome::resolve(
                Some("/user".into()),
                Some("/user".into()),
                None,
                None,
                None,
                Path::new("/test")
            )
            .is_err()
        );
        assert!(
            LocalHome::resolve(None, Some("/".into()), None, None, None, Path::new("/test"))
                .is_err()
        );
    }

    #[test]
    fn old_registry_is_not_silently_replaced_or_modified() {
        let root = std::env::temp_dir().join(format!("feynman-home-test-{}", uuid::Uuid::new_v4()));
        let home = layout(&root);
        let old = root.join(".feynman");
        fs::create_dir_all(&old).unwrap();
        let database = old.join("rust-platform.sqlite3");
        fs::write(&database, b"old registry dummy").unwrap();
        assert!(home.check_legacy_registry(&root, false).is_err());
        assert!(home.check_legacy_registry(&root, true).is_ok());
        assert!(!home.home.exists());
        assert_eq!(fs::read(&database).unwrap(), b"old registry dummy");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn initialization_and_auth_import_are_private_and_non_overwriting() {
        let root = std::env::temp_dir().join(format!("feynman-home-test-{}", uuid::Uuid::new_v4()));
        let home = layout(&root);
        home.initialize().unwrap();
        assert!(!home.auth_cache_present());
        fs::write(home.codex_home.join("config.toml"), "# user custom config").unwrap();
        home.initialize().unwrap();
        assert_eq!(
            fs::read_to_string(home.codex_home.join("config.toml")).unwrap(),
            "# user custom config"
        );
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("auth.json"), r#"{"test_token":"dummy"}"#).unwrap();
        home.import_codex_auth(&source).unwrap();
        home.import_codex_auth(&source).unwrap();
        fs::write(source.join("auth.json"), r#"{"test_token":"changed"}"#).unwrap();
        assert!(home.import_codex_auth(&source).is_err());
        assert_eq!(
            fs::read(home.codex_home.join("auth.json")).unwrap(),
            br#"{"test_token":"dummy"}"#
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&home.home).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(home.codex_home.join("auth.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_linked_or_public_state_without_changing_it() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = std::env::temp_dir().join(format!("feynman-home-test-{}", uuid::Uuid::new_v4()));
        let home = layout(&root);
        home.initialize().unwrap();
        let source = root.join("original.json");
        fs::write(&source, b"{}").unwrap();
        symlink(&source, home.codex_home.join("auth.json")).unwrap();
        assert!(home.initialize().is_err());
        assert_eq!(fs::read(&source).unwrap(), b"{}");
        fs::set_permissions(&home.home, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(home.initialize().is_err());
        assert_eq!(
            fs::metadata(&home.home).unwrap().permissions().mode() & 0o777,
            0o755
        );
        fs::remove_dir_all(root).unwrap();
    }
}
