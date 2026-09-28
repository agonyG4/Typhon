use serde::de::Error as DeError;
use serde::ser::{Error as SerError, SerializeMap};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::effects::{
    EffectParameterRange, EffectParameterType, EffectUniformValue, EffectValidationError,
    MAX_EFFECT_PARAMETERS, config::validate_effect_name,
};

pub const MATERIAL_PROGRAM_PARAMETER_CONFIGURATION_VERSION: u8 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialProgramNameArguments {
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MaterialProgramParameterValue {
    Float(f64),
    Vec2([f64; 2]),
    Vec3([f64; 3]),
    Vec4([f64; 4]),
    Int(i32),
}

#[derive(Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum MaterialProgramParameterValueWire {
    Float(f64),
    Vec2([f64; 2]),
    Vec3([f64; 3]),
    Vec4([f64; 4]),
    Int(i32),
}

impl MaterialProgramParameterValue {
    pub fn to_effect_uniform_value(self) -> Result<EffectUniformValue, EffectValidationError> {
        let checked_f32 = |value: f64| {
            if !value.is_finite() || value < f64::from(f32::MIN) || value > f64::from(f32::MAX) {
                return Err(EffectValidationError::InvalidParameterValue);
            }
            let converted = value as f32;
            if converted.is_finite() && (value == 0.0 || converted != 0.0) {
                Ok(converted)
            } else {
                Err(EffectValidationError::InvalidParameterValue)
            }
        };

        Ok(match self {
            Self::Float(value) => EffectUniformValue::Float(checked_f32(value)?),
            Self::Vec2(values) => {
                EffectUniformValue::Vec2([checked_f32(values[0])?, checked_f32(values[1])?])
            }
            Self::Vec3(values) => EffectUniformValue::Vec3([
                checked_f32(values[0])?,
                checked_f32(values[1])?,
                checked_f32(values[2])?,
            ]),
            Self::Vec4(values) => EffectUniformValue::Vec4([
                checked_f32(values[0])?,
                checked_f32(values[1])?,
                checked_f32(values[2])?,
                checked_f32(values[3])?,
            ]),
            Self::Int(value) => EffectUniformValue::Int(value),
        })
    }

    pub fn from_effect_uniform_value(
        value: EffectUniformValue,
    ) -> Result<Self, EffectValidationError> {
        if !value.is_finite() {
            return Err(EffectValidationError::InvalidParameterValue);
        }
        Ok(match value {
            EffectUniformValue::Float(value) => Self::Float(effect_float_to_wire(value)),
            EffectUniformValue::Vec2(values) => Self::Vec2(values.map(effect_float_to_wire)),
            EffectUniformValue::Vec3(values) => Self::Vec3(values.map(effect_float_to_wire)),
            EffectUniformValue::Vec4(values) => Self::Vec4(values.map(effect_float_to_wire)),
            EffectUniformValue::Int(value) => Self::Int(value),
        })
    }

    fn is_finite(self) -> bool {
        match self {
            Self::Float(value) => value.is_finite(),
            Self::Vec2(values) => values.iter().all(|value| value.is_finite()),
            Self::Vec3(values) => values.iter().all(|value| value.is_finite()),
            Self::Vec4(values) => values.iter().all(|value| value.is_finite()),
            Self::Int(_) => true,
        }
    }
}

fn effect_float_to_wire(value: f32) -> f64 {
    value
        .to_string()
        .parse()
        .unwrap_or_else(|_| f64::from(value))
}

