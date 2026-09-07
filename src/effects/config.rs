//! Bounded, trusted-local configuration for named effects.
//!
//! This module deliberately stops at the compositor trust boundary.  A
//! manifest can describe an effect program and reference a shader only by a
//! relative path below the caller-supplied trusted root.  It cannot provide
//! arbitrary source to a Wayland client or bypass the typed effect IR.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use serde_json::{Map, Value};

use super::{
    BlendMode, BlendSpec, ColorMatrixSpec, CustomFragmentSpec, DualKawaseBlurSpec, EffectAlphaMode,
    EffectFailurePolicy, EffectFootprint, EffectFrameDemand, EffectNode, EffectNodeId,
    EffectParameterId, EffectParameterImpact, EffectParameterRange, EffectParameterSpec,
    EffectProgram, EffectProgramId, EffectSource, EffectUniformBinding, EffectUniformValue,
    EffectValidationError, EffectWorkingSpace, MAX_EFFECT_PROGRAM_NODES, MAX_EFFECT_PROGRAMS,
    MAX_EFFECT_SHADER_SOURCE_BYTES, MAX_EFFECT_UNIFORMS_PER_SHADER, MaskMode, MaskSpec, NoiseKind,
    NoiseSpec, ShaderModuleId, TintSpec,
};

pub const EFFECT_MANIFEST_VERSION: u64 = 1;
pub const MAX_EFFECT_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_EFFECT_NAME_BYTES: usize = 128;
pub const MAX_EFFECT_PARAMETERS: usize = MAX_EFFECT_UNIFORMS_PER_SHADER;

