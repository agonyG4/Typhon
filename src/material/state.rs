use super::{
    MaterialCapabilities, MaterialConfigSource, MaterialConfiguration, MaterialConfigurationStore,
    MaterialDimension, MaterialPersistenceError, MaterialSnapshot,
};

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialControlState {
    configuration: MaterialConfiguration,
    store: MaterialConfigurationStore,
    generation: u64,
    source: MaterialConfigSource,
}

impl Default for MaterialControlState {
    fn default() -> Self {
        Self::from_environment()
    }
}

impl MaterialControlState {
    pub fn from_environment() -> Self {
        let store = MaterialConfigurationStore::from_environment()
            .unwrap_or_else(|_| MaterialConfigurationStore::unavailable());
        Self::from_store(store)
    }

    pub fn from_store(store: MaterialConfigurationStore) -> Self {
        let (configuration, source) = match store.read() {
            Ok(configuration) => (configuration, MaterialConfigSource::Persisted),
            Err(_) => (
                MaterialConfiguration::default(),
                MaterialConfigSource::Default,
            ),
        };
        Self {
            configuration,
            store,
            generation: 0,
            source,
        }
    }

    pub fn configuration(&self) -> &MaterialConfiguration {
        &self.configuration
    }

    pub fn snapshot(&self, runtime_capabilities: MaterialCapabilities) -> MaterialSnapshot {
        MaterialSnapshot::new(
            self.generation,
            self.source,
            self.configuration.clone(),
            runtime_capabilities,
        )
        .expect("active material configuration is validated")
    }

    pub(crate) fn validate_candidate(
        &self,
        candidate: &MaterialConfiguration,
        runtime_capabilities: MaterialCapabilities,
    ) -> Result<(), MaterialMutationError> {
        candidate
            .validate()
            .map_err(|_| MaterialMutationError::Invalid)?;
        candidate
            .effective()
            .map_err(|_| MaterialMutationError::Invalid)?;

        for (dimension, supported, current, proposed) in [
            (
                MaterialDimension::Blur,
                runtime_capabilities.blur_override,
                self.configuration.overrides.blur,
                candidate.overrides.blur,
            ),
            (
                MaterialDimension::Saturation,
                runtime_capabilities.saturation_override,
                self.configuration.overrides.saturation,
                candidate.overrides.saturation,
            ),
            (
                MaterialDimension::Noise,
                runtime_capabilities.noise_override,
                self.configuration.overrides.noise,
                candidate.overrides.noise,
            ),
        ] {
            if !supported && proposed.is_some() && proposed != current {
                return Err(MaterialMutationError::UnsupportedCapability(dimension));
            }
        }
        Ok(())
    }

    /// Validate and persist first, then publish the candidate as the active state.
    pub fn set_configuration(
        &mut self,
        candidate: MaterialConfiguration,
        runtime_capabilities: MaterialCapabilities,
    ) -> Result<MaterialSnapshot, MaterialMutationError> {
        self.validate_candidate(&candidate, runtime_capabilities)?;
        self.store
            .write(&candidate)
            .map_err(MaterialMutationError::Persistence)?;
        self.configuration = candidate;
        self.source = MaterialConfigSource::Runtime;
        self.generation = self.generation.saturating_add(1);
        Ok(self.snapshot(runtime_capabilities))
    }
}

#[derive(Debug)]
pub enum MaterialMutationError {
    Invalid,
    UnsupportedCapability(MaterialDimension),
    Persistence(MaterialPersistenceError),
}

impl std::fmt::Display for MaterialMutationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid => formatter.write_str("material configuration is invalid"),
            Self::UnsupportedCapability(_) => {
                formatter.write_str("material override is unsupported by the active renderer")
            }
            Self::Persistence(error) => formatter.write_str(&error.to_string()),
        }
    }
}

impl std::error::Error for MaterialMutationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::MaterialOverrides;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn config_home() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "typhon-material-state-{}-{}",
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
    fn successful_update_persists_then_advances_authoritative_generation() {
        let directory = config_home();
        let store = MaterialConfigurationStore::new(directory.clone()).unwrap();
        let mut state = MaterialControlState::from_store(store.clone());
        let candidate = MaterialConfiguration {
            position: 0.9,
            overrides: MaterialOverrides {
                noise: Some(0.4),
                ..MaterialOverrides::default()
            },
            ..MaterialConfiguration::default()
        };

        let snapshot = state
            .set_configuration(candidate.clone(), MaterialCapabilities::full())
            .unwrap();
        assert_eq!(snapshot.generation, 1);
        assert_eq!(snapshot.source, MaterialConfigSource::Runtime);
        assert_eq!(snapshot.configuration, candidate);
        assert_eq!(store.read().unwrap(), candidate);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn failed_persistence_does_not_publish_candidate_or_advance_generation() {
        let mut state = MaterialControlState::from_store(MaterialConfigurationStore::unavailable());
        let previous = state.snapshot(MaterialCapabilities::unavailable());
        let candidate = MaterialConfiguration {
            position: 0.9,
            ..MaterialConfiguration::default()
        };

        assert!(
            state
                .set_configuration(candidate, MaterialCapabilities::unavailable())
                .is_err()
        );
        assert_eq!(
            state.snapshot(MaterialCapabilities::unavailable()),
            previous
        );
    }

    #[test]
    fn invalid_candidate_is_rejected_without_generation_change() {
        let directory = config_home();
        let store = MaterialConfigurationStore::new(directory.clone()).unwrap();
        let mut state = MaterialControlState::from_store(store);
        let previous = state.snapshot(MaterialCapabilities::full());
        let candidate = MaterialConfiguration {
            position: f32::NAN,
            ..MaterialConfiguration::default()
        };

        assert!(
            state
                .set_configuration(candidate, MaterialCapabilities::full())
                .is_err()
        );
        assert_eq!(state.snapshot(MaterialCapabilities::full()), previous);
        let _ = fs::remove_dir_all(directory);
    }
}
