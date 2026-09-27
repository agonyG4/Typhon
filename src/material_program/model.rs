use serde::{Deserialize, Serialize};

pub const MATERIAL_PROGRAM_CONFIGURATION_VERSION: u8 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialProgramConfiguration {
    pub version: u8,
    pub requested_program: String,
}

impl Default for MaterialProgramConfiguration {
    fn default() -> Self {
        Self {
            version: MATERIAL_PROGRAM_CONFIGURATION_VERSION,
            requested_program: crate::effects::BUILTIN_BACKGROUND_BLUR_NAME.to_owned(),
        }
    }
}

impl MaterialProgramConfiguration {
    pub fn validate(&self) -> Result<(), MaterialProgramConfigurationError> {
        if self.version != MATERIAL_PROGRAM_CONFIGURATION_VERSION {
            return Err(MaterialProgramConfigurationError::UnsupportedVersion(
                self.version,
            ));
        }
        crate::effects::config::validate_effect_name(&self.requested_program)
            .map_err(|_| MaterialProgramConfigurationError::InvalidProgramName)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialProgramConfigurationError {
    UnsupportedVersion(u8),
    InvalidProgramName,
}
