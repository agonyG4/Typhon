//! Shared secure storage primitives for persistent control-plane configuration.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const ASTREA_DIRECTORY: &str = "AstreaOS";
const TYPHON_DIRECTORY: &str = "typhon";
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DirectorySyncPoint {
    BeforeReplace,
    AfterReplace,
    Rollback,
    Cleanup,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PrivateConfigError {
    Missing,
    Invalid,
    Insecure,
    WriteFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PrivateConfigFile {
    config_home: PathBuf,
    directory: PathBuf,
    file: PathBuf,
    file_name: String,
    create_missing_config_home: bool,
}

impl PrivateConfigFile {
    pub(crate) fn from_environment(file_name: &str) -> Result<Self, PrivateConfigError> {
        let (config_home, create_missing_config_home) = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(value) if !value.is_empty() => (PathBuf::from(value), false),
            _ => {
                let home = std::env::var_os("HOME").ok_or(PrivateConfigError::Insecure)?;
                (PathBuf::from(home).join(".config"), true)
            }
        };
        Self::new_with_policy(config_home, file_name, create_missing_config_home)
    }

    pub(crate) fn new(config_home: PathBuf, file_name: &str) -> Result<Self, PrivateConfigError> {
        Self::new_with_policy(config_home, file_name, false)
    }

    pub(crate) fn unavailable(file_name: &str) -> Self {
        let config_home = PathBuf::from("/.invalid");
        let directory = config_home.join(ASTREA_DIRECTORY).join(TYPHON_DIRECTORY);
        Self {
            config_home,
            file: directory.join(file_name),
            directory,
            file_name: file_name.to_owned(),
            create_missing_config_home: false,
        }
    }

    fn new_with_policy(
        config_home: PathBuf,
        file_name: &str,
        create_missing_config_home: bool,
    ) -> Result<Self, PrivateConfigError> {
        if !config_home.is_absolute()
            || file_name.is_empty()
            || Path::new(file_name).components().count() != 1
            || !matches!(
                Path::new(file_name).components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return Err(PrivateConfigError::Insecure);
        }
        let directory = config_home.join(ASTREA_DIRECTORY).join(TYPHON_DIRECTORY);
        Ok(Self {
            config_home,
            file: directory.join(file_name),
            directory,
            file_name: file_name.to_owned(),
            create_missing_config_home,
        })
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.file
    }

    pub(crate) fn read_bytes(&self, maximum_bytes: usize) -> Result<Vec<u8>, PrivateConfigError> {
        self.validate_existing_directories()?;
        let metadata = match fs::symlink_metadata(&self.file) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(PrivateConfigError::Missing);
            }
            Err(_) => return Err(PrivateConfigError::Insecure),
        };
        validate_private_file(&metadata)?;
        if metadata.len() > maximum_bytes as u64 {
            return Err(PrivateConfigError::Invalid);
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&self.file)
            .map_err(|_| PrivateConfigError::Insecure)?;
        validate_private_file(&file.metadata().map_err(|_| PrivateConfigError::Insecure)?)?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(maximum_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| PrivateConfigError::Invalid)?;
        if bytes.len() > maximum_bytes {
            return Err(PrivateConfigError::Invalid);
        }
        Ok(bytes)
    }

    pub(crate) fn write_bytes(
        &self,
        bytes: &[u8],
        maximum_bytes: usize,
    ) -> Result<(), PrivateConfigError> {
        self.write_bytes_with_directory_sync(bytes, maximum_bytes, |_, directory| {
            File::open(directory).and_then(|directory| directory.sync_all())
        })
    }

    fn write_bytes_with_directory_sync(
        &self,
        bytes: &[u8],
        maximum_bytes: usize,
        mut sync_directory: impl FnMut(DirectorySyncPoint, &Path) -> io::Result<()>,
    ) -> Result<(), PrivateConfigError> {
        if bytes.len() > maximum_bytes {
            return Err(PrivateConfigError::Invalid);
        }
        self.open_directories_for_write()?;
        let previous_file_exists = match fs::symlink_metadata(&self.file) {
            Ok(metadata) => {
                validate_private_file(&metadata)?;
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(_) => return Err(PrivateConfigError::Insecure),
        };
        let previous = if previous_file_exists {
            let backup = self.directory.join(format!(
                ".{}.previous-{}-{}",
                self.file_name,
                std::process::id(),
                NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::hard_link(&self.file, &backup).map_err(|_| PrivateConfigError::WriteFailed)?;
            let backup_metadata = match fs::symlink_metadata(&backup) {
                Ok(metadata) => metadata,
                Err(_) => {
                    let _ = fs::remove_file(&backup);
                    return Err(PrivateConfigError::Insecure);
                }
            };
            if let Err(error) = validate_private_file(&backup_metadata) {
                let _ = fs::remove_file(&backup);
                return Err(error);
            }
            if sync_directory(DirectorySyncPoint::BeforeReplace, &self.directory).is_err() {
                let _ = fs::remove_file(&backup);
                return Err(PrivateConfigError::WriteFailed);
            }
            Some(backup)
        } else {
            None
        };
        let temporary = self.directory.join(format!(
            ".{}.tmp-{}-{}",
            self.file_name,
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .mode(PRIVATE_FILE_MODE)
                .open(&temporary)
                .map_err(|_| PrivateConfigError::WriteFailed)?;
            file.write_all(bytes)
                .map_err(|_| PrivateConfigError::WriteFailed)?;
            file.flush().map_err(|_| PrivateConfigError::WriteFailed)?;
            file.sync_all()
                .map_err(|_| PrivateConfigError::WriteFailed)?;
            drop(file);
            fs::rename(&temporary, &self.file).map_err(|_| PrivateConfigError::WriteFailed)?;
            if sync_directory(DirectorySyncPoint::AfterReplace, &self.directory).is_err() {
                let rollback = match &previous {
                    Some(backup) => fs::rename(backup, &self.file),
                    None => fs::remove_file(&self.file),
                };
                if rollback.is_ok() {
                    let _ = sync_directory(DirectorySyncPoint::Rollback, &self.directory);
                    return Err(PrivateConfigError::WriteFailed);
                }
                return Ok(());
            }
            if previous
                .as_ref()
                .is_some_and(|backup| fs::remove_file(backup).is_ok())
            {
                let _ = sync_directory(DirectorySyncPoint::Cleanup, &self.directory);
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
            if let Some(backup) = &previous {
                let _ = fs::remove_file(backup);
            }
        }
        result
    }

    fn validate_existing_directories(&self) -> Result<(), PrivateConfigError> {
        validate_directory(&self.config_home, false)?;
        validate_directory(&self.config_home.join(ASTREA_DIRECTORY), true)?;
        validate_directory(&self.directory, true)
    }

    fn open_directories_for_write(&self) -> Result<(), PrivateConfigError> {
        if !self.config_home.exists() {
            if !self.create_missing_config_home {
                return Err(PrivateConfigError::WriteFailed);
            }
            fs::create_dir_all(&self.config_home).map_err(|_| PrivateConfigError::WriteFailed)?;
            fs::set_permissions(
                &self.config_home,
                fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE),
            )
            .map_err(|_| PrivateConfigError::WriteFailed)?;
        }
        validate_directory(&self.config_home, false)?;
        let astrea = self.config_home.join(ASTREA_DIRECTORY);
        ensure_private_directory(&astrea)?;
        ensure_private_directory(&self.directory)
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), PrivateConfigError> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_directory(path, true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|_| PrivateConfigError::WriteFailed)?;
            fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE))
                .map_err(|_| PrivateConfigError::WriteFailed)?;
            validate_directory(path, true)
        }
        Err(_) => Err(PrivateConfigError::Insecure),
    }
}

