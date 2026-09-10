//! Bounded atomic persistence for animation user intent.

use super::config::{AnimationConfiguration, AnimationConfigurationDocument};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const ASTREA_DIRECTORY: &str = "AstreaOS";
const TYPHON_DIRECTORY: &str = "typhon";
const CONFIGURATION_FILE: &str = "animations.json";
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
pub const MAX_DOCUMENT_BYTES: usize = 16 * 1024;
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationPersistenceError { Missing, Invalid, Insecure, Unavailable, WriteFailed }

impl std::fmt::Display for AnimationPersistenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "animation configuration is missing",
            Self::Invalid => "animation configuration is invalid",
            Self::Insecure => "animation configuration is insecure",
            Self::Unavailable => "animation configuration persistence is unavailable",
            Self::WriteFailed => "animation configuration could not be saved",
        })
    }
}
impl std::error::Error for AnimationPersistenceError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimationConfigurationStore {
    config_home: PathBuf,
    configuration_directory: PathBuf,
    configuration_file: PathBuf,
    create_missing_config_home: bool,
    unavailable: Option<AnimationPersistenceError>,
}

impl AnimationConfigurationStore {
    pub fn from_environment() -> Result<Self, AnimationPersistenceError> {
        let (config_home, create_missing_config_home) = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(value) if !value.is_empty() => (PathBuf::from(value), false),
            _ => {
                let home = std::env::var_os("HOME").ok_or(AnimationPersistenceError::Insecure)?;
                (PathBuf::from(home).join(".config"), true)
            }
        };
        Self::new_with_policy(config_home, create_missing_config_home)
    }

    pub fn new(config_home: PathBuf) -> Result<Self, AnimationPersistenceError> {
        Self::new_with_policy(config_home, false)
    }

    pub fn unavailable(error: AnimationPersistenceError) -> Self {
        Self {
            config_home: PathBuf::from("/.invalid"),
            configuration_directory: PathBuf::from("/.invalid/AstreaOS/typhon"),
            configuration_file: PathBuf::from("/.invalid/AstreaOS/typhon/animations.json"),
            create_missing_config_home: false,
            unavailable: Some(error),
        }
    }

    fn new_with_policy(config_home: PathBuf, create_missing_config_home: bool) -> Result<Self, AnimationPersistenceError> {
        if !config_home.is_absolute() { return Err(AnimationPersistenceError::Insecure); }
        let configuration_directory = config_home.join(ASTREA_DIRECTORY).join(TYPHON_DIRECTORY);
        Ok(Self {
            config_home,
            configuration_file: configuration_directory.join(CONFIGURATION_FILE),
            configuration_directory,
            create_missing_config_home,
            unavailable: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn configuration_file(&self) -> &Path { &self.configuration_file }

    pub fn read(&self) -> Result<AnimationConfiguration, AnimationPersistenceError> {
        self.check_available()?;
        let metadata = match fs::symlink_metadata(&self.configuration_file) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(AnimationPersistenceError::Missing),
            Err(_) => return Err(AnimationPersistenceError::Insecure),
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file()
            || metadata.permissions().mode() & 0o777 != PRIVATE_FILE_MODE {
            return Err(AnimationPersistenceError::Insecure);
        }
        if metadata.len() > MAX_DOCUMENT_BYTES as u64 { return Err(AnimationPersistenceError::Invalid); }
        let mut file = File::open(&self.configuration_file).map_err(|_| AnimationPersistenceError::Insecure)?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut bytes).map_err(|_| AnimationPersistenceError::Invalid)?;
        let document: AnimationConfigurationDocument = serde_json::from_slice(&bytes).map_err(|_| AnimationPersistenceError::Invalid)?;
        AnimationConfiguration::from_document(document).map_err(|_| AnimationPersistenceError::Invalid)
    }

    pub fn write(&self, configuration: &AnimationConfiguration) -> Result<(), AnimationPersistenceError> {
        self.check_available()?;
        configuration.validate().map_err(|_| AnimationPersistenceError::Invalid)?;
        let document = serde_json::to_vec(&configuration.to_document()).map_err(|_| AnimationPersistenceError::WriteFailed)?;
        if document.len() > MAX_DOCUMENT_BYTES { return Err(AnimationPersistenceError::Invalid); }
        self.open_directories_for_write()?;
        if let Ok(metadata) = fs::symlink_metadata(&self.configuration_file)
            && (metadata.file_type().is_symlink() || !metadata.file_type().is_file()
                || metadata.permissions().mode() & 0o777 != PRIVATE_FILE_MODE) {
            return Err(AnimationPersistenceError::Insecure);
        }
        let temporary = self.configuration_directory.join(format!(
            ".animations.json.tmp-{}-{}", std::process::id(), NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = OpenOptions::new().read(true).write(true).create_new(true)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW).mode(PRIVATE_FILE_MODE)
                .open(&temporary).map_err(|_| AnimationPersistenceError::WriteFailed)?;
            file.write_all(&document).map_err(|_| AnimationPersistenceError::WriteFailed)?;
            file.flush().map_err(|_| AnimationPersistenceError::WriteFailed)?;
            file.sync_all().map_err(|_| AnimationPersistenceError::WriteFailed)?;
            drop(file);
            fs::rename(&temporary, &self.configuration_file).map_err(|_| AnimationPersistenceError::WriteFailed)?;
            File::open(&self.configuration_directory).and_then(|directory| directory.sync_all())
                .map_err(|_| AnimationPersistenceError::WriteFailed)
        })();
        if result.is_err() { let _ = fs::remove_file(&temporary); }
        result
    }

    fn check_available(&self) -> Result<(), AnimationPersistenceError> { self.unavailable.map_or(Ok(()), Err) }

    fn open_directories_for_write(&self) -> Result<(), AnimationPersistenceError> {
        if !self.config_home.exists() {
            if !self.create_missing_config_home { return Err(AnimationPersistenceError::WriteFailed); }
            fs::create_dir_all(&self.config_home).map_err(|_| AnimationPersistenceError::WriteFailed)?;
            fs::set_permissions(&self.config_home, fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE)).map_err(|_| AnimationPersistenceError::WriteFailed)?;
        }
        validate_directory(&self.config_home, false)?;
        let astrea = self.config_home.join(ASTREA_DIRECTORY);
        fs::create_dir_all(&astrea).map_err(|_| AnimationPersistenceError::WriteFailed)?;
        validate_directory(&astrea, true)?;
        fs::create_dir_all(&self.configuration_directory).map_err(|_| AnimationPersistenceError::WriteFailed)?;
        validate_directory(&self.configuration_directory, true)
    }
}

