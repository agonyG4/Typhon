mod commands;
mod identity;

#[cfg(test)]
pub(in crate::egl_renderer) use commands::rgba_to_pixel;
pub(in crate::egl_renderer) use commands::{
    push_egl_decoration_instance, push_egl_surface_commands, push_output_background_command,
};
pub(in crate::egl_renderer) use identity::{
    EglSceneCacheKey, EglSceneSurfaceSignature, egl_scene_surface_signatures,
};
