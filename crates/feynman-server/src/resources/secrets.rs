use super::*;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Secrets {
    root: PathBuf,
    keychain: bool,
    #[cfg(target_os = "macos")]
    service: String,
}
impl Secrets {
    pub fn new(root: PathBuf) -> PlatformResult<Self> {
        feynman_local::private_directory(&root)
            .map_err(|_| PlatformError::Storage("private secret directory unavailable".into()))?;
        let mode = std::env::var("FEYNMAN_SECRET_STORE").unwrap_or_else(|_| {
            if cfg!(target_os = "macos") {
                "keychain".into()
            } else {
                "disabled".into()
            }
        });
        if !matches!(mode.as_str(), "keychain" | "private_file" | "disabled") {
            return Err(invalid("unknown secret store"));
        }
        Ok(Self {
            #[cfg(target_os = "macos")]
            service: format!("Feynman.workspace:{}", root.display()),
            root,
            keychain: mode == "keychain",
        })
    }
    fn account(&self, project: &str, id: &str, version: i64) -> PlatformResult<String> {
        if ![project, id]
            .iter()
            .all(|v| v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            || version < 1
        {
            return Err(invalid("invalid secret reference"));
        }
        Ok(format!("{project}.{id}.{version}"))
    }
    pub fn put(
        &self,
        project: &str,
        id: &str,
        version: i64,
        value: &str,
    ) -> PlatformResult<String> {
        if value.is_empty() || value.len() > 16384 || value.contains('\0') {
            return Err(invalid(
                "secret must be nonempty, <=16 KiB and contain no NUL",
            ));
        }
        let account = self.account(project, id, version)?;
        if self.keychain {
            #[cfg(target_os = "macos")]
            {
                security_framework::passwords::set_generic_password(
                    &self.service,
                    &account,
                    value.as_bytes(),
                )
                .map_err(|_| PlatformError::Storage("Keychain secret write failed".into()))?;
                return Ok("keychain".into());
            }
            #[cfg(not(target_os = "macos"))]
            {
                return Err(invalid("Keychain not available on this platform"));
            }
        }
        if std::env::var("FEYNMAN_SECRET_STORE").as_deref() != Ok("private_file") {
            return Err(invalid(
                "secret storage disabled; explicitly opt into private_file on non-macOS",
            ));
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut f = options.open(self.root.join(account)).map_err(|_| {
            PlatformError::Storage("secret version already exists or cannot be created".into())
        })?;
        f.write_all(value.as_bytes())
            .and_then(|_| f.sync_all())
            .map_err(|_| PlatformError::Storage("secret write failed".into()))?;
        Ok("private_file".into())
    }
    pub fn read(&self, project: &str, meta: &SecretMetadata) -> PlatformResult<String> {
        let account = self.account(project, &meta.id, meta.version)?;
        let bytes = match meta.storage.as_str() {
            "keychain" => {
                #[cfg(target_os = "macos")]
                {
                    security_framework::passwords::generic_password(
                        security_framework::passwords::PasswordOptions::new_generic_password(
                            &self.service,
                            &account,
                        ),
                    )
                    .map_err(|_| PlatformError::Storage("Keychain secret unavailable".into()))?
                }
                #[cfg(not(target_os = "macos"))]
                {
                    return Err(invalid("Keychain not available"));
                }
            }
            "private_file" => private_read(&self.root.join(account))?,
            _ => return Err(invalid("unsupported secret storage")),
        };
        String::from_utf8(bytes)
            .map_err(|_| PlatformError::Storage("invalid secret encoding".into()))
    }
}
fn private_read(path: &Path) -> PlatformResult<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|_| PlatformError::Storage("secret unavailable".into()))?;
    if !meta.is_file() || meta.len() > 16384 {
        return Err(invalid("unsafe secret file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(invalid("secret file permissions are not private"));
        }
    }
    std::fs::read(path).map_err(|_| PlatformError::Storage("secret unavailable".into()))
}
