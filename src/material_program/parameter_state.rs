use std::collections::BTreeMap;
use std::sync::Arc;

use crate::effects::{
    EffectParameterBlock, EffectRegistryGeneration, EffectUniformValue, EffectValidationError,
    RegisteredEffect, config::validate_effect_name, validate_parameter_value,
};

use super::{
    MaterialProgramDescriptionSnapshot, MaterialProgramOrigin,
    MaterialProgramParameterConfiguration, MaterialProgramParameterConfigurationSnapshot,
    MaterialProgramParameterConfigurationStore, MaterialProgramParameterDescriptor,
    MaterialProgramParameterPersistenceDocument, MaterialProgramParameterPersistenceError,
    MaterialProgramParameterProgramIntent, MaterialProgramParameterRange,
    MaterialProgramParameterValue, qualifies_registered_effect,
};

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialProgramParameterControlState {
    document: MaterialProgramParameterPersistenceDocument,
    store: MaterialProgramParameterConfigurationStore,
    parameter_generation: u64,
}

impl Default for MaterialProgramParameterControlState {
    fn default() -> Self {
        Self::from_environment()
    }
}

impl MaterialProgramParameterControlState {
    pub fn from_environment() -> Self {
        let store = MaterialProgramParameterConfigurationStore::from_environment()
            .unwrap_or_else(|_| MaterialProgramParameterConfigurationStore::unavailable());
        Self::from_store(store)
    }

    pub fn from_store(store: MaterialProgramParameterConfigurationStore) -> Self {
        let document = store
            .read()
            .unwrap_or_else(|_| MaterialProgramParameterPersistenceDocument::default());
        Self {
            document,
            store,
            parameter_generation: 0,
        }
    }

    pub fn generation(&self) -> u64 {
        self.parameter_generation
    }

    pub fn describe(
        &self,
        name: &str,
        registry_generation: &Arc<EffectRegistryGeneration>,
    ) -> Result<MaterialProgramDescriptionSnapshot, MaterialProgramParameterDescriptionError> {
        validate_effect_name(name)
            .map_err(|_| MaterialProgramParameterDescriptionError::InvalidName)?;

        let builtin_name = crate::effects::BUILTIN_BACKGROUND_BLUR_NAME;
        let Some(effect) = registry_generation.effects.get(name) else {
            return Err(MaterialProgramParameterDescriptionError::UnknownProgram);
        };
        if name != builtin_name && !qualifies_registered_effect(effect) {
            return Err(MaterialProgramParameterDescriptionError::Unqualified);
        }

        let valid_overrides = self.validated_overrides(effect);
        let mut parameters = effect
            .parameters
            .iter()
            .map(|(name, parameter)| {
                let default =
                    MaterialProgramParameterValue::from_effect_uniform_value(parameter.default)
                        .map_err(|_| {
                            MaterialProgramParameterDescriptionError::InvalidTrustedSchema
                        })?;
                let effective_value = valid_overrides
                    .as_ref()
                    .and_then(|overrides| overrides.get(name))
                    .copied()
                    .unwrap_or(parameter.default);
                let effective =
                    MaterialProgramParameterValue::from_effect_uniform_value(effective_value)
                        .map_err(|_| {
                            MaterialProgramParameterDescriptionError::InvalidTrustedSchema
                        })?;
                let range = MaterialProgramParameterRange::from_effect_range(parameter.spec.range)
                    .map_err(|_| MaterialProgramParameterDescriptionError::InvalidTrustedSchema)?;
                Ok(MaterialProgramParameterDescriptor {
                    name: name.clone(),
                    parameter_type: parameter.spec.ty.into(),
                    range,
                    default,
                    effective,
                    overridden: valid_overrides
                        .as_ref()
                        .is_some_and(|overrides| overrides.contains_key(name)),
                })
            })
            .collect::<Result<Vec<_>, MaterialProgramParameterDescriptionError>>()?;
        parameters.sort_by(|left, right| left.name.cmp(&right.name));

        Ok(MaterialProgramDescriptionSnapshot {
            registry_generation: registry_generation.generation,
            parameter_generation: self.parameter_generation,
            name: effect.name.clone(),
            origin: if name == builtin_name {
                MaterialProgramOrigin::System
            } else {
                MaterialProgramOrigin::TrustedLocal
            },
            schema_signature: effect.parameter_schema_signature(),
            parameters,
        })
    }