impl Serialize for MaterialProgramParameterValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if !self.is_finite() {
            return Err(S::Error::custom("material parameter values must be finite"));
        }
        let mut map = serializer.serialize_map(Some(2))?;
        match self {
            Self::Float(value) => {
                map.serialize_entry("type", "float")?;
                map.serialize_entry("value", value)?;
            }
            Self::Vec2(value) => {
                map.serialize_entry("type", "vec2")?;
                map.serialize_entry("value", value)?;
            }
            Self::Vec3(value) => {
                map.serialize_entry("type", "vec3")?;
                map.serialize_entry("value", value)?;
            }
            Self::Vec4(value) => {
                map.serialize_entry("type", "vec4")?;
                map.serialize_entry("value", value)?;
            }
            Self::Int(value) => {
                map.serialize_entry("type", "int")?;
                map.serialize_entry("value", value)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for MaterialProgramParameterValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = match MaterialProgramParameterValueWire::deserialize(deserializer)? {
            MaterialProgramParameterValueWire::Float(value) => Self::Float(value),
            MaterialProgramParameterValueWire::Vec2(value) => Self::Vec2(value),
            MaterialProgramParameterValueWire::Vec3(value) => Self::Vec3(value),
            MaterialProgramParameterValueWire::Vec4(value) => Self::Vec4(value),
            MaterialProgramParameterValueWire::Int(value) => Self::Int(value),
        };
        if value.is_finite() {
            Ok(value)
        } else {
            Err(D::Error::custom("material parameter values must be finite"))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialProgramParameterType {
    Float,
    Vec2,
    Vec3,
    Vec4,
    Int,
}

impl From<EffectParameterType> for MaterialProgramParameterType {
    fn from(value: EffectParameterType) -> Self {
        match value {
            EffectParameterType::Float => Self::Float,
            EffectParameterType::Vec2 => Self::Vec2,
            EffectParameterType::Vec3 => Self::Vec3,
            EffectParameterType::Vec4 => Self::Vec4,
            EffectParameterType::Int => Self::Int,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MaterialProgramParameterRange {
    Float {
        min: f64,
        max: f64,
    },
    FloatComponents {
        components: u8,
        min: Vec<f64>,
        max: Vec<f64>,
    },
    Int {
        min: i32,
        max: i32,
    },
}

impl MaterialProgramParameterRange {
    fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Float { min, max } if min.is_finite() && max.is_finite() && min <= max => Ok(()),
            Self::Float { .. } => Err("invalid float parameter range"),
            Self::FloatComponents {
                components,
                min,
                max,
            } if (2..=4).contains(components)
                && min.len() == usize::from(*components)
                && max.len() == usize::from(*components)
                && min
                    .iter()
                    .zip(max)
                    .all(|(min, max)| min.is_finite() && max.is_finite() && min <= max) =>
            {
                Ok(())
            }
            Self::FloatComponents { .. } => Err("invalid component parameter range"),
            Self::Int { min, max } if min <= max => Ok(()),
            Self::Int { .. } => Err("invalid integer parameter range"),
        }
    }

    pub(crate) fn from_effect_range(
        range: Option<EffectParameterRange>,
    ) -> Result<Option<Self>, EffectValidationError> {
        let range = match range {
            None => return Ok(None),
            Some(EffectParameterRange::Float { min, max })
                if min.is_finite() && max.is_finite() && min <= max =>
            {
                Self::Float {
                    min: effect_float_to_wire(min),
                    max: effect_float_to_wire(max),
                }
            }
            Some(EffectParameterRange::FloatComponents {
                min,
                max,
                components,
            }) if (2..=4).contains(&components)
                && min[..usize::from(components)]
                    .iter()
                    .zip(&max[..usize::from(components)])
                    .all(|(min, max)| min.is_finite() && max.is_finite() && min <= max) =>
            {
                let count = usize::from(components);
                Self::FloatComponents {
                    components,
                    min: min[..count]
                        .iter()
                        .copied()
                        .map(effect_float_to_wire)
                        .collect(),
                    max: max[..count]
                        .iter()
                        .copied()
                        .map(effect_float_to_wire)
                        .collect(),
                }
            }
            Some(EffectParameterRange::Float { .. })
            | Some(EffectParameterRange::FloatComponents { .. }) => {
                return Err(EffectValidationError::InvalidParameterValue);
            }
            Some(EffectParameterRange::Int { min, max }) if min <= max => Self::Int { min, max },
            Some(EffectParameterRange::Int { .. }) => {
                return Err(EffectValidationError::InvalidParameterValue);
            }
        };
        Ok(Some(range))
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum MaterialProgramParameterRangeWireRef<'a> {
    Float {
        min: &'a f64,
        max: &'a f64,
    },
    FloatComponents {
        components: &'a u8,
        min: &'a [f64],
        max: &'a [f64],
    },
    Int {
        min: &'a i32,
        max: &'a i32,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum MaterialProgramParameterRangeWire {
    Float {
        min: f64,
        max: f64,
    },
    FloatComponents {
        components: u8,
        min: Vec<f64>,
        max: Vec<f64>,
    },
    Int {
        min: i32,
        max: i32,
    },
}

impl Serialize for MaterialProgramParameterRange {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate().map_err(S::Error::custom)?;
        let wire = match self {
            Self::Float { min, max } => MaterialProgramParameterRangeWireRef::Float { min, max },
            Self::FloatComponents {
                components,
                min,
                max,
            } => MaterialProgramParameterRangeWireRef::FloatComponents {
                components,
                min,
                max,
            },
            Self::Int { min, max } => MaterialProgramParameterRangeWireRef::Int { min, max },
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MaterialProgramParameterRange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let range = match MaterialProgramParameterRangeWire::deserialize(deserializer)? {
            MaterialProgramParameterRangeWire::Float { min, max } => Self::Float { min, max },
            MaterialProgramParameterRangeWire::FloatComponents {
                components,
                min,
                max,
            } => Self::FloatComponents {
                components,
                min,
                max,
            },
            MaterialProgramParameterRangeWire::Int { min, max } => Self::Int { min, max },
        };
        range.validate().map_err(D::Error::custom)?;
        Ok(range)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialProgramParameterDescriptor {
    pub name: String,
    pub parameter_type: MaterialProgramParameterType,
    pub range: Option<MaterialProgramParameterRange>,
    pub default: MaterialProgramParameterValue,
    pub effective: MaterialProgramParameterValue,
    pub overridden: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialProgramDescriptionSnapshot {
    pub registry_generation: u64,
    pub parameter_generation: u64,
    pub name: String,
    pub origin: super::MaterialProgramOrigin,
    pub schema_signature: u64,
    pub parameters: Vec<MaterialProgramParameterDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialProgramParameterConfiguration {
    pub version: u8,
    pub program: String,
    pub schema_signature: u64,
    pub overrides: std::collections::BTreeMap<String, MaterialProgramParameterValue>,
}

impl MaterialProgramParameterConfiguration {
    pub fn validate(&self) -> Result<(), MaterialProgramParameterConfigurationError> {
        if self.version != MATERIAL_PROGRAM_PARAMETER_CONFIGURATION_VERSION {
            return Err(MaterialProgramParameterConfigurationError::UnsupportedVersion);
        }
        validate_effect_name(&self.program)
            .map_err(|_| MaterialProgramParameterConfigurationError::InvalidProgramName)?;
        if self.overrides.len() > MAX_EFFECT_PARAMETERS {
            return Err(MaterialProgramParameterConfigurationError::TooManyOverrides);
        }
        for (name, value) in &self.overrides {
            validate_effect_name(name)
                .map_err(|_| MaterialProgramParameterConfigurationError::InvalidParameterName)?;
            value
                .to_effect_uniform_value()
                .map_err(|_| MaterialProgramParameterConfigurationError::InvalidValue)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialProgramParameterConfigurationError {
    UnsupportedVersion,
    InvalidProgramName,
    InvalidParameterName,
    TooManyOverrides,
    InvalidValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialProgramParameterConfigurationSnapshot {
    pub registry_generation: u64,
    pub parameter_generation: u64,
    pub configuration: MaterialProgramParameterConfiguration,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub description: Option<MaterialProgramDescriptionSnapshot>,
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}
