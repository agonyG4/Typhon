//! Bounded atomic persistence for animation user intent.

use super::config::{AnimationConfiguration, AnimationConfigurationDocument};
use crate::private_config::{PrivateConfigError, PrivateConfigFile};
use std::path::PathBuf;

#[cfg(test)]
use std::{fs, path::Path};

const CONFIGURATION_FILE: &str = "animations.json";
pub const MAX_DOCUMENT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationPersistenceError {
    Missing,
    Invalid,
    Insecure,
    Unavailable,
    WriteFailed,
}

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
    file: PrivateConfigFile,
    unavailable: Option<AnimationPersistenceError>,
}

impl AnimationConfigurationStore {
    pub fn from_environment() -> Result<Self, AnimationPersistenceError> {
        PrivateConfigFile::from_environment(CONFIGURATION_FILE)
            .map(|file| Self {
                file,
                unavailable: None,
            })
            .map_err(map_private_error)
    }

    pub fn new(config_home: PathBuf) -> Result<Self, AnimationPersistenceError> {
        PrivateConfigFile::new(config_home, CONFIGURATION_FILE)
            .map(|file| Self {
                file,
                unavailable: None,
            })
            .map_err(map_private_error)
    }

    pub fn unavailable(error: AnimationPersistenceError) -> Self {
        Self {
            file: PrivateConfigFile::unavailable(CONFIGURATION_FILE),
            unavailable: Some(error),
        }
    }

    #[cfg(test)]
    pub(crate) fn configuration_file(&self) -> &Path {
        self.file.path()
    }

    pub fn read(&self) -> Result<AnimationConfiguration, AnimationPersistenceError> {
        self.check_available()?;
        let bytes = self
            .file
            .read_bytes(MAX_DOCUMENT_BYTES)
            .map_err(map_private_error)?;
        let document: AnimationConfigurationDocument =
            serde_json::from_slice(&bytes).map_err(|_| AnimationPersistenceError::Invalid)?;
        AnimationConfiguration::from_document(document)
            .map_err(|_| AnimationPersistenceError::Invalid)
    }

    pub fn write(
        &self,
        configuration: &AnimationConfiguration,
    ) -> Result<(), AnimationPersistenceError> {
        self.check_available()?;
        configuration
            .validate()
            .map_err(|_| AnimationPersistenceError::Invalid)?;
        let document = serde_json::to_vec(&configuration.to_document())
            .map_err(|_| AnimationPersistenceError::WriteFailed)?;
        if document.len() > MAX_DOCUMENT_BYTES {
            return Err(AnimationPersistenceError::Invalid);
        }
        self.file
            .write_bytes(&document, MAX_DOCUMENT_BYTES)
            .map_err(map_private_error)
    }

    fn check_available(&self) -> Result<(), AnimationPersistenceError> {
        self.unavailable.map_or(Ok(()), Err)
    }
}

fn map_private_error(error: PrivateConfigError) -> AnimationPersistenceError {
    match error {
        PrivateConfigError::Missing => AnimationPersistenceError::Missing,
        PrivateConfigError::Invalid => AnimationPersistenceError::Invalid,
        PrivateConfigError::Insecure => AnimationPersistenceError::Insecure,
        PrivateConfigError::WriteFailed => AnimationPersistenceError::WriteFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::PermissionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "typhon-animation-persistence-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
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
        assert_eq!(
            fs::metadata(store.configuration_file())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn malformed_and_unsupported_documents_fall_back_safely() {
        let directory = temp_directory();
        let store = AnimationConfigurationStore::new(directory.clone()).unwrap();
        store.write(&AnimationConfiguration::default()).unwrap();
        fs::write(store.configuration_file(), b"not-json").unwrap();
        fs::set_permissions(
            store.configuration_file(),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert_eq!(store.read(), Err(AnimationPersistenceError::Invalid));
        let value = serde_json::json!({"version": 99, "enabled": true, "preset": "astrea", "speed": 1.0, "overrides": {}});
        fs::write(
            store.configuration_file(),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        fs::set_permissions(
            store.configuration_file(),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert_eq!(store.read(), Err(AnimationPersistenceError::Invalid));
        let _ = fs::remove_dir_all(directory);
    }
}
