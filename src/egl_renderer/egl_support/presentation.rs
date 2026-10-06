use std::io;

use khronos_egl as egl;

use super::super::{
    EglInstance, EglOutputDamage, EglPartialRepaintCapabilities, RendererResult, damage::BufferAge,
    native_egl_debug_enabled,
};

pub(crate) type EglSwapBuffersWithDamage = unsafe extern "system" fn(
    egl::EGLDisplay,
    egl::EGLSurface,
    *const egl::Int,
    egl::Int,
) -> egl::Boolean;

const EGL_BUFFER_AGE_EXT: egl::Int = 0x313d;

pub(crate) fn load_swap_buffers_with_damage(
    egl: &EglInstance,
    display: egl::Display,
) -> Option<EglSwapBuffersWithDamage> {
    let extensions = egl
        .query_string(Some(display), egl::EXTENSIONS)
        .ok()?
        .to_string_lossy();
    let mut extensions = extensions.split_ascii_whitespace();
    let has_khr = extensions
        .clone()
        .any(|extension| extension == "EGL_KHR_swap_buffers_with_damage");
    let has_ext = extensions.any(|extension| extension == "EGL_EXT_swap_buffers_with_damage");
    let symbol_name = if has_khr {
        "eglSwapBuffersWithDamageKHR"
    } else if has_ext {
        "eglSwapBuffersWithDamageEXT"
    } else {
        return None;
    };
    let symbol = egl.get_proc_address(symbol_name)?;
    Some(unsafe { std::mem::transmute::<extern "system" fn(), EglSwapBuffersWithDamage>(symbol) })
}

pub(crate) fn detect_partial_repaint_capabilities(
    egl: &EglInstance,
    display: egl::Display,
    target_lineage_available: bool,
    partial_render_repair: bool,
    swap_buffers_with_damage: bool,
) -> EglPartialRepaintCapabilities {
    let extensions = egl
        .query_string(Some(display), egl::EXTENSIONS)
        .map(|extensions| extensions.to_string_lossy().into_owned())
        .unwrap_or_default();
    EglPartialRepaintCapabilities {
        buffer_age: !buffer_age_disabled()
            && (target_lineage_available
                || extensions
                    .split_ascii_whitespace()
                    .any(|extension| extension == "EGL_EXT_buffer_age")),
        partial_render_repair,
        swap_buffers_with_damage,
    }
}

pub(in crate::egl_renderer) fn query_egl_buffer_age(
    egl: &EglInstance,
    display: egl::Display,
    surface: egl::Surface,
    supported: bool,
) -> BufferAge {
    if !supported {
        return BufferAge::Unsupported;
    }
    match egl.query_surface(display, surface, EGL_BUFFER_AGE_EXT) {
        Ok(age) => BufferAge::Value(age),
        Err(error) => {
            if native_egl_debug_enabled() {
                eprintln!("EGL buffer-age query failed: {error}");
            }
            BufferAge::QueryFailed
        }
    }
}

fn buffer_age_disabled() -> bool {
    std::env::var_os("OBLIVION_ONE_DISABLE_BUFFER_AGE").is_some_and(|value| value == "1")
}

pub(crate) fn egl_swap_buffers_with_damage(
    egl: &EglInstance,
    display: egl::Display,
    surface: egl::Surface,
    swap_buffers_with_damage: Option<EglSwapBuffersWithDamage>,
    damage: &EglOutputDamage,
    output_size: (u32, u32),
) -> RendererResult<()> {
    let Some(swap_buffers_with_damage) = swap_buffers_with_damage else {
        egl.swap_buffers(display, surface)?;
        return Ok(());
    };
    let Some(rects) = damage.to_egl_rects(output_size.0, output_size.1) else {
        egl.swap_buffers(display, surface)?;
        return Ok(());
    };

    let ok = unsafe {
        swap_buffers_with_damage(
            display.as_ptr(),
            surface.as_ptr(),
            rects.as_ptr(),
            rects.rect_count() as egl::Int,
        )
    };
    if ok == egl::TRUE {
        Ok(())
    } else {
        Err(egl
            .get_error()
            .map(|error| io::Error::other(format!("eglSwapBuffersWithDamage failed: {error}")))
            .unwrap_or_else(|| io::Error::other("eglSwapBuffersWithDamage failed"))
            .into())
    }
}
