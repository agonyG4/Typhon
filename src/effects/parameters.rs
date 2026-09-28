use super::{EffectParameterId, EffectValidationError, MAX_EFFECT_UNIFORMS_PER_SHADER};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectParameterType {
    Float,
    Vec2,
    Vec3,
    Vec4,
    Int,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EffectParameterRange {
    Float {
        min: f32,
        max: f32,
    },
    FloatComponents {
        min: [f32; 4],
        max: [f32; 4],
        components: u8,
    },
    Int {
        min: i32,
        max: i32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectParameterImpact {
    UniformOnly,
    Footprint,
    Structure,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectParameterSpec {
    pub id: EffectParameterId,
    pub name: String,
    pub ty: EffectParameterType,
    pub range: Option<EffectParameterRange>,
    pub impact: EffectParameterImpact,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EffectUniformValue {
    Float(f32),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
    Int(i32),
}

impl EffectUniformValue {
    pub fn is_finite(self) -> bool {
        match self {
            Self::Float(value) => value.is_finite(),
            Self::Vec2(value) => value.iter().all(|value| value.is_finite()),
            Self::Vec3(value) => value.iter().all(|value| value.is_finite()),
            Self::Vec4(value) => value.iter().all(|value| value.is_finite()),
            Self::Int(_) => true,
        }
    }
}

/// Validate a value against the trusted declaration for one effect parameter.
///
/// This is the authoritative type, finiteness, and declared-range validator
/// shared by the authenticated per-surface protocol and Material Program
/// control.
pub fn validate_parameter_value(
    spec: &EffectParameterSpec,
    value: EffectUniformValue,
) -> Result<(), EffectValidationError> {
    let type_matches = matches!(
        (spec.ty, value),
        (EffectParameterType::Float, EffectUniformValue::Float(_))
            | (EffectParameterType::Vec2, EffectUniformValue::Vec2(_))
            | (EffectParameterType::Vec3, EffectUniformValue::Vec3(_))
            | (EffectParameterType::Vec4, EffectUniformValue::Vec4(_))
            | (EffectParameterType::Int, EffectUniformValue::Int(_))
    );
    if !type_matches || !value.is_finite() {
        return Err(EffectValidationError::InvalidParameterValue);
    }

    let within_range = match (spec.range, value) {
        (None, _) => true,
        (Some(EffectParameterRange::Float { min, max }), EffectUniformValue::Float(value)) => {
            value >= min && value <= max
        }
        (Some(EffectParameterRange::Float { min, max }), EffectUniformValue::Vec2(values)) => {
            values.iter().all(|value| *value >= min && *value <= max)
        }
        (Some(EffectParameterRange::Float { min, max }), EffectUniformValue::Vec3(values)) => {
            values.iter().all(|value| *value >= min && *value <= max)
        }
        (Some(EffectParameterRange::Float { min, max }), EffectUniformValue::Vec4(values)) => {
            values.iter().all(|value| *value >= min && *value <= max)
        }
        (
            Some(EffectParameterRange::FloatComponents {
                min,
                max,
                components: 2,
            }),
            EffectUniformValue::Vec2(values),
        ) => values
            .iter()
            .enumerate()
            .all(|(index, value)| *value >= min[index] && *value <= max[index]),
        (
            Some(EffectParameterRange::FloatComponents {
                min,
                max,
                components: 3,
            }),
            EffectUniformValue::Vec3(values),
        ) => values
            .iter()
            .enumerate()
            .all(|(index, value)| *value >= min[index] && *value <= max[index]),
        (
            Some(EffectParameterRange::FloatComponents {
                min,
                max,
                components: 4,
            }),
            EffectUniformValue::Vec4(values),
        ) => values
            .iter()
            .enumerate()
            .all(|(index, value)| *value >= min[index] && *value <= max[index]),
        (Some(EffectParameterRange::Int { min, max }), EffectUniformValue::Int(value)) => {
            value >= min && value <= max
        }
        _ => false,
    };
    if within_range {
        Ok(())
    } else {
        Err(EffectValidationError::InvalidParameterValue)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectParameterValue {
    pub id: EffectParameterId,
    pub value: EffectUniformValue,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EffectParameterBlock {
    values: Vec<EffectParameterValue>,
}

impl EffectParameterBlock {
    pub fn from_values<I>(values: I) -> Result<Self, EffectValidationError>
    where
        I: IntoIterator<Item = (EffectParameterId, EffectUniformValue)>,
    {
        let mut block = Self::default();
        for (id, value) in values {
            block.insert(id, value)?;
        }
        Ok(block)
    }

    pub fn insert(
        &mut self,
        id: EffectParameterId,
        value: EffectUniformValue,
    ) -> Result<(), EffectValidationError> {
        if self.values.len() >= MAX_EFFECT_UNIFORMS_PER_SHADER {
            return Err(EffectValidationError::TooManyUniforms);
        }
        if self.values.iter().any(|entry| entry.id == id) {
            return Err(EffectValidationError::InvalidId);
        }
        if !value.is_finite() {
            return Err(EffectValidationError::NonFiniteValue);
        }
        self.values.push(EffectParameterValue { id, value });
        self.values.sort_by_key(|entry| entry.id);
        Ok(())
    }

    pub fn values(&self) -> &[EffectParameterValue] {
        &self.values
    }
}
