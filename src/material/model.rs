use serde::{Deserialize, Serialize};

pub const MATERIAL_CONFIGURATION_VERSION: u8 = 1;
pub const DEFAULT_MATERIAL_POSITION: f32 = 0.5;
pub const MATERIAL_VALUE_MIN: f32 = 0.0;
pub const MATERIAL_VALUE_MAX: f32 = 1.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialConfiguration {
    pub version: u8,
    pub position: f32,
    pub overrides: MaterialOverrides,
}

impl Default for MaterialConfiguration {
    fn default() -> Self {
        Self {
            version: MATERIAL_CONFIGURATION_VERSION,
            position: DEFAULT_MATERIAL_POSITION,
            overrides: MaterialOverrides::default(),
        }
    }
}

impl MaterialConfiguration {
    pub fn validate(&self) -> Result<(), MaterialConfigurationError> {
        if self.version != MATERIAL_CONFIGURATION_VERSION {
            return Err(MaterialConfigurationError::UnsupportedVersion(self.version));
        }
        validate_normalized(self.position, MaterialDimension::Position)?;
        self.overrides.validate()
    }

    pub fn effective(&self) -> Result<EffectiveMaterial, MaterialConfigurationError> {
        self.validate()?;
        MaterialCurveV1::resolve(self.position, &self.overrides)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialOverrides {
    pub blur: Option<f32>,
    pub saturation: Option<f32>,
    pub noise: Option<f32>,
}

impl MaterialOverrides {
    pub fn validate(&self) -> Result<(), MaterialConfigurationError> {
        for (dimension, value) in [
            (MaterialDimension::Blur, self.blur),
            (MaterialDimension::Saturation, self.saturation),
            (MaterialDimension::Noise, self.noise),
        ] {
            if let Some(value) = value {
                validate_normalized(value, dimension)?;
            }
        }
        Ok(())
    }

    pub fn clear(&mut self) {
        self.blur = None;
        self.saturation = None;
        self.noise = None;
    }

    pub fn is_empty(&self) -> bool {
        self.blur.is_none() && self.saturation.is_none() && self.noise.is_none()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EffectiveMaterial {
    /// Normalized blur strength. Typhon maps this to compositor-owned blur tuning.
    pub blur: f32,
    /// Normalized saturation intent. One is the strongest point on this semantic curve.
    pub saturation: f32,
    /// Normalized noise amount.
    pub noise: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialConfigSource {
    Default,
    Persisted,
    Runtime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialCapabilities {
    pub blur_override: bool,
    pub saturation_override: bool,
    pub noise_override: bool,
}

impl MaterialCapabilities {
    pub const fn unavailable() -> Self {
        Self {
            blur_override: false,
            saturation_override: false,
            noise_override: false,
        }
    }

    pub const fn full() -> Self {
        Self {
            blur_override: true,
            saturation_override: true,
            noise_override: true,
        }
    }
}

impl Default for MaterialCapabilities {
    fn default() -> Self {
        Self::unavailable()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialSnapshot {
    pub generation: u64,
    pub source: MaterialConfigSource,
    pub configuration: MaterialConfiguration,
    pub effective: EffectiveMaterial,
    pub capabilities: MaterialCapabilities,
}

impl MaterialSnapshot {
    pub fn new(
        generation: u64,
        source: MaterialConfigSource,
        configuration: MaterialConfiguration,
        capabilities: MaterialCapabilities,
    ) -> Result<Self, MaterialConfigurationError> {
        let effective = configuration.effective()?;
        Ok(Self {
            generation,
            source,
            configuration,
            effective,
            capabilities,
        })
    }
}

/// The one semantic Glass ↔ Frosted mapping used by Typhon.
pub struct MaterialCurveV1;

impl MaterialCurveV1 {
    pub fn resolve(
        position: f32,
        overrides: &MaterialOverrides,
    ) -> Result<EffectiveMaterial, MaterialConfigurationError> {
        validate_normalized(position, MaterialDimension::Position)?;
        overrides.validate()?;

        let position = position.clamp(MATERIAL_VALUE_MIN, MATERIAL_VALUE_MAX);
        let base = EffectiveMaterial {
            blur: 0.12 + 0.88 * position,
            saturation: 1.0 - 0.28 * position,
            noise: 0.02 + 0.12 * position,
        };
        let effective = EffectiveMaterial {
            blur: overrides.blur.unwrap_or(base.blur),
            saturation: overrides.saturation.unwrap_or(base.saturation),
            noise: overrides.noise.unwrap_or(base.noise),
        };
        debug_assert!(
            [effective.blur, effective.saturation, effective.noise]
                .into_iter()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(&value))
        );
        Ok(effective)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialDimension {
    Position,
    Blur,
    Saturation,
    Noise,
}

impl MaterialDimension {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Position => "position",
            Self::Blur => "blur override",
            Self::Saturation => "saturation override",
            Self::Noise => "noise override",
        }
    }
}

fn validate_normalized(
    value: f32,
    dimension: MaterialDimension,
) -> Result<(), MaterialConfigurationError> {
    if !value.is_finite() {
        return Err(MaterialConfigurationError::NonFinite(dimension));
    }
    if !(MATERIAL_VALUE_MIN..=MATERIAL_VALUE_MAX).contains(&value) {
        return Err(MaterialConfigurationError::OutOfRange(dimension));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialConfigurationError {
    UnsupportedVersion(u8),
    NonFinite(MaterialDimension),
    OutOfRange(MaterialDimension),
}

impl std::fmt::Display for MaterialConfigurationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => {
                write!(
                    formatter,
                    "unsupported material configuration version {version}"
                )
            }
            Self::NonFinite(dimension) => {
                write!(formatter, "{} must be finite", dimension.as_str())
            }
            Self::OutOfRange(dimension) => {
                write!(formatter, "{} must be within 0.0..=1.0", dimension.as_str())
            }
        }
    }
}

impl std::error::Error for MaterialConfigurationError {}
