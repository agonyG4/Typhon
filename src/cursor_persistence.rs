//! Secure persistence for the compositor-owned cursor configuration.

use crate::cursor_theme::CursorConfiguration;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const ASTREA_DIRECTORY: &str = "AstreaOS";
const INPUT_DIRECTORY: &str = "input";
const CONFIGURATION_FILE: &str = "cursor.json";
const TEMP_PREFIX: &str = ".cursor.json.tmp-";
const TEMP_ATTEMPTS: usize = 8;
const MAX_DOCUMENT_BYTES: u64 = 4096;
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorPersistenceError {
    Missing,
    Invalid,
    Insecure,
    WriteFailed,
}

impl std::fmt::Display for CursorPersistenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "cursor configuration is missing",
            Self::Invalid => "cursor configuration is invalid",
            Self::Insecure => "cursor configuration is insecure",
            Self::WriteFailed => "cursor configuration could not be saved",
        })
    }
}

impl std::error::Error for CursorPersistenceError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorConfigurationStore {
    config_home: PathBuf,
    astrea_directory: PathBuf,
    input_directory: PathBuf,
    configuration_file: PathBuf,
    unavailable: Option<CursorPersistenceError>,
}

impl CursorConfigurationStore {
    pub fn from_environment() -> Result<Self, CursorPersistenceError> {
        let config_home = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(value) if !value.is_empty() => PathBuf::from(value),
            _ => {
                let home = std::env::var_os("HOME")
                    .ok_or(CursorPersistenceError::Insecure)
                    .map(PathBuf::from)?;
                home.join(".config")
            }
        };
        Self::new(config_home)
    }

    pub fn unavailable(error: CursorPersistenceError) -> Self {
        Self {
            config_home: PathBuf::from("/"),
            astrea_directory: PathBuf::from("/.invalid/AstreaOS"),
            input_directory: PathBuf::from("/.invalid/AstreaOS/input"),
            configuration_file: PathBuf::from("/.invalid/AstreaOS/input/cursor.json"),
            unavailable: Some(error),
        }
    }

    pub fn new(config_home: PathBuf) -> Result<Self, CursorPersistenceError> {
        if !config_home.is_absolute() {
            return Err(CursorPersistenceError::Insecure);
        }
        let astrea_directory = config_home.join(ASTREA_DIRECTORY);
        let input_directory = astrea_directory.join(INPUT_DIRECTORY);
        let configuration_file = input_directory.join(CONFIGURATION_FILE);
        Ok(Self {
            config_home,
            astrea_directory,
            input_directory,
            configuration_file,
            unavailable: None,
        })
    }

    pub fn configuration_file(&self) -> &Path {
        &self.configuration_file
    }

    pub fn read(&self) -> Result<CursorConfiguration, CursorPersistenceError> {
        if let Some(error) = self.unavailable {
            return Err(error);
        }
        self.validate_config_home(false)?;
        self.validate_directory(&self.astrea_directory, false)?;
        self.validate_directory(&self.input_directory, false)?;
        self.validate_configuration_file()?;

        let metadata =
            fs::symlink_metadata(&self.configuration_file).map_err(classify_read_metadata_error)?;
        if metadata.len() > MAX_DOCUMENT_BYTES {
            return Err(CursorPersistenceError::Invalid);
        }
        let mut file =
            File::open(&self.configuration_file).map_err(|_| CursorPersistenceError::Invalid)?;
        let mut document = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut document)
            .map_err(|_| CursorPersistenceError::Invalid)?;
        let persisted: PersistedCursorConfiguration =
            serde_json::from_slice(&document).map_err(|_| CursorPersistenceError::Invalid)?;
        if persisted.version != 1 {
            return Err(CursorPersistenceError::Invalid);
        }
        CursorConfiguration::new(persisted.theme, persisted.size_px)
            .map_err(|_| CursorPersistenceError::Invalid)
    }

    pub fn write(&self, configuration: &CursorConfiguration) -> Result<(), CursorPersistenceError> {
        if let Some(error) = self.unavailable {
            return Err(error);
        }
        CursorConfiguration::new(&configuration.theme, configuration.size_px)
            .map_err(|_| CursorPersistenceError::Invalid)?;
        self.validate_config_home(true)?;
        self.ensure_directory(&self.astrea_directory)?;
        self.ensure_directory(&self.input_directory)?;
        if self.configuration_file.exists() || self.configuration_file.is_symlink() {
            self.validate_configuration_file()
                .map_err(|_| CursorPersistenceError::Insecure)?;
        }

        let document = serde_json::to_vec(&PersistedCursorConfiguration {
            version: 1,
            theme: configuration.theme.clone(),
            size_px: configuration.size_px,
        })
        .map_err(|_| CursorPersistenceError::WriteFailed)?;

        let mut temporary = None;
        let mut file = None;
        for _ in 0..TEMP_ATTEMPTS {
            let candidate = self.input_directory.join(format!(
                "{TEMP_PREFIX}{}",
                random_suffix().map_err(|_| CursorPersistenceError::WriteFailed)?
            ));
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(PRIVATE_FILE_MODE)
                .open(&candidate)
            {
                Ok(created) => {
                    temporary = Some(candidate);
                    file = Some(created);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(CursorPersistenceError::WriteFailed),
            }
        }
        let temporary = temporary.ok_or(CursorPersistenceError::WriteFailed)?;
        let mut file = file.expect("temporary file exists with its path");
        let write_result = (|| {
            file.write_all(&document)
                .map_err(|_| CursorPersistenceError::WriteFailed)?;
            file.flush()
                .map_err(|_| CursorPersistenceError::WriteFailed)?;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(PRIVATE_FILE_MODE))
                .map_err(|_| CursorPersistenceError::WriteFailed)?;
            file.sync_all()
                .map_err(|_| CursorPersistenceError::WriteFailed)?;
            verify_regular_owned_file(&temporary, PRIVATE_FILE_MODE)?;
            drop(file);
            fs::rename(&temporary, &self.configuration_file)
                .map_err(|_| CursorPersistenceError::WriteFailed)?;
            sync_directory(&self.input_directory)?;
            verify_regular_owned_file(&self.configuration_file, PRIVATE_FILE_MODE)
        })();
        if write_result.is_err() && temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        write_result
    }

    fn validate_config_home(&self, for_write: bool) -> Result<(), CursorPersistenceError> {
        match fs::symlink_metadata(&self.config_home) {
            Ok(metadata) => {
                if !metadata.file_type().is_dir() || metadata.uid() != effective_uid() {
                    Err(CursorPersistenceError::Insecure)
                } else {
                    Ok(())
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && for_write => {
                Err(CursorPersistenceError::WriteFailed)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(CursorPersistenceError::Missing)
            }
            Err(_) => Err(CursorPersistenceError::Insecure),
        }
    }

    fn validate_directory(
        &self,
        path: &Path,
        for_write: bool,
    ) -> Result<(), CursorPersistenceError> {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if !metadata.file_type().is_dir()
                    || !owned_with_mode(
                        metadata.uid(),
                        metadata.mode(),
                        effective_uid(),
                        Some(PRIVATE_DIRECTORY_MODE),
                    )
                {
                    Err(CursorPersistenceError::Insecure)
                } else {
                    Ok(())
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && !for_write => {
                Err(CursorPersistenceError::Missing)
            }
            Err(_) => Err(if for_write {
                CursorPersistenceError::WriteFailed
            } else {
                CursorPersistenceError::Insecure
            }),
        }
    }

    fn ensure_directory(&self, path: &Path) -> Result<(), CursorPersistenceError> {
        match fs::symlink_metadata(path) {
            Ok(_) => self.validate_directory(path, true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(path).map_err(|_| CursorPersistenceError::WriteFailed)?;
                fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE))
                    .map_err(|_| CursorPersistenceError::WriteFailed)?;
                self.validate_directory(path, true)
            }
            Err(_) => Err(CursorPersistenceError::WriteFailed),
        }
    }

    fn validate_configuration_file(&self) -> Result<(), CursorPersistenceError> {
        verify_regular_owned_file(&self.configuration_file, PRIVATE_FILE_MODE)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedCursorConfiguration {
    version: u32,
    theme: String,
    size_px: u32,
}

fn classify_read_metadata_error(error: io::Error) -> CursorPersistenceError {
    if error.kind() == io::ErrorKind::NotFound {
        CursorPersistenceError::Missing
    } else {
        CursorPersistenceError::Insecure
    }
}

fn verify_regular_owned_file(
    path: &Path,
    expected_mode: u32,
) -> Result<(), CursorPersistenceError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| CursorPersistenceError::Insecure)?;
    if !metadata.file_type().is_file()
        || !owned_with_mode(
            metadata.uid(),
            metadata.mode(),
            effective_uid(),
            Some(expected_mode),
        )
    {
        return Err(CursorPersistenceError::Insecure);
    }
    Ok(())
}

