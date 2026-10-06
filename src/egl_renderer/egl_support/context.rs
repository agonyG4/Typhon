use std::{error::Error, io};

use khronos_egl as egl;

use super::super::{EglInstance, GlEglImageTargetTexture2DOes, RendererResult};

pub(crate) fn create_gles_context(
    egl: &EglInstance,
    display: egl::Display,
    config: egl::Config,
) -> RendererResult<egl::Context> {
    egl.create_context(display, config, None, gles_context_attributes())
        .map_err(|error| io::Error::other(format_gles3_context_error(&error)).into())
}

pub(in crate::egl_renderer) fn gles_context_attributes() -> &'static [egl::Int] {
    &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE]
}

pub(in crate::egl_renderer) fn format_gles3_context_error(error: &dyn Error) -> String {
    format!("failed to create required GLES3 context: {error}")
}

pub(crate) fn load_egl_image_target_texture_2d(
    egl: &EglInstance,
) -> Option<GlEglImageTargetTexture2DOes> {
    let symbol = egl.get_proc_address("glEGLImageTargetTexture2DOES")?;
    Some(unsafe {
        std::mem::transmute::<extern "system" fn(), GlEglImageTargetTexture2DOes>(symbol)
    })
}