#[derive(Clone, Debug, PartialEq)]
pub struct EffectManifest {
    pub version: u64,
    pub effects: BTreeMap<String, EffectDefinition>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectDefinition {
    pub name: String,
    pub program: EffectProgram,
    pub parameters: BTreeMap<String, EffectParameterDefinition>,
    pub shader_assets: Vec<EffectShaderAsset>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectParameterDefinition {
    pub spec: EffectParameterSpec,
    pub default: EffectUniformValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectShaderAsset {
    pub module: ShaderModuleId,
    pub relative_path: PathBuf,
    pub source: String,
    pub uniforms: Vec<EffectUniformBinding>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EffectConfigError {
    ManifestTooLarge,
    InvalidJson(String),
    InvalidSchema(String),
    UnsupportedVersion(u64),
    InvalidName(String),
    InvalidPath(String),
    ShaderIo(String),
    ShaderTooLarge,
    Validation(EffectValidationError),
    LimitExceeded(&'static str),
}

impl std::fmt::Display for EffectConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for EffectConfigError {}

impl From<EffectValidationError> for EffectConfigError {
    fn from(error: EffectValidationError) -> Self {
        Self::Validation(error)
    }
}

/// Parse and validate a manifest.  `shader_root` is consulted only for
/// custom-fragment assets; it is never searched implicitly.
pub fn parse_manifest(
    bytes: &[u8],
    shader_root: &Path,
) -> Result<EffectManifest, EffectConfigError> {
    if bytes.len() > MAX_EFFECT_MANIFEST_BYTES {
        return Err(EffectConfigError::ManifestTooLarge);
    }
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| EffectConfigError::InvalidJson(error.to_string()))?;
    let object = as_object(&value, "manifest")?;
    reject_unknown(object, &["version", "effects"], "manifest")?;
    let version = required(object, "version")?
        .as_u64()
        .ok_or_else(|| invalid("version must be an unsigned integer"))?;
    if version != EFFECT_MANIFEST_VERSION {
        return Err(EffectConfigError::UnsupportedVersion(version));
    }
    let effects = as_object(required(object, "effects")?, "effects")?;
    if effects.len() > MAX_EFFECT_PROGRAMS {
        return Err(EffectConfigError::LimitExceeded("effect programs"));
    }
    let mut definitions = BTreeMap::new();
    for (name, value) in effects {
        validate_name(name)?;
        let definition = parse_definition(name, value, shader_root)?;
        if definitions.insert(name.clone(), definition).is_some() {
            return Err(invalid("duplicate effect name"));
        }
    }
    Ok(EffectManifest {
        version,
        effects: definitions,
    })
}

/// Read a manifest from an explicitly selected file, bounded before parsing.
/// The file itself must resolve below `config_root`.
pub fn load_manifest(path: &Path, config_root: &Path) -> Result<EffectManifest, EffectConfigError> {
    let resolved = resolve_in_root(config_root, path)?;
    let metadata =
        fs::metadata(&resolved).map_err(|error| EffectConfigError::ShaderIo(error.to_string()))?;
    if !metadata.is_file() {
        return Err(EffectConfigError::InvalidPath(path.display().to_string()));
    }
    if metadata.len() > MAX_EFFECT_MANIFEST_BYTES as u64 {
        return Err(EffectConfigError::ManifestTooLarge);
    }
    let bytes =
        fs::read(&resolved).map_err(|error| EffectConfigError::ShaderIo(error.to_string()))?;
    parse_manifest(&bytes, config_root)
}

pub fn validate_relative_path(path: &str) -> Result<(), EffectConfigError> {
    let path_ref = Path::new(path);
    if path.is_empty()
        || path_ref.is_absolute()
        || path_ref.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(EffectConfigError::InvalidPath(path.to_string()));
    }
    Ok(())
}

pub fn resolve_in_root(root: &Path, relative: &Path) -> Result<PathBuf, EffectConfigError> {
    let relative_string = relative
        .to_str()
        .ok_or_else(|| EffectConfigError::InvalidPath(relative.display().to_string()))?;
    validate_relative_path(relative_string)?;
    let canonical_root =
        fs::canonicalize(root).map_err(|error| EffectConfigError::ShaderIo(error.to_string()))?;
    let candidate = canonical_root.join(relative);
    let resolved = fs::canonicalize(&candidate)
        .map_err(|error| EffectConfigError::ShaderIo(error.to_string()))?;
    if !resolved.starts_with(&canonical_root) {
        return Err(EffectConfigError::InvalidPath(
            relative.display().to_string(),
        ));
    }
    Ok(resolved)
}

fn parse_definition(
    name: &str,
    value: &Value,
    shader_root: &Path,
) -> Result<EffectDefinition, EffectConfigError> {
    let object = as_object(value, "effect definition")?;
    reject_unknown(
        object,
        &[
            "working_space",
            "alpha",
            "frame_demand",
            "failure_policy",
            "outsets",
            "nodes",
            "output",
            "parameters",
        ],
        "effect definition",
    )?;
    let working_space = match optional_string(object, "working_space")?.unwrap_or("linear-srgb") {
        "linear-srgb" => EffectWorkingSpace::LinearSrgb,
        "output-srgb" | "output-encoded-srgb" => EffectWorkingSpace::OutputEncodedSrgb,
        value => return Err(invalid(format!("unknown working_space {value}"))),
    };
    let alpha_mode = match optional_string(object, "alpha")?.unwrap_or("preserve") {
        "opaque" => EffectAlphaMode::Opaque,
        "preserve" => EffectAlphaMode::Preserve,
        value => return Err(invalid(format!("unknown alpha {value}"))),
    };
    let frame_demand = match optional_string(object, "frame_demand")?.unwrap_or("on-damage") {
        "on-damage" => EffectFrameDemand::OnDamage,
        "continuous" => EffectFrameDemand::Continuous,
        "manual" => EffectFrameDemand::Manual,
        value => return Err(invalid(format!("unknown frame_demand {value}"))),
    };
    let failure_policy = match optional_string(object, "failure_policy")?.unwrap_or("passthrough") {
        "passthrough" => EffectFailurePolicy::Passthrough,
        "disable-instance" => EffectFailurePolicy::DisableInstance,
        value => return Err(invalid(format!("unknown failure_policy {value}"))),
    };
    let outsets = parse_outsets(object.get("outsets"))?;
    let nodes_value = required(object, "nodes")?;
    let nodes_array = nodes_value
        .as_array()
        .ok_or_else(|| invalid("nodes must be an array"))?;
    if nodes_array.len() > MAX_EFFECT_PROGRAM_NODES {
        return Err(EffectConfigError::LimitExceeded("nodes"));
    }
    let mut assets = Vec::new();
    let mut nodes = Vec::with_capacity(nodes_array.len());
    for node in nodes_array {
        nodes.push(parse_node(node, shader_root, &mut assets)?);
    }
    let output = parse_id(required(object, "output")?, "output")?;
    let parameters = parse_parameters(object.get("parameters"))?;
    let id = stable_program_id(name);
    let program = EffectProgram {
        id,
        nodes,
        output,
        working_space,
        alpha_mode,
        outsets,
        frame_demand,
        failure_policy,
    };
    super::validate_effect_program(program.clone())?;
    Ok(EffectDefinition {
        name: name.to_string(),
        program,
        parameters,
        shader_assets: assets,
    })
}

fn parse_node(
    value: &Value,
    shader_root: &Path,
    assets: &mut Vec<EffectShaderAsset>,
) -> Result<EffectNode, EffectConfigError> {
    let object = as_object(value, "node")?;
    let id = parse_id(required(object, "id")?, "node id")?;
    let kind = required(object, "kind")?
        .as_str()
        .ok_or_else(|| invalid("node kind must be a string"))?;
    match kind {
        "backdrop" | "target-content" => {
            reject_unknown(object, &["id", "kind"], "source node")?;
            let source = if kind == "backdrop" {
                EffectSource::Backdrop
            } else {
                EffectSource::TargetContent
            };
            Ok(EffectNode::source(id, source))
        }
        "dual-kawase-blur" => {
            reject_unknown(
                object,
                &["id", "kind", "input", "radius", "passes", "scale"],
                "blur node",
            )?;
            let spec = DualKawaseBlurSpec::new(
                parse_f32(required(object, "radius")?, "radius")?,
                parse_u8(required(object, "passes")?, "passes")?,
                parse_f32(required(object, "scale")?, "scale")?,
            )?;
            Ok(EffectNode::dual_kawase(
                id,
                parse_id(required(object, "input")?, "input")?,
                spec,
            ))
        }
        "tint" => {
            reject_unknown(
                object,
                &["id", "kind", "input", "color", "amount"],
                "tint node",
            )?;
            let color = parse_array::<4>(required(object, "color")?, "color")?;
            let spec = TintSpec::new(color, parse_f32(required(object, "amount")?, "amount")?)?;
            Ok(EffectNode::tint(
                id,
                parse_id(required(object, "input")?, "input")?,
                spec,
            ))
        }
        "color-matrix" => {
            reject_unknown(
                object,
                &["id", "kind", "input", "matrix", "bias"],
                "color matrix node",
            )?;
            let spec = ColorMatrixSpec {
                matrix: parse_array::<16>(required(object, "matrix")?, "matrix")?,
                bias: parse_array::<4>(required(object, "bias")?, "bias")?,
            };
            Ok(EffectNode::color_matrix(
                id,
                parse_id(required(object, "input")?, "input")?,
                spec,
            )?)
        }
        "noise" => {
            reject_unknown(object, &["id", "kind", "input", "amount"], "noise node")?;
            let spec = NoiseSpec::new(
                NoiseKind::Hash,
                parse_f32(required(object, "amount")?, "amount")?,
            )?;
            Ok(EffectNode::noise(
                id,
                parse_id(required(object, "input")?, "input")?,
                spec,
            ))
        }
        "mask" => {
            reject_unknown(object, &["id", "kind", "input", "mode"], "mask node")?;
            let mode = match optional_string(object, "mode")?.unwrap_or("alpha") {
                "alpha" => MaskMode::Alpha,
                "inverted-alpha" => MaskMode::InvertedAlpha,
                value => return Err(invalid(format!("unknown mask mode {value}"))),
            };
            Ok(EffectNode::mask(
                id,
                parse_id(required(object, "input")?, "input")?,
                MaskSpec { mode },
            ))
        }
        "blend" => {
            reject_unknown(
                object,
                &["id", "kind", "inputs", "mode", "opacity"],
                "blend node",
            )?;
            let inputs = required(object, "inputs")?
                .as_array()
                .ok_or_else(|| invalid("blend inputs must be an array"))?
                .iter()
                .map(|value| parse_id(value, "blend input"))
                .collect::<Result<Vec<_>, _>>()?;
            let mode = match optional_string(object, "mode")?.unwrap_or("source-over") {
                "source-over" => BlendMode::SourceOver,
                "add" => BlendMode::Add,
                "multiply" => BlendMode::Multiply,
                "screen" => BlendMode::Screen,
                value => return Err(invalid(format!("unknown blend mode {value}"))),
            };
            let spec = BlendSpec::new(mode, parse_f32(required(object, "opacity")?, "opacity")?)?;
            Ok(EffectNode::blend(id, inputs, spec))
        }
        "custom-fragment" => parse_custom_node(id, object, shader_root, assets),
        "static-texture" => Err(invalid(
            "static-texture requires a compositor-owned immutable asset",
        )),
        value => Err(invalid(format!("unknown node kind {value}"))),
    }
}

fn parse_custom_node(
    id: EffectNodeId,
    object: &Map<String, Value>,
    shader_root: &Path,
    assets: &mut Vec<EffectShaderAsset>,
) -> Result<EffectNode, EffectConfigError> {
    reject_unknown(
        object,
        &[
            "id",
            "kind",
            "input",
            "shader",
            "module",
            "declared_footprint",
            "uniforms",
            "auxiliary_inputs",
        ],
        "custom fragment node",
    )?;
    let path = required(object, "shader")?
        .as_str()
        .ok_or_else(|| invalid("shader must be a relative path string"))?;
    let relative_path = PathBuf::from(path);
    let resolved = resolve_in_root(shader_root, &relative_path)?;
    let metadata =
        fs::metadata(&resolved).map_err(|error| EffectConfigError::ShaderIo(error.to_string()))?;
    if metadata.len() > MAX_EFFECT_SHADER_SOURCE_BYTES as u64 {
        return Err(EffectConfigError::ShaderTooLarge);
    }
    let source = fs::read_to_string(&resolved)
        .map_err(|error| EffectConfigError::ShaderIo(error.to_string()))?;
    let module = match object.get("module") {
        Some(value) => ShaderModuleId::new(parse_nonzero_u64(value, "module")?)
            .ok_or_else(|| invalid("module must be non-zero"))?,
        None => stable_shader_module_id(path),
    };
    let footprint = parse_footprint(object.get("declared_footprint"))?;
    let uniforms = object
        .get("uniforms")
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| invalid("uniforms must be an array"))?
                .iter()
                .map(|entry| {
                    let object = as_object(entry, "uniform binding")?;
                    reject_unknown(object, &["parameter", "shader_name"], "uniform binding")?;
                    Ok(EffectUniformBinding {
                        parameter: parse_parameter_id(required(object, "parameter")?, "parameter")?,
                        shader_name: required(object, "shader_name")?
                            .as_str()
                            .ok_or_else(|| invalid("shader_name must be a string"))?
                            .to_string(),
                    })
                })
                .collect::<Result<Vec<_>, EffectConfigError>>()
        })
        .transpose()?
        .unwrap_or_default();
    if uniforms.len() > MAX_EFFECT_UNIFORMS_PER_SHADER {
        return Err(EffectConfigError::LimitExceeded("uniform bindings"));
    }
    if assets.iter().any(|asset| asset.module == module) {
        return Err(invalid("duplicate shader module id"));
    }
    assets.push(EffectShaderAsset {
        module,
        relative_path,
        source,
        uniforms: uniforms.clone(),
    });
    let auxiliary_inputs = object
        .get("auxiliary_inputs")
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| invalid("auxiliary_inputs must be an array"))?
                .iter()
                .map(|value| parse_id(value, "auxiliary input"))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let spec = CustomFragmentSpec {
        shader: module,
        declared_footprint: footprint,
        uniforms,
        auxiliary_inputs,
    };
    Ok(EffectNode::custom_fragment(
        id,
        parse_id(required(object, "input")?, "input")?,
        spec,
    )?)
}

