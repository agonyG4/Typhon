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
    Float { min: f32, max: f32 },
    Int { min: i32, max: i32 },
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
