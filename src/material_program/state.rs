use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};

use crate::effects::EffectRegistryGeneration;

use super::{
    MaterialProgramConfiguration, MaterialProgramConfigurationStore,
    MaterialProgramPersistenceError, qualifies_registered_effect,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialProgramConfigSource {
    Default,
    Persisted,
    Runtime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialProgramFallbackReason {
    Missing,
    Unqualified,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialProgramSelectionSnapshot {
    pub generation: u64,
    pub source: MaterialProgramConfigSource,
    pub configuration: MaterialProgramConfiguration,
    pub effective_program: String,
    pub registry_generation: u64,
    pub rendering_available: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub fallback_reason: Option<MaterialProgramFallbackReason>,
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialProgramControlState {
    configuration: MaterialProgramConfiguration,
    store: MaterialProgramConfigurationStore,
    generation: u64,
    source: MaterialProgramConfigSource,
}

impl Default for MaterialProgramControlState {
    fn default() -> Self {
        Self::from_environment()
    }
}

impl MaterialProgramControlState {
    pub fn from_environment() -> Self {
        let store = MaterialProgramConfigurationStore::from_environment()
            .unwrap_or_else(|_| MaterialProgramConfigurationStore::unavailable());
        Self::from_store(store)
    }

    pub fn from_store(store: MaterialProgramConfigurationStore) -> Self {
        let (configuration, source) = match store.read() {
            Ok(configuration) => (configuration, MaterialProgramConfigSource::Persisted),
            Err(_) => (
                MaterialProgramConfiguration::default(),
                MaterialProgramConfigSource::Default,
            ),
        };
        Self {
            configuration,
            store,
            generation: 0,
            source,
        }
    }

    pub fn configuration(&self) -> &MaterialProgramConfiguration {
        &self.configuration
    }

    pub fn snapshot(
        &self,
        registry_generation: &Arc<EffectRegistryGeneration>,
        rendering_available: bool,
    ) -> MaterialProgramSelectionSnapshot {
        let (effective_program, fallback_reason) =
            resolve_requested_program(&self.configuration, registry_generation.as_ref());
        MaterialProgramSelectionSnapshot {
            generation: self.generation,
            source: self.source,
            configuration: self.configuration.clone(),
            effective_program,
            registry_generation: registry_generation.generation,
            rendering_available,
            fallback_reason,
        }
    }

    pub fn set_configuration(
        &mut self,
        candidate: MaterialProgramConfiguration,
        registry_generation: &Arc<EffectRegistryGeneration>,
        rendering_available: bool,
    ) -> Result<MaterialProgramSelectionUpdate, MaterialProgramMutationError> {
        candidate
            .validate()
            .map_err(|_| MaterialProgramMutationError::InvalidConfiguration)?;
        if candidate == self.configuration {
            return Ok(MaterialProgramSelectionUpdate {
                snapshot: self.snapshot(registry_generation, rendering_available),
                changed: false,
            });
        }
        let Some(effect) = registry_generation
            .effects
            .get(&candidate.requested_program)
        else {
            return Err(MaterialProgramMutationError::UnknownProgram);
        };
        if !qualifies_registered_effect(effect) {
            return Err(MaterialProgramMutationError::Unqualified);
        }
        self.store
            .write(&candidate)
            .map_err(MaterialProgramMutationError::Persistence)?;
        self.configuration = candidate;
        self.source = MaterialProgramConfigSource::Runtime;
        self.generation = self.generation.saturating_add(1);
        Ok(MaterialProgramSelectionUpdate {
            snapshot: self.snapshot(registry_generation, rendering_available),
            changed: true,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialProgramSelectionUpdate {
    pub snapshot: MaterialProgramSelectionSnapshot,
    pub changed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MaterialProgramMutationError {
    InvalidConfiguration,
    UnknownProgram,
    Unqualified,
    Persistence(MaterialProgramPersistenceError),
}

impl std::fmt::Display for MaterialProgramMutationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "material program configuration is invalid",
            Self::UnknownProgram => "requested material program is not registered",
            Self::Unqualified => "requested program is not qualified as a global material",
            Self::Persistence(error) => return error.fmt(formatter),
        })
    }
}

impl std::error::Error for MaterialProgramMutationError {}

fn resolve_requested_program(
    configuration: &MaterialProgramConfiguration,
    registry_generation: &EffectRegistryGeneration,
) -> (String, Option<MaterialProgramFallbackReason>) {
    let fallback = crate::effects::BUILTIN_BACKGROUND_BLUR_NAME.to_owned();
    match registry_generation
        .effects
        .get(&configuration.requested_program)
    {
        None => (fallback, Some(MaterialProgramFallbackReason::Missing)),
        Some(effect) if !qualifies_registered_effect(effect) => {
            (fallback, Some(MaterialProgramFallbackReason::Unqualified))
        }
        Some(effect) => (effect.name.clone(), None),
    }
}
