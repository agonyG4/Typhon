use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::effects::{MAX_EFFECT_PARAMETERS, MAX_EFFECT_PROGRAMS, config::validate_effect_name};
use crate::private_config::{PrivateConfigError, PrivateConfigFile};

use super::MaterialProgramParameterValue;

pub const MATERIAL_PROGRAM_PARAMETERS_FILE_NAME: &str = "material-program-parameters.json";
pub const MATERIAL_PROGRAM_PARAMETER_DOCUMENT_VERSION: u8 = 1;
pub const MAX_MATERIAL_PROGRAM_PARAMETER_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialProgramParameterPersistenceDocument {
    pub version: u8,
    pub programs: BTreeMap<String, MaterialProgramParameterProgramIntent>,
}

impl Default for MaterialProgramParameterPersistenceDocument {
    fn default() -> Self {
        Self {
            version: MATERIAL_PROGRAM_PARAMETER_DOCUMENT_VERSION,
            programs: BTreeMap::new(),
        }
    }
}

impl MaterialProgramParameterPersistenceDocument {
    pub fn validate(&self) -> Result<(), MaterialProgramParameterPersistenceError> {
        if self.version != MATERIAL_PROGRAM_PARAMETER_DOCUMENT_VERSION {
            return Err(MaterialProgramParameterPersistenceError::Invalid);
        }
        if self.programs.len() > MAX_EFFECT_PROGRAMS {
            return Err(MaterialProgramParameterPersistenceError::Invalid);
        }
        for (program, intent) in &self.programs {
            validate_effect_name(program)
                .map_err(|_| MaterialProgramParameterPersistenceError::Invalid)?;
            if intent.overrides.len() > MAX_EFFECT_PARAMETERS {
                return Err(MaterialProgramParameterPersistenceError::Invalid);
            }
            for (name, value) in &intent.overrides {
                validate_effect_name(name)
                    .map_err(|_| MaterialProgramParameterPersistenceError::Invalid)?;
                value
                    .to_effect_uniform_value()
                    .map_err(|_| MaterialProgramParameterPersistenceError::Invalid)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialProgramParameterProgramIntent {
    pub schema_signature: u64,
    pub overrides: BTreeMap<String, MaterialProgramParameterValue>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialProgramParameterPersistenceError {
    Missing,
    Invalid,
    Insecure,
    WriteFailed,
}

impl std::fmt::Display for MaterialProgramParameterPersistenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "material program parameter configuration is missing",
            Self::Invalid => "material program parameter configuration is invalid",
            Self::Insecure => "material program parameter configuration is insecure",
            Self::WriteFailed => "material program parameter configuration could not be saved",
        })
    }
}

impl std::error::Error for MaterialProgramParameterPersistenceError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialProgramParameterConfigurationStore {
    file: PrivateConfigFile,
}

impl MaterialProgramParameterConfigurationStore {
    pub fn from_environment() -> Result<Self, MaterialProgramParameterPersistenceError> {
        PrivateConfigFile::from_environment(MATERIAL_PROGRAM_PARAMETERS_FILE_NAME)
            .map(|file| Self { file })
            .map_err(map_private_error)
    }

    pub fn new(config_home: PathBuf) -> Result<Self, MaterialProgramParameterPersistenceError> {
        PrivateConfigFile::new(config_home, MATERIAL_PROGRAM_PARAMETERS_FILE_NAME)
            .map(|file| Self { file })
            .map_err(map_private_error)
    }

    pub fn unavailable() -> Self {
        Self {
            file: PrivateConfigFile::unavailable(MATERIAL_PROGRAM_PARAMETERS_FILE_NAME),
        }
    }

    pub fn read(
        &self,
    ) -> Result<MaterialProgramParameterPersistenceDocument, MaterialProgramParameterPersistenceError>
    {
        let bytes = self
            .file
            .read_bytes(MAX_MATERIAL_PROGRAM_PARAMETER_DOCUMENT_BYTES)
            .map_err(map_private_error)?;
        let document: MaterialProgramParameterPersistenceDocument = serde_json::from_slice(&bytes)
            .map_err(|_| MaterialProgramParameterPersistenceError::Invalid)?;
        document.validate()?;
        Ok(document)
    }

    pub fn write(
        &self,
        document: &MaterialProgramParameterPersistenceDocument,
    ) -> Result<(), MaterialProgramParameterPersistenceError> {
        document.validate()?;
        let bytes = serde_json::to_vec(document)
            .map_err(|_| MaterialProgramParameterPersistenceError::WriteFailed)?;
        self.file
            .write_bytes(&bytes, MAX_MATERIAL_PROGRAM_PARAMETER_DOCUMENT_BYTES)
            .map_err(map_private_error)
    }
}

fn map_private_error(error: PrivateConfigError) -> MaterialProgramParameterPersistenceError {
    match error {
        PrivateConfigError::Missing => MaterialProgramParameterPersistenceError::Missing,
        PrivateConfigError::Invalid => MaterialProgramParameterPersistenceError::Invalid,
        PrivateConfigError::Insecure => MaterialProgramParameterPersistenceError::Insecure,
        PrivateConfigError::WriteFailed => MaterialProgramParameterPersistenceError::WriteFailed,
    }
}
