use std::{collections::BTreeMap, sync::Mutex};

use crate::astrea_effects::server::{astrea_effects_manager_v1, astrea_surface_effect_v1};
use crate::effects::{
    EffectParameterBlock, EffectParameterType, EffectProgramId, EffectUniformValue,
};
use wayland_server::protocol::wl_surface;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use super::super::*;

const MAX_SLOT_BYTES: usize = 64;
const FIXED_SCALE: f64 = 65_536.0;

#[derive(Debug, Default, Clone)]
struct SurfaceEffectState {
    program_name: Option<String>,
    program: Option<EffectProgramId>,
    enabled: bool,
    values: BTreeMap<crate::effects::EffectParameterId, EffectUniformValue>,
}

#[derive(Debug)]
pub(super) struct AstreaSurfaceEffectData {
    surface_id: u32,
    surface: wl_surface::WlSurface,
    slot: SurfaceEffectSlot,
    binding_id: u64,
    state: Mutex<SurfaceEffectState>,
}

impl GlobalDispatch<astrea_effects_manager_v1::AstreaEffectsManagerV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<astrea_effects_manager_v1::AstreaEffectsManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<astrea_effects_manager_v1::AstreaEffectsManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &astrea_effects_manager_v1::AstreaEffectsManagerV1,
        request: astrea_effects_manager_v1::Request,
        _data: &(),
        handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            astrea_effects_manager_v1::Request::Destroy => {}
            astrea_effects_manager_v1::Request::GetSurfaceEffect { id, surface, slot } => {
                if !state.astrea_shell_client_allowed(client, handle) {
                    resource.post_error(
                        astrea_effects_manager_v1::Error::Unauthorized,
                        "client is not an authorized Astrea shell client",
                    );
                    return;
                }
                if !surface.id().same_client_as(&resource.id())
                    || surface.data::<SurfaceData>().is_none()
                {
                    resource.post_error(
                        astrea_effects_manager_v1::Error::InvalidSurface,
                        "surface is not owned by this client or is not a wl_surface",
                    );
                    return;
                }
                if slot.is_empty()
                    || slot.len() > MAX_SLOT_BYTES
                    || slot.chars().any(char::is_control)
                {
                    resource.post_error(
                        astrea_effects_manager_v1::Error::InvalidSlot,
                        "effect slot is outside the bounded name policy",
                    );
                    return;
                }
                let Some(slot) = SurfaceEffectSlot::parse(&slot) else {
                    resource.post_error(
                        astrea_effects_manager_v1::Error::InvalidSlot,
                        "surface effect slot is unsupported for v1",
                    );
                    return;
                };
                let surface_id = compositor_surface_id(&surface);
                let Some(binding_id) = state.claim_protocol_surface_effect(surface_id, slot) else {
                    resource.post_error(
                        astrea_effects_manager_v1::Error::EffectExists,
                        "a surface effect already occupies this surface slot",
                    );
                    return;
                };
                data_init.init(
                    id,
                    AstreaSurfaceEffectData {
                        surface_id,
                        surface,
                        slot,
                        binding_id,
                        state: Mutex::new(SurfaceEffectState::default()),
                    },
                );
            }
        }
    }
}

