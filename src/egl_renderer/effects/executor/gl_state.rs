use glow::HasContext;

use super::super::CanonicalBlurSemanticsPolicy;

pub(crate) struct DitherStateRestore<'a> {
    gl: &'a glow::Context,
    previous: bool,
}

impl Drop for DitherStateRestore<'_> {
    fn drop(&mut self) {
        unsafe {
            if self.previous {
                self.gl.enable(glow::DITHER);
            } else {
                self.gl.disable(glow::DITHER);
            }
        }
    }
}

pub(crate) fn set_dither_state_with_restore(
    gl: &glow::Context,
    enabled: bool,
) -> DitherStateRestore<'_> {
    let previous = unsafe { gl.is_enabled(glow::DITHER) };
    unsafe {
        if enabled {
            gl.enable(glow::DITHER);
        } else {
            gl.disable(glow::DITHER);
        }
    }
    DitherStateRestore { gl, previous }
}

pub(crate) fn canonical_offscreen_dither_restore(
    gl: &glow::Context,
    policy: CanonicalBlurSemanticsPolicy,
    offscreen: bool,
) -> Option<DitherStateRestore<'_>> {
    (policy == CanonicalBlurSemanticsPolicy::Canonical && offscreen)
        .then(|| set_dither_state_with_restore(gl, false))
}