fn validate_directory(path: &Path, require_private: bool) -> Result<(), AnimationPersistenceError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| AnimationPersistenceError::Insecure)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir()
        || (require_private && metadata.permissions().mode() & 0o777 != PRIVATE_DIRECTORY_MODE) {
        return Err(AnimationPersistenceError::Insecure);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, time::{SystemTime, UNIX_EPOCH}};

    fn temp_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!("typhon-animation-persistence-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn missing_file_is_safe_and_defaults_are_available_to_the_caller() {
        let directory = temp_directory();
        let store = AnimationConfigurationStore::new(directory.clone()).unwrap();
        assert_eq!(store.read(), Err(AnimationPersistenceError::Missing));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn valid_configuration_round_trips_with_private_file_mode() {
        let directory = temp_directory();
        let store = AnimationConfigurationStore::new(directory.clone()).unwrap();
        let configuration = AnimationConfiguration::default();
        store.write(&configuration).unwrap();
        assert_eq!(store.read().unwrap(), configuration);
        assert_eq!(fs::metadata(store.configuration_file()).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn malformed_and_unsupported_documents_fall_back_safely() {
        let directory = temp_directory();
        let store = AnimationConfigurationStore::new(directory.clone()).unwrap();
        store.write(&AnimationConfiguration::default()).unwrap();
        fs::write(store.configuration_file(), b"not-json").unwrap();
        fs::set_permissions(store.configuration_file(), fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(store.read(), Err(AnimationPersistenceError::Invalid));
        let value = serde_json::json!({"version": 99, "enabled": true, "preset": "astrea", "speed": 1.0, "overrides": {}});
        fs::write(store.configuration_file(), serde_json::to_vec(&value).unwrap()).unwrap();
        fs::set_permissions(store.configuration_file(), fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(store.read(), Err(AnimationPersistenceError::Invalid));
        let _ = fs::remove_dir_all(directory);
    }
}
