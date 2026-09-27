use crate::private_config::{PrivateConfigError, PrivateConfigFile};
use std::path::PathBuf;

use super::MaterialProgramConfiguration;

pub const MATERIAL_PROGRAM_CONFIGURATION_FILE_NAME: &str = "material-program.json";
pub const MAX_MATERIAL_PROGRAM_CONFIGURATION_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialProgramPersistenceError {
    Missing,
    Invalid,
    Insecure,
    Unavailable,
    WriteFailed,
}

impl std::fmt::Display for MaterialProgramPersistenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "material program configuration is missing",
            Self::Invalid => "material program configuration is invalid",
            Self::Insecure => "material program configuration is insecure",
            Self::Unavailable => "material program configuration persistence is unavailable",
            Self::WriteFailed => "material program configuration could not be saved",
        })
    }
}

impl std::error::Error for MaterialProgramPersistenceError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialProgramConfigurationStore {
    file: PrivateConfigFile,
}

impl MaterialProgramConfigurationStore {
    pub fn from_environment() -> Result<Self, MaterialProgramPersistenceError> {
        PrivateConfigFile::from_environment(MATERIAL_PROGRAM_CONFIGURATION_FILE_NAME)
            .map(|file| Self { file })
            .map_err(map_private_error)
    }

    pub fn new(config_home: PathBuf) -> Result<Self, MaterialProgramPersistenceError> {
        PrivateConfigFile::new(config_home, MATERIAL_PROGRAM_CONFIGURATION_FILE_NAME)
            .map(|file| Self { file })
            .map_err(map_private_error)
    }

    pub fn unavailable() -> Self {
        Self {
            file: PrivateConfigFile::unavailable(MATERIAL_PROGRAM_CONFIGURATION_FILE_NAME),
        }
    }

    #[cfg(test)]
    pub(crate) fn configuration_file(&self) -> &std::path::Path {
        self.file.path()
    }

    pub fn read(&self) -> Result<MaterialProgramConfiguration, MaterialProgramPersistenceError> {
        let bytes = self
            .file
            .read_bytes(MAX_MATERIAL_PROGRAM_CONFIGURATION_BYTES)
            .map_err(map_private_error)?;
        let configuration: MaterialProgramConfiguration =
            serde_json::from_slice(&bytes).map_err(|_| MaterialProgramPersistenceError::Invalid)?;
        configuration
            .validate()
            .map_err(|_| MaterialProgramPersistenceError::Invalid)?;
        Ok(configuration)
    }

    pub fn write(
        &self,
        configuration: &MaterialProgramConfiguration,
    ) -> Result<(), MaterialProgramPersistenceError> {
        configuration
            .validate()
            .map_err(|_| MaterialProgramPersistenceError::Invalid)?;
        let bytes = serde_json::to_vec(configuration)
            .map_err(|_| MaterialProgramPersistenceError::WriteFailed)?;
        self.file
            .write_bytes(&bytes, MAX_MATERIAL_PROGRAM_CONFIGURATION_BYTES)
            .map_err(map_private_error)
    }
}

fn map_private_error(error: PrivateConfigError) -> MaterialProgramPersistenceError {
    match error {
        PrivateConfigError::Missing => MaterialProgramPersistenceError::Missing,
        PrivateConfigError::Invalid => MaterialProgramPersistenceError::Invalid,
        PrivateConfigError::Insecure => MaterialProgramPersistenceError::Insecure,
        PrivateConfigError::WriteFailed => MaterialProgramPersistenceError::WriteFailed,
    }
}