    pub fn set_configuration(
        &mut self,
        candidate: MaterialProgramParameterConfiguration,
        registry_generation: &Arc<EffectRegistryGeneration>,
    ) -> Result<MaterialProgramParameterUpdate, MaterialProgramParameterMutationError> {
        candidate
            .validate()
            .map_err(|_| MaterialProgramParameterMutationError::InvalidConfiguration)?;

        if self
            .document
            .programs
            .get(&candidate.program)
            .is_some_and(|intent| {
                intent.schema_signature == candidate.schema_signature
                    && intent.overrides == candidate.overrides
            })
        {
            return Ok(MaterialProgramParameterUpdate {
                snapshot: self.configuration_snapshot(candidate, registry_generation),
                changed: false,
            });
        }

        if !candidate.overrides.is_empty() {
            let Some(effect) = registry_generation.effects.get(&candidate.program) else {
                return Err(MaterialProgramParameterMutationError::UnknownProgram);
            };
            if !qualifies_registered_effect(effect) {
                return Err(MaterialProgramParameterMutationError::Unqualified);
            }
            if candidate.schema_signature != effect.parameter_schema_signature() {
                return Err(MaterialProgramParameterMutationError::StaleSchema);
            }
            for (name, value) in &candidate.overrides {
                let Some(parameter) = effect.parameters.get(name) else {
                    return Err(MaterialProgramParameterMutationError::UnknownParameter);
                };
                let value = value
                    .to_effect_uniform_value()
                    .map_err(|_| MaterialProgramParameterMutationError::InvalidParameterValue)?;
                validate_parameter_value(&parameter.spec, value)
                    .map_err(|_| MaterialProgramParameterMutationError::InvalidParameterValue)?;
            }
        }

        let next_generation = self
            .parameter_generation
            .checked_add(1)
            .ok_or(MaterialProgramParameterMutationError::GenerationExhausted)?;
        let mut next_document = self.document.clone();
        next_document.programs.insert(
            candidate.program.clone(),
            MaterialProgramParameterProgramIntent {
                schema_signature: candidate.schema_signature,
                overrides: candidate.overrides.clone(),
            },
        );

        self.store
            .write(&next_document)
            .map_err(MaterialProgramParameterMutationError::Persistence)?;
        self.document = next_document;
        self.parameter_generation = next_generation;
        Ok(MaterialProgramParameterUpdate {
            snapshot: self.configuration_snapshot(candidate, registry_generation),
            changed: true,
        })
    }

    pub fn parameter_block_for_effect(
        &self,
        effect: &RegisteredEffect,
    ) -> Result<EffectParameterBlock, EffectValidationError> {
        let overrides = self.validated_overrides(effect).unwrap_or_default();
        let values = effect.parameters.iter().map(|(name, parameter)| {
            (
                parameter.spec.id,
                overrides.get(name).copied().unwrap_or(parameter.default),
            )
        });
        EffectParameterBlock::from_values(values)
    }

    fn configuration_snapshot(
        &self,
        configuration: MaterialProgramParameterConfiguration,
        registry_generation: &Arc<EffectRegistryGeneration>,
    ) -> MaterialProgramParameterConfigurationSnapshot {
        MaterialProgramParameterConfigurationSnapshot {
            registry_generation: registry_generation.generation,
            parameter_generation: self.parameter_generation,
            description: self
                .describe(&configuration.program, registry_generation)
                .ok(),
            configuration,
        }
    }

    fn validated_overrides(
        &self,
        effect: &RegisteredEffect,
    ) -> Option<BTreeMap<String, EffectUniformValue>> {
        let intent = self.document.programs.get(&effect.name)?;
        if intent.schema_signature != effect.parameter_schema_signature() {
            return None;
        }
        if intent.overrides.len() > crate::effects::MAX_EFFECT_PARAMETERS {
            return None;
        }

        let mut values = BTreeMap::new();
        for (name, value) in &intent.overrides {
            let parameter = effect.parameters.get(name)?;
            let value = value.to_effect_uniform_value().ok()?;
            validate_parameter_value(&parameter.spec, value).ok()?;
            values.insert(name.clone(), value);
        }
        Some(values)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialProgramParameterUpdate {
    pub snapshot: MaterialProgramParameterConfigurationSnapshot,
    pub changed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialProgramParameterDescriptionError {
    InvalidName,
    UnknownProgram,
    Unqualified,
    InvalidTrustedSchema,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MaterialProgramParameterMutationError {
    InvalidConfiguration,
    UnknownProgram,
    Unqualified,
    StaleSchema,
    UnknownParameter,
    InvalidParameterValue,
    GenerationExhausted,
    Persistence(MaterialProgramParameterPersistenceError),
}

impl std::fmt::Display for MaterialProgramParameterMutationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "material program parameter configuration is invalid",
            Self::UnknownProgram => "material program is not registered",
            Self::Unqualified => "program is not qualified as a global material",
            Self::StaleSchema => "material program schema is stale",
            Self::UnknownParameter => "material program parameter is not declared",
            Self::InvalidParameterValue => "material program parameter value is invalid",
            Self::GenerationExhausted => "material program parameter generation is exhausted",
            Self::Persistence(error) => return error.fmt(formatter),
        })
    }
}

impl std::error::Error for MaterialProgramParameterMutationError {}