fn parse_parameters(
    value: Option<&Value>,
) -> Result<BTreeMap<String, EffectParameterDefinition>, EffectConfigError> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let object = as_object(value, "parameters")?;
    if object.len() > MAX_EFFECT_PARAMETERS {
        return Err(EffectConfigError::LimitExceeded("parameters"));
    }
    let mut result = BTreeMap::new();
    for (name, value) in object {
        validate_name(name)?;
        let parameter = as_object(value, "parameter")?;
        reject_unknown(
            parameter,
            &["id", "type", "min", "max", "default", "impact"],
            "parameter",
        )?;
        let id = parse_parameter_id(required(parameter, "id")?, "parameter id")?;
        let ty = optional_string(parameter, "type")?
            .ok_or_else(|| invalid("parameter type is required"))?;
        let default = parse_uniform_value(required(parameter, "default")?, ty)?;
        let range = match (parameter.get("min"), parameter.get("max")) {
            (Some(min), Some(max)) if ty == "float" => Some(EffectParameterRange::Float {
                min: parse_f32(min, "min")?,
                max: parse_f32(max, "max")?,
            }),
            (Some(min), Some(max)) if ty == "int" => Some(EffectParameterRange::Int {
                min: parse_i32(min, "min")?,
                max: parse_i32(max, "max")?,
            }),
            (Some(_), Some(_)) => return Err(invalid("only scalar parameters accept min/max")),
            (None, None) => None,
            _ => return Err(invalid("min and max must be specified together")),
        };
        if let Some(range) = range {
            match (range, default) {
                (EffectParameterRange::Float { min, max }, EffectUniformValue::Float(value))
                    if min <= max && value >= min && value <= max => {}
                (EffectParameterRange::Int { min, max }, EffectUniformValue::Int(value))
                    if min <= max && value >= min && value <= max => {}
                _ => return Err(invalid("parameter range or default is invalid")),
            }
        }
        let impact = match optional_string(parameter, "impact")?.unwrap_or("uniform-only") {
            "uniform-only" => EffectParameterImpact::UniformOnly,
            "footprint" => EffectParameterImpact::Footprint,
            "structure" => EffectParameterImpact::Structure,
            value => return Err(invalid(format!("unknown parameter impact {value}"))),
        };
        let spec = EffectParameterSpec {
            id,
            name: name.clone(),
            ty: parse_parameter_type(ty)?,
            range,
            impact,
        };
        if result
            .insert(name.clone(), EffectParameterDefinition { spec, default })
            .is_some()
        {
            return Err(invalid("duplicate parameter name"));
        }
    }
    Ok(result)
}