fn owned_with_mode(uid: u32, mode: u32, effective_uid: u32, exact_mode: Option<u32>) -> bool {
    uid == effective_uid
        && exact_mode.map_or(mode & 0o022 == 0, |expected| mode & 0o777 == expected)
}

fn sync_directory(path: &Path) -> Result<(), CursorPersistenceError> {
    match File::open(path).and_then(|directory| directory.sync_all()) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
            ) =>
        {
            Ok(())
        }
        Err(_) => Err(CursorPersistenceError::WriteFailed),
    }
}

fn random_suffix() -> io::Result<String> {
    let mut bytes = [0_u8; 16];
    loop {
        // SAFETY: the buffer is valid writable storage for exactly its length,
        // and `getrandom` does not retain the pointer after returning.
        let count = unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) };
        if count == bytes.len() as isize {
            break;
        }
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "short random suffix",
        ));
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn effective_uid() -> u32 {
    // SAFETY: `geteuid` has no pointer arguments and only reads process state.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistence_document_validation_uses_shared_cursor_validator() {
        assert!(CursorConfiguration::new("default", 8).is_ok());
        assert!(crate::cursor_theme::validate_cursor_theme("bad/theme").is_err());
        assert!(crate::cursor_theme::validate_cursor_size(7).is_err());
    }

    #[test]
    fn metadata_policy_requires_effective_uid_and_expected_mode() {
        assert!(owned_with_mode(1000, 0o700, 1000, Some(0o700)));
        assert!(!owned_with_mode(1001, 0o700, 1000, Some(0o700)));
        assert!(!owned_with_mode(1000, 0o755, 1000, Some(0o700)));
        assert!(owned_with_mode(1000, 0o755, 1000, None));
        assert!(!owned_with_mode(1000, 0o775, 1000, None));
    }
}