impl Dispatch<astrea_surface_effect_v1::AstreaSurfaceEffectV1, AstreaSurfaceEffectData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &astrea_surface_effect_v1::AstreaSurfaceEffectV1,
        request: astrea_surface_effect_v1::Request,
        data: &AstreaSurfaceEffectData,
        handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if !matches!(request, astrea_surface_effect_v1::Request::Destroy)
            && !state.astrea_shell_client_allowed(client, handle)
        {
            post_surface_error(
                resource,
                astrea_surface_effect_v1::Error::Unauthorized,
                "client is not an authorized Astrea shell client",
            );
            return;
        }

        match request {
            astrea_surface_effect_v1::Request::Destroy => {
                state.release_protocol_surface_effect(
                    SurfaceEffectBindingKey {
                        surface_id: data.surface_id,
                        slot: data.slot,
                    },
                    data.binding_id,
                );
            }
            astrea_surface_effect_v1::Request::SetProgram { name } => {
                if !data.surface.is_alive() {
                    post_surface_error(
                        resource,
                        astrea_surface_effect_v1::Error::SurfaceDestroyed,
                        "associated wl_surface is destroyed",
                    );
                    return;
                }
                let Some(program) = state.effect_program_id_for_name(&name) else {
                    post_surface_error(
                        resource,
                        astrea_surface_effect_v1::Error::UnknownProgram,
                        "effect name is not in the trusted registry",
                    );
                    return;
                };
                if !state.prepare_protocol_surface_effect_program(
                    data.binding_id,
                    data.surface_id,
                    data.slot,
                    &name,
                    program,
                ) {
                    post_surface_error(
                        resource,
                        astrea_surface_effect_v1::Error::InvalidValue,
                        "trusted effect generation changed; set_program cannot be applied",
                    );
                    return;
                }
                let defaults = state.effect_parameter_defaults(program);
                let values = defaults.iter().copied().collect::<BTreeMap<_, _>>();
                let (enabled, block) = {
                    let current = data
                        .state
                        .lock()
                        .expect("effect binding lock is not poisoned");
                    let block = parameter_block(&values);
                    (current.enabled, block)
                };
                if enabled
                    && !state.apply_protocol_surface_effect(
                        data.binding_id,
                        data.surface_id,
                        data.slot,
                        program,
                        true,
                        block,
                    )
                {
                    post_surface_error(
                        resource,
                        astrea_surface_effect_v1::Error::InvalidValue,
                        "surface is not currently renderable",
                    );
                    return;
                }
                let mut current = data
                    .state
                    .lock()
                    .expect("effect binding lock is not poisoned");
                current.program_name = Some(name);
                current.program = Some(program);
                current.values = values;
            }
            astrea_surface_effect_v1::Request::SetFloat {
                parameter,
                value_16_16,
            } => {
                update_parameter(
                    state,
                    resource,
                    data,
                    &parameter,
                    EffectUniformValue::Float(fixed(value_16_16)),
                );
            }
            astrea_surface_effect_v1::Request::SetVec2 {
                parameter,
                x_16_16,
                y_16_16,
            } => {
                update_parameter(
                    state,
                    resource,
                    data,
                    &parameter,
                    EffectUniformValue::Vec2([fixed(x_16_16), fixed(y_16_16)]),
                );
            }
            astrea_surface_effect_v1::Request::SetVec4 {
                parameter,
                x_16_16,
                y_16_16,
                z_16_16,
                w_16_16,
            } => {
                update_parameter(
                    state,
                    resource,
                    data,
                    &parameter,
                    EffectUniformValue::Vec4([
                        fixed(x_16_16),
                        fixed(y_16_16),
                        fixed(z_16_16),
                        fixed(w_16_16),
                    ]),
                );
            }
            astrea_surface_effect_v1::Request::SetEnabled { enabled } => {
                if enabled > 1 {
                    post_surface_error(
                        resource,
                        astrea_surface_effect_v1::Error::InvalidValue,
                        "enabled must be zero or one",
                    );
                    return;
                }
                let current = data
                    .state
                    .lock()
                    .expect("effect binding lock is not poisoned");
                let Some(program) = current.program else {
                    post_surface_error(
                        resource,
                        astrea_surface_effect_v1::Error::InvalidValue,
                        "set_program is required before enabling an effect",
                    );
                    return;
                };
                let block = parameter_block(&current.values);
                let want_enabled = enabled != 0;
                drop(current);
                if want_enabled
                    && !state.apply_protocol_surface_effect(
                        data.binding_id,
                        data.surface_id,
                        data.slot,
                        program,
                        true,
                        block.clone(),
                    )
                {
                    post_surface_error(
                        resource,
                        astrea_surface_effect_v1::Error::InvalidValue,
                        "surface is not currently renderable",
                    );
                    return;
                }
                if !want_enabled {
                    state.apply_protocol_surface_effect(
                        data.binding_id,
                        data.surface_id,
                        data.slot,
                        program,
                        false,
                        block,
                    );
                }
                data.state
                    .lock()
                    .expect("effect binding lock is not poisoned")
                    .enabled = want_enabled;
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: wayland_server::backend::ClientId,
        _resource: &astrea_surface_effect_v1::AstreaSurfaceEffectV1,
        data: &AstreaSurfaceEffectData,
    ) {
        state.release_protocol_surface_effect(
            SurfaceEffectBindingKey {
                surface_id: data.surface_id,
                slot: data.slot,
            },
            data.binding_id,
        );
    }
}

fn update_parameter(
    state: &mut CompositorState,
    resource: &astrea_surface_effect_v1::AstreaSurfaceEffectV1,
    data: &AstreaSurfaceEffectData,
    name: &str,
    value: EffectUniformValue,
) {
    let Some(program) = data
        .state
        .lock()
        .expect("effect binding lock is not poisoned")
        .program
    else {
        post_surface_error(
            resource,
            astrea_surface_effect_v1::Error::UnknownProgram,
            "set_program is required before setting parameters",
        );
        return;
    };
    let Some(definition) = state.effect_parameter_definition(program, name) else {
        post_surface_error(
            resource,
            astrea_surface_effect_v1::Error::UnknownParameter,
            "parameter is not declared by the trusted effect",
        );
        return;
    };
    if !parameter_matches(definition.spec.ty, value)
        || !value_in_range(definition.spec.range, value)
    {
        post_surface_error(
            resource,
            astrea_surface_effect_v1::Error::InvalidValue,
            "parameter type or range is invalid",
        );
        return;
    }
    let (enabled, values) = {
        let mut current = data
            .state
            .lock()
            .expect("effect binding lock is not poisoned");
        current.values.insert(definition.spec.id, value);
        (current.enabled, current.values.clone())
    };
    if enabled
        && !state.apply_protocol_surface_effect(
            data.binding_id,
            data.surface_id,
            data.slot,
            program,
            true,
            parameter_block(&values),
        )
    {
        post_surface_error(
            resource,
            astrea_surface_effect_v1::Error::InvalidValue,
            "surface is not currently renderable",
        );
    }
}