fn parse_parameter_type(value: &str) -> Result<super::EffectParameterType, EffectConfigError> {
    Ok(match value {
        "float" => super::EffectParameterType::Float,
        "vec2" => super::EffectParameterType::Vec2,
        "vec3" => super::EffectParameterType::Vec3,
        "vec4" => super::EffectParameterType::Vec4,
        "int" => super::EffectParameterType::Int,
        value => return Err(invalid(format!("unknown parameter type {value}"))),
    })
}

fn parse_uniform_value(value: &Value, ty: &str) -> Result<EffectUniformValue, EffectConfigError> {
    Ok(match ty {
        "float" => EffectUniformValue::Float(parse_f32(value, "default")?),
        "vec2" => EffectUniformValue::Vec2(parse_array::<2>(value, "default")?),
        "vec3" => EffectUniformValue::Vec3(parse_array::<3>(value, "default")?),
        "vec4" => EffectUniformValue::Vec4(parse_array::<4>(value, "default")?),
        "int" => EffectUniformValue::Int(parse_i32(value, "default")?),
        value => return Err(invalid(format!("unknown parameter type {value}"))),
    })
}

fn parse_footprint(value: Option<&Value>) -> Result<EffectFootprint, EffectConfigError> {
    let Some(value) = value else {
        return Ok(EffectFootprint::ZERO);
    };
    let object = as_object(value, "declared_footprint")?;
    reject_unknown(
        object,
        &["sample_radius_x", "sample_radius_y", "outsets"],
        "footprint",
    )?;
    Ok(EffectFootprint {
        sample_radius_x: parse_u32(required(object, "sample_radius_x")?, "sample_radius_x")?,
        sample_radius_y: parse_u32(required(object, "sample_radius_y")?, "sample_radius_y")?,
        output_outsets: parse_outsets(object.get("outsets"))?,
    })
}

