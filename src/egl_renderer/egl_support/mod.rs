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
#[cfg(test)]
pub(in crate::egl_renderer) use context::create_gles_context;
pub(crate) use context::{
    CreatedGlesContext, EglContextPriorityPolicy, create_compositor_gles_context,
    load_egl_image_target_texture_2d,
};
#[cfg(test)]
pub(in crate::egl_renderer) use context::{
    EGL_CONTEXT_PRIORITY_HIGH_IMG, EGL_CONTEXT_PRIORITY_LEVEL_IMG, EGL_CONTEXT_PRIORITY_LOW_IMG,
    EGL_CONTEXT_PRIORITY_MEDIUM_IMG, EglContextCreationCandidate, EglContextPriorityLevel,
    EglPriorityExtensionSupport, create_context_from_candidates, format_gles3_context_error,
    gles_context_attributes, has_egl_img_context_priority, plan_gles_context_candidates,
    retain_context_when_priority_query_fails,
};
pub(in crate::egl_renderer) use presentation::query_egl_buffer_age;
pub(crate) use presentation::{
    EglSwapBuffersWithDamage, detect_partial_repaint_capabilities, egl_swap_buffers_with_damage,
    load_swap_buffers_with_damage,
};