fn validate_directory(path: &Path, require_private: bool) -> Result<(), PrivateConfigError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            PrivateConfigError::Missing
        } else {
            PrivateConfigError::Insecure
        }
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.uid() != effective_uid()
        || metadata.permissions().mode() & 0o022 != 0
        || (require_private && metadata.permissions().mode() & 0o777 != PRIVATE_DIRECTORY_MODE)
    {
        return Err(PrivateConfigError::Insecure);
    }
    Ok(())
}

fn validate_private_file(metadata: &fs::Metadata) -> Result<(), PrivateConfigError> {
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.uid() != effective_uid()
        || metadata.permissions().mode() & 0o777 != PRIVATE_FILE_MODE
    {
        return Err(PrivateConfigError::Insecure);
    }
    Ok(())
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference memory.
    unsafe { libc::geteuid() as u32 }
}

#[cfg(test)]
mod tests {
    use super::{DirectorySyncPoint, PrivateConfigError, PrivateConfigFile};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn failed_directory_sync_after_replace_restores_the_previous_file() {
        let home = std::env::temp_dir().join(format!(
            "typhon-private-config-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&home).unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        let store = PrivateConfigFile::new(home.clone(), "material.json").unwrap();
        store.write_bytes(b"previous", 64).unwrap();

        let result = store.write_bytes_with_directory_sync(b"candidate", 64, |point, _| {
            if point == DirectorySyncPoint::AfterReplace {
                Err(std::io::Error::other("simulated directory sync failure"))
            } else {
                Ok(())
            }
        });

        assert_eq!(result, Err(PrivateConfigError::WriteFailed));
        assert_eq!(store.read_bytes(64).unwrap(), b"previous");
        let names = fs::read_dir(store.file.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["material.json"]);
        let _ = fs::remove_dir_all(home);
    }
}