fn parse_outsets(value: Option<&Value>) -> Result<super::EffectOutsets, EffectConfigError> {
    let Some(value) = value else {
        return Ok(super::EffectOutsets::ZERO);
    };
    let object = as_object(value, "outsets")?;
    reject_unknown(object, &["top", "right", "bottom", "left"], "outsets")?;
    Ok(super::EffectOutsets {
        top: parse_u32(required(object, "top")?, "top")?,
        right: parse_u32(required(object, "right")?, "right")?,
        bottom: parse_u32(required(object, "bottom")?, "bottom")?,
        left: parse_u32(required(object, "left")?, "left")?,
    })
}

fn parse_id(value: &Value, field: &str) -> Result<EffectNodeId, EffectConfigError> {
    let value = parse_nonzero_u64(value, field)?;
    let value = u16::try_from(value).map_err(|_| invalid(format!("{field} exceeds u16")))?;
    EffectNodeId::new(value).ok_or_else(|| invalid(format!("{field} must be non-zero")))
}

fn parse_parameter_id(value: &Value, field: &str) -> Result<EffectParameterId, EffectConfigError> {
    let value = u16::try_from(parse_nonzero_u64(value, field)?)
        .map_err(|_| invalid(format!("{field} exceeds u16")))?;
    EffectParameterId::new(value).ok_or_else(|| invalid(format!("{field} must be non-zero")))
}

