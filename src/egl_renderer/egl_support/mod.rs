mod config;
mod context;
mod presentation;

pub(in crate::egl_renderer) use config::native_egl_debug_enabled;
#[cfg(test)]
pub(in crate::egl_renderer) use config::select_native_egl_visual_format;
#[cfg(test)]
pub(in crate::egl_renderer) use config::{
    NativeEglConfigCandidate, native_egl_config_candidate_matches,
    native_egl_config_candidate_matches_common, select_native_egl_config_candidate,
};
pub(crate) use config::{
    choose_native_egl_config, choose_surfaceless_egl_config, native_visual_label,
};
pub(crate) use context::{create_gles_context, load_egl_image_target_texture_2d};
#[cfg(test)]
pub(in crate::egl_renderer) use context::{format_gles3_context_error, gles_context_attributes};
pub(in crate::egl_renderer) use presentation::query_egl_buffer_age;
pub(crate) use presentation::{
    EglSwapBuffersWithDamage, detect_partial_repaint_capabilities, egl_swap_buffers_with_damage,
    load_swap_buffers_with_damage,
};