fn parameter_block(
    values: &BTreeMap<crate::effects::EffectParameterId, EffectUniformValue>,
) -> EffectParameterBlock {
    EffectParameterBlock::from_values(values.iter().map(|(id, value)| (*id, *value)))
        .expect("validated protocol parameter block")
}

fn parameter_matches(ty: EffectParameterType, value: EffectUniformValue) -> bool {
    matches!(
        (ty, value),
        (EffectParameterType::Float, EffectUniformValue::Float(_))
            | (EffectParameterType::Vec2, EffectUniformValue::Vec2(_))
            | (EffectParameterType::Vec3, EffectUniformValue::Vec3(_))
            | (EffectParameterType::Vec4, EffectUniformValue::Vec4(_))
            | (EffectParameterType::Int, EffectUniformValue::Int(_))
    )
}

fn value_in_range(
    range: Option<crate::effects::EffectParameterRange>,
    value: EffectUniformValue,
) -> bool {
    match (range, value) {
        (
            Some(crate::effects::EffectParameterRange::Float { min, max }),
            EffectUniformValue::Float(value),
        ) => value >= min && value <= max,
        (
            Some(crate::effects::EffectParameterRange::Float { min, max }),
            EffectUniformValue::Vec2(value),
        ) => value.iter().all(|value| *value >= min && *value <= max),
        (
            Some(crate::effects::EffectParameterRange::Float { min, max }),
            EffectUniformValue::Vec3(value),
        ) => value.iter().all(|value| *value >= min && *value <= max),
        (
            Some(crate::effects::EffectParameterRange::Float { min, max }),
            EffectUniformValue::Vec4(value),
        ) => value.iter().all(|value| *value >= min && *value <= max),
        (
            Some(crate::effects::EffectParameterRange::FloatComponents {
                min,
                max,
                components: 2,
            }),
            EffectUniformValue::Vec2(value),
        ) => value
            .iter()
            .enumerate()
            .all(|(index, value)| *value >= min[index] && *value <= max[index]),
        (
            Some(crate::effects::EffectParameterRange::FloatComponents {
                min,
                max,
                components: 3,
            }),
            EffectUniformValue::Vec3(value),
        ) => value
            .iter()
            .enumerate()
            .all(|(index, value)| *value >= min[index] && *value <= max[index]),
        (
            Some(crate::effects::EffectParameterRange::FloatComponents {
                min,
                max,
                components: 4,
            }),
            EffectUniformValue::Vec4(value),
        ) => value
            .iter()
            .enumerate()
            .all(|(index, value)| *value >= min[index] && *value <= max[index]),
        (
            Some(crate::effects::EffectParameterRange::Int { min, max }),
            EffectUniformValue::Int(value),
        ) => value >= min && value <= max,
        (None, _) => true,
        _ => false,
    }
}

fn fixed(value: i32) -> f32 {
    (f64::from(value) / FIXED_SCALE) as f32
}

fn post_surface_error(
    resource: &astrea_surface_effect_v1::AstreaSurfaceEffectV1,
    error: astrea_surface_effect_v1::Error,
    message: &str,
) {
    resource.post_error(error, message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_ranges_apply_component_wise_to_vectors() {
        let range = Some(crate::effects::EffectParameterRange::Float { min: 0.0, max: 1.0 });
        assert!(value_in_range(range, EffectUniformValue::Vec2([0.1, 0.9])));
        assert!(!value_in_range(
            range,
            EffectUniformValue::Vec3([0.1, 1.1, 0.2])
        ));
        assert!(value_in_range(
            range,
            EffectUniformValue::Vec4([0.0, 0.25, 0.5, 1.0])
        ));
    }

    #[test]
    fn component_float_ranges_apply_per_vector_component() {
        let range = Some(crate::effects::EffectParameterRange::FloatComponents {
            min: [0.0, 0.2, 0.4, 0.0],
            max: [0.1, 0.3, 0.6, 1.0],
            components: 3,
        });
        assert!(value_in_range(
            range,
            EffectUniformValue::Vec3([0.05, 0.25, 0.5])
        ));
        assert!(!value_in_range(
            range,
            EffectUniformValue::Vec3([0.05, 0.35, 0.5])
        ));
    }
}