fn parse_nonzero_u64(value: &Value, field: &str) -> Result<u64, EffectConfigError> {
    let value = value
        .as_u64()
        .ok_or_else(|| invalid(format!("{field} must be an unsigned integer")))?;
    if value == 0 {
        return Err(invalid(format!("{field} must be non-zero")));
    }
    Ok(value)
}

fn parse_u8(value: &Value, field: &str) -> Result<u8, EffectConfigError> {
    u8::try_from(parse_nonzero_u64(value, field)?)
        .map_err(|_| invalid(format!("{field} exceeds u8")))
}

fn parse_u32(value: &Value, field: &str) -> Result<u32, EffectConfigError> {
    let value = value
        .as_u64()
        .ok_or_else(|| invalid(format!("{field} must be an unsigned integer")))?;
    u32::try_from(value).map_err(|_| invalid(format!("{field} exceeds u32")))
}

fn parse_i32(value: &Value, field: &str) -> Result<i32, EffectConfigError> {
    let value = value
        .as_i64()
        .ok_or_else(|| invalid(format!("{field} must be an integer")))?;
    i32::try_from(value).map_err(|_| invalid(format!("{field} exceeds i32")))
}

fn parse_f32(value: &Value, field: &str) -> Result<f32, EffectConfigError> {
    let value = value
        .as_f64()
        .ok_or_else(|| invalid(format!("{field} must be a number")))? as f32;
    value
        .is_finite()
        .then_some(value)
        .ok_or_else(|| invalid(format!("{field} must be finite")))
}

fn parse_array<const N: usize>(value: &Value, field: &str) -> Result<[f32; N], EffectConfigError> {
    let array = value
        .as_array()
        .ok_or_else(|| invalid(format!("{field} must be an array")))?;
    if array.len() != N {
        return Err(invalid(format!("{field} must contain {N} values")));
    }
    let mut output = [0.0; N];
    for (index, value) in array.iter().enumerate() {
        output[index] = parse_f32(value, field)?;
    }
    Ok(output)
}

fn as_object<'a>(
    value: &'a Value,
    field: &str,
) -> Result<&'a Map<String, Value>, EffectConfigError> {
    value
        .as_object()
        .ok_or_else(|| invalid(format!("{field} must be an object")))
}

fn required<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a Value, EffectConfigError> {
    object
        .get(field)
        .ok_or_else(|| invalid(format!("missing required field {field}")))
}

fn optional_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<Option<&'a str>, EffectConfigError> {
    object
        .get(field)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| invalid(format!("{field} must be a string")))
        })
        .transpose()
}

