use super::MaterialConfiguration;
use crate::private_config::{PrivateConfigError, PrivateConfigFile};
use std::path::PathBuf;

#[cfg(test)]
use std::path::Path;

#[cfg(test)]
use std::{fs, os::unix::fs::PermissionsExt, time::SystemTime};

pub const MAX_MATERIAL_CONFIGURATION_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialPersistenceError {
    Missing,
    Invalid,
    Insecure,
    Unavailable,
    WriteFailed,
}

impl std::fmt::Display for MaterialPersistenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "material configuration is missing",
            Self::Invalid => "material configuration is invalid",
            Self::Insecure => "material configuration is insecure",
            Self::Unavailable => "material configuration persistence is unavailable",
            Self::WriteFailed => "material configuration could not be saved",
        })
    }
}

impl std::error::Error for MaterialPersistenceError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialConfigurationStore {
    file: PrivateConfigFile,
}

impl MaterialConfigurationStore {
    pub fn from_environment() -> Result<Self, MaterialPersistenceError> {
        PrivateConfigFile::from_environment("material.json")
            .map(|file| Self { file })
            .map_err(map_private_error)
    }

    pub fn new(config_home: PathBuf) -> Result<Self, MaterialPersistenceError> {
        PrivateConfigFile::new(config_home, "material.json")
            .map(|file| Self { file })
            .map_err(map_private_error)
    }

    pub fn unavailable() -> Self {
        Self {
            file: PrivateConfigFile::unavailable("material.json"),
        }
    }

    #[cfg(test)]
    pub(crate) fn configuration_file(&self) -> &Path {
        self.file.path()
    }

    pub fn read(&self) -> Result<MaterialConfiguration, MaterialPersistenceError> {
        let bytes = self
            .file
            .read_bytes(MAX_MATERIAL_CONFIGURATION_BYTES)
            .map_err(map_private_error)?;
        let configuration: MaterialConfiguration =
            serde_json::from_slice(&bytes).map_err(|_| MaterialPersistenceError::Invalid)?;
        configuration
            .validate()
            .map_err(|_| MaterialPersistenceError::Invalid)?;
        Ok(configuration)
    }

    pub fn read_or_default(&self) -> MaterialConfiguration {
        self.read().unwrap_or_default()
    }

    pub fn write(
        &self,
        configuration: &MaterialConfiguration,
    ) -> Result<(), MaterialPersistenceError> {
        configuration
            .validate()
            .map_err(|_| MaterialPersistenceError::Invalid)?;
        let document =
            serde_json::to_vec(configuration).map_err(|_| MaterialPersistenceError::WriteFailed)?;
        self.file
            .write_bytes(&document, MAX_MATERIAL_CONFIGURATION_BYTES)
            .map_err(map_private_error)
    }
}

fn map_private_error(error: PrivateConfigError) -> MaterialPersistenceError {
    match error {
        PrivateConfigError::Missing => MaterialPersistenceError::Missing,
        PrivateConfigError::Invalid => MaterialPersistenceError::Invalid,
        PrivateConfigError::Insecure => MaterialPersistenceError::Insecure,
        PrivateConfigError::WriteFailed => MaterialPersistenceError::WriteFailed,
    }
}

#[cfg(test)]
fn temp_config_home() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "typhon-material-store-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[cfg(test)]
fn create_private_configuration_directories(store: &MaterialConfigurationStore) {
    let astrea = store
        .configuration_file()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let typhon = store.configuration_file().parent().unwrap();
    fs::create_dir_all(typhon).unwrap();
    fs::set_permissions(astrea, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(typhon, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn missing_material_configuration_uses_the_builtin_default() {
    let home = temp_config_home();
    let store = MaterialConfigurationStore::new(home.clone()).unwrap();

    assert_eq!(store.read(), Err(MaterialPersistenceError::Missing));
    assert_eq!(store.read_or_default(), MaterialConfiguration::default());
    let _ = fs::remove_dir_all(home);
}

#[test]
fn valid_material_configuration_round_trips_and_is_user_private() {
    let home = temp_config_home();
    let store = MaterialConfigurationStore::new(home.clone()).unwrap();
    let configuration = MaterialConfiguration {
        position: 0.81,
        ..MaterialConfiguration::default()
    };

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
    for directory in [home.join("AstreaOS"), home.join("AstreaOS/typhon")] {
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let _ = fs::remove_dir_all(home);
}

#[test]
fn unsupported_versions_and_malformed_json_are_rejected() {
    let home = temp_config_home();
    let store = MaterialConfigurationStore::new(home.clone()).unwrap();
    create_private_configuration_directories(&store);

    fs::write(
        store.configuration_file(),
        br#"{"version":2,"position":0.5,"overrides":{}}"#,
    )
    .unwrap();
    fs::set_permissions(
        store.configuration_file(),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(store.read(), Err(MaterialPersistenceError::Invalid));

    fs::write(store.configuration_file(), b"not json").unwrap();
    assert_eq!(store.read(), Err(MaterialPersistenceError::Invalid));
    let _ = fs::remove_dir_all(home);
}

#[test]
fn oversized_material_configuration_is_rejected() {
    let home = temp_config_home();
    let store = MaterialConfigurationStore::new(home.clone()).unwrap();
    create_private_configuration_directories(&store);
    fs::write(store.configuration_file(), vec![b' '; 16 * 1024 + 1]).unwrap();
    fs::set_permissions(
        store.configuration_file(),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();

    assert_eq!(store.read(), Err(MaterialPersistenceError::Invalid));
    let _ = fs::remove_dir_all(home);
}

#[cfg(unix)]
#[test]
fn symlinked_material_file_is_rejected() {
    use std::os::unix::fs::symlink;

    let home = temp_config_home();
    let store = MaterialConfigurationStore::new(home.clone()).unwrap();
    create_private_configuration_directories(&store);
    let target = home.join("target.json");
    fs::write(&target, b"{}").unwrap();
    symlink(target, store.configuration_file()).unwrap();

    assert_eq!(store.read(), Err(MaterialPersistenceError::Insecure));
    assert_eq!(
        store.write(&MaterialConfiguration::default()),
        Err(MaterialPersistenceError::Insecure)
    );
    let _ = fs::remove_dir_all(home);
}

#[test]
fn failed_write_keeps_the_previous_valid_file_and_atomic_success_leaves_no_temp_file() {
    let home = temp_config_home();
    let store = MaterialConfigurationStore::new(home.clone()).unwrap();
    let previous = MaterialConfiguration {
        position: 0.21,
        ..MaterialConfiguration::default()
    };
    store.write(&previous).unwrap();
    let old_bytes = fs::read(store.configuration_file()).unwrap();
    fs::set_permissions(
        store.configuration_file(),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(
        store
            .write(&MaterialConfiguration {
                position: 0.93,
                ..MaterialConfiguration::default()
            })
            .is_err()
    );
    assert_eq!(fs::read(store.configuration_file()).unwrap(), old_bytes);

    fs::set_permissions(
        store.configuration_file(),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let next = MaterialConfiguration {
        position: 0.93,
        ..MaterialConfiguration::default()
    };
    store.write(&next).unwrap();
    assert_eq!(store.read().unwrap(), next);
    let names = fs::read_dir(store.configuration_file().parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["material.json"]);

    let restarted = MaterialConfigurationStore::new(home.clone()).unwrap();
    assert_eq!(restarted.read().unwrap(), next);
    let _ = fs::remove_dir_all(home);
}
