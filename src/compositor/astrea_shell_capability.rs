use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg_attr(test, allow(dead_code))]
pub(crate) const ASTREA_SHELL_CAPABILITY_FILE_ENV: &str = "ASTREA_SHELL_CAPABILITY_FILE";
const CAPABILITY_BYTES: usize = 32;
const CAPABILITY_HEX_BYTES: usize = CAPABILITY_BYTES * 2;

#[cfg(test)]
pub(crate) fn test_capability_path(socket_name: &str) -> PathBuf {
    std::env::temp_dir()
        .join(format!(
            "oblivion-one-test-capabilities-{}",
            std::process::id()
        ))
        .join(format!("capability-{socket_name}"))
}

pub(crate) struct AstreaShellCapability {
    value: [u8; CAPABILITY_HEX_BYTES],
    path: PathBuf,
}

#[derive(Clone)]
pub(crate) struct AstreaShellCapabilityVerifier {
    value: [u8; CAPABILITY_HEX_BYTES],
}

impl fmt::Debug for AstreaShellCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AstreaShellCapability")
            .field("path", &self.path)
            .field("value", &"<redacted>")
            .finish()
    }
}

impl fmt::Debug for AstreaShellCapabilityVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AstreaShellCapabilityVerifier")
            .field("value", &"<redacted>")
            .finish()
    }
}

impl AstreaShellCapability {
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn create_from_environment() -> io::Result<Self> {
        let path = std::env::var_os(ASTREA_SHELL_CAPABILITY_FILE_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("XDG_RUNTIME_DIR")
                    .filter(|value| !value.is_empty())
                    .map(|runtime| PathBuf::from(runtime).join("astrea-shell/capability"))
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "XDG_RUNTIME_DIR is required for Astrea shell capability handoff",
                )
            })?;
        Self::create_for_path(path)
    }

    pub(crate) fn create_for_path(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Astrea shell capability path has no parent",
            )
        })?;
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;

        let value = random_capability()?;
        let temporary_path = parent.join(format!(
            ".capability.{}.{}.tmp",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let result: io::Result<()> = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary_path)?;
            file.write_all(&value)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::set_permissions(&temporary_path, fs::Permissions::from_mode(0o600))?;
            fs::rename(&temporary_path, &path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        result?;

        Ok(Self { value, path })
    }

    pub(crate) fn verifier(&self) -> AstreaShellCapabilityVerifier {
        AstreaShellCapabilityVerifier { value: self.value }
    }
}

impl Drop for AstreaShellCapability {
    fn drop(&mut self) {
        let Ok(mut file) = File::open(&self.path) else {
            return;
        };
        let mut contents = Vec::with_capacity(CAPABILITY_HEX_BYTES + 1);
        let expected = self
            .value
            .iter()
            .copied()
            .chain(std::iter::once(b'\n'))
            .collect::<Vec<_>>();
        if std::io::Read::by_ref(&mut file)
            .take((CAPABILITY_HEX_BYTES + 1) as u64)
            .read_to_end(&mut contents)
            .is_err()
            || contents != expected
        {
            return;
        }
        let _ = fs::remove_file(&self.path);
    }
}

impl AstreaShellCapabilityVerifier {
    pub(crate) fn matches(&self, candidate: &str) -> bool {
        let candidate = candidate.as_bytes();
        if candidate.len() != self.value.len() {
            return false;
        }
        let mut difference = 0u8;
        for (&expected, &actual) in self.value.iter().zip(candidate) {
            difference |= expected ^ actual;
        }
        difference == 0
    }
}

fn random_capability() -> io::Result<[u8; CAPABILITY_HEX_BYTES]> {
    let mut random = [0u8; CAPABILITY_BYTES];
    File::open("/dev/urandom")?.read_exact(&mut random)?;
    let mut value = [0u8; CAPABILITY_HEX_BYTES];
    for (index, byte) in random.into_iter().enumerate() {
        value[index * 2] = b"0123456789abcdef"[(byte >> 4) as usize];
        value[index * 2 + 1] = b"0123456789abcdef"[(byte & 0x0f) as usize];
    }
    Ok(value)
}