fn reject_unknown(
    object: &Map<String, Value>,
    allowed: &[&str],
    context: &str,
) -> Result<(), EffectConfigError> {
    let allowed = allowed.iter().copied().collect::<BTreeSet<_>>();
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(field.as_str()))
    {
        return Err(invalid(format!("unknown {context} field {field}")));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), EffectConfigError> {
    if name.is_empty()
        || name.len() > MAX_EFFECT_NAME_BYTES
        || name
            .chars()
            .any(|c| c.is_control() || !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
    {
        return Err(EffectConfigError::InvalidName(name.to_string()));
    }
    Ok(())
}

fn stable_program_id(name: &str) -> EffectProgramId {
    EffectProgramId::new(stable_hash(name)).expect("stable effect id is non-zero")
}

fn stable_shader_module_id(path: &str) -> ShaderModuleId {
    ShaderModuleId::new(stable_hash(path)).expect("stable shader id is non-zero")
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if hash == 0 { 1 } else { hash }
}

fn invalid(message: impl Into<String>) -> EffectConfigError {
    EffectConfigError::InvalidSchema(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn manifest(nodes: &str, output: u32) -> String {
        format!(
            r#"{{"version":1,"effects":{{"glass.panel":{{"nodes":{nodes},"output":{output}}}}}}}"#
        )
    }

    #[test]
    fn accepts_bounded_named_blur_manifest() {
        let json = manifest(
            r#"[{"id":1,"kind":"backdrop"},{"id":2,"kind":"dual-kawase-blur","input":1,"radius":4.0,"passes":2,"scale":1.0}]"#,
            2,
        );
        let parsed = parse_manifest(json.as_bytes(), Path::new(".")).unwrap();
        assert_eq!(parsed.effects["glass.panel"].program.nodes.len(), 2);
    }

    #[test]
    fn rejects_unknown_version_and_missing_output() {
        let unknown = manifest(r#"[{"id":1,"kind":"backdrop"}]"#, 1)
            .replace("\"version\":1", "\"version\":2");
        assert!(matches!(
            parse_manifest(unknown.as_bytes(), Path::new(".")),
            Err(EffectConfigError::UnsupportedVersion(2))
        ));
        let missing = r#"{"version":1,"effects":{"x":{"nodes":[{"id":1,"kind":"backdrop"}]}}}"#;
        assert!(parse_manifest(missing.as_bytes(), Path::new(".")).is_err());
    }

    #[test]
    fn rejects_path_escape_and_oversized_shader() {
        assert!(validate_relative_path("../shader.frag").is_err());
        assert!(validate_relative_path("/tmp/shader.frag").is_err());
        let directory = tempfile_dir();
        let path = directory.join("large.frag");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(&vec![b'x'; MAX_EFFECT_SHADER_SOURCE_BYTES + 1])
            .unwrap();
        let json = r#"{"version":1,"effects":{"x":{"nodes":[{"id":1,"kind":"backdrop"},{"id":2,"kind":"custom-fragment","input":1,"shader":"large.frag","module":3}],"output":2}}}"#;
        assert!(matches!(
            parse_manifest(json.as_bytes(), &directory),
            Err(EffectConfigError::ShaderTooLarge)
        ));
    }

    #[test]
    fn rejects_cycle_and_limit_overflow() {
        let cycle = manifest(
            r#"[{"id":1,"kind":"tint","input":2,"color":[1,1,1,1],"amount":1.0},{"id":2,"kind":"tint","input":1,"color":[1,1,1,1],"amount":1.0}]"#,
            1,
        );
        assert!(matches!(
            parse_manifest(cycle.as_bytes(), Path::new(".")),
            Err(EffectConfigError::Validation(EffectValidationError::Cycle))
        ));
        let many = (0..=MAX_EFFECT_PROGRAM_NODES)
            .map(|id| format!(r#"{{"id":{},"kind":"backdrop"}}"#, id + 1))
            .collect::<Vec<_>>()
            .join(",");
        assert!(matches!(
            parse_manifest(manifest(&format!("[{many}]"), 1).as_bytes(), Path::new(".")),
            Err(EffectConfigError::LimitExceeded("nodes"))
        ));
    }

    fn tempfile_dir() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("typhon-effects-config-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
