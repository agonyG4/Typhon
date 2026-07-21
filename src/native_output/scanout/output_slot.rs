use std::{io, os::fd::BorrowedFd, ptr};

use glow::HasContext;
use khronos_egl as egl;
use oblivion_one::{
    native::kms::FramebufferId,
    render_backend::{
        buffer::{
            BufferSize, DmabufBufferHandle, DmabufPlane, DmabufPlaneDescriptor, DrmFormat,
            DrmModifier,
        },
        egl_gles::EglGlesDmabufImportAttributes,
    },
};

use crate::egl_renderer::{
    BufferAge, EglInstance, GlEglImageTargetTexture2DOes, render_target_buffer_age,
};

use super::{
    EXPLICIT_OUTPUT_SLOT_CAPACITY, ExplicitFramebufferDescriptor, ExplicitFramebufferPlane,
    OutputSlotId,
};

const EGL_LINUX_DMA_BUF_EXT: egl::Enum = 0x3270;

pub(crate) struct AtomicOutputSlot {
    pub(crate) id: OutputSlotId,
    pub(crate) pool_generation: u64,
    pub(crate) bo: gbm::BufferObject<()>,
    pub(crate) framebuffer: FramebufferId,
    pub(crate) egl_image: egl::Image,
    pub(crate) texture: glow::Texture,
    pub(crate) gl_framebuffer: glow::Framebuffer,
    pub(crate) last_presented_serial: Option<u64>,
}

impl AtomicOutputSlot {
    pub(crate) fn buffer_age(
        &self,
        presentation_serial: u64,
        presentation_pending: bool,
    ) -> BufferAge {
        render_target_buffer_age(
            presentation_serial,
            self.last_presented_serial,
            presentation_pending,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn import(
        id: OutputSlotId,
        pool_generation: u64,
        bo: gbm::BufferObject<()>,
        framebuffer: FramebufferId,
        egl: &EglInstance,
        egl_display: egl::Display,
        gl: &glow::Context,
        image_target: GlEglImageTargetTexture2DOes,
    ) -> io::Result<Self> {
        let size = BufferSize::new(bo.width(), bo.height())
            .ok_or_else(|| io::Error::other("explicit output BO dimensions are zero"))?;
        let plane_count = usize::try_from(bo.plane_count())
            .map_err(|_| io::Error::other("explicit output BO plane count overflow"))?;
        if !(1..=4).contains(&plane_count) {
            return Err(io::Error::other(
                "explicit output BO plane count must be between one and four",
            ));
        }
        let modifier = DrmModifier(u64::from(bo.modifier()));
        if modifier == DrmModifier::INVALID {
            return Err(io::Error::other(
                "explicit output BO unexpectedly has DRM_FORMAT_MOD_INVALID",
            ));
        }
        let mut planes = Vec::with_capacity(plane_count);
        for plane_index in 0..plane_count {
            let raw_index = i32::try_from(plane_index)
                .map_err(|_| io::Error::other("explicit output plane index overflow"))?;
            let fd = bo
                .fd_for_plane(raw_index)
                .map_err(|_| io::Error::other("failed to export explicit output plane FD"))?;
            planes.push(DmabufPlane::new(
                fd,
                DmabufPlaneDescriptor {
                    plane_index: u32::try_from(plane_index).unwrap(),
                    offset: bo.offset(raw_index),
                    stride: bo.stride_for_plane(raw_index),
                    modifier,
                },
            ));
        }
        let handle =
            DmabufBufferHandle::new(size, DrmFormat::from_fourcc(bo.format() as u32), planes)
                .map_err(|error| {
                    io::Error::other(format!("invalid explicit output BO: {error:?}"))
                })?;
        let attributes = EglGlesDmabufImportAttributes::from_handle(&handle)
            .map_err(|error| io::Error::other(format!("invalid EGL output import: {error:?}")))?;
        let no_context = unsafe { egl::Context::from_ptr(egl::NO_CONTEXT) };
        let null_buffer = unsafe { egl::ClientBuffer::from_ptr(ptr::null_mut()) };
        let egl_image = egl
            .create_image(
                egl_display,
                no_context,
                EGL_LINUX_DMA_BUF_EXT,
                null_buffer,
                attributes.as_slice(),
            )
            .map_err(|error| {
                io::Error::other(format!("EGL output image import failed: {error}"))
            })?;

        let texture = unsafe { gl.create_texture().map_err(io::Error::other)? };
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            image_target(glow::TEXTURE_2D, egl_image.as_ptr());
            if gl.get_error() != glow::NO_ERROR {
                gl.delete_texture(texture);
                let _ = egl.destroy_image(egl_display, egl_image);
                return Err(io::Error::other(
                    "glEGLImageTargetTexture2DOES failed for explicit output BO",
                ));
            }
        }
        let gl_framebuffer = match unsafe { gl.create_framebuffer() } {
            Ok(framebuffer) => framebuffer,
            Err(error) => {
                unsafe { gl.delete_texture(texture) };
                let _ = egl.destroy_image(egl_display, egl_image);
                return Err(io::Error::other(error));
            }
        };
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(gl_framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(gl_framebuffer);
                gl.delete_texture(texture);
                let _ = egl.destroy_image(egl_display, egl_image);
                return Err(io::Error::other("explicit output FBO is incomplete"));
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        Ok(Self {
            id,
            pool_generation,
            bo,
            framebuffer,
            egl_image,
            texture,
            gl_framebuffer,
            last_presented_serial: None,
        })
    }
}

pub(crate) fn explicit_framebuffer_descriptor(
    bo: &gbm::BufferObject<()>,
) -> io::Result<ExplicitFramebufferDescriptor> {
    let plane_count = usize::try_from(bo.plane_count())
        .map_err(|_| io::Error::other("explicit output BO plane count overflow"))?;
    if !(1..=4).contains(&plane_count) {
        return Err(io::Error::other(
            "explicit output BO plane count must be between one and four",
        ));
    }
    let modifier = u64::from(bo.modifier());
    let mut planes = Vec::with_capacity(plane_count);
    for plane_index in 0..plane_count {
        let raw_index = i32::try_from(plane_index)
            .map_err(|_| io::Error::other("explicit output BO plane index overflow"))?;
        planes.push(ExplicitFramebufferPlane {
            handle: unsafe { bo.handle_for_plane(raw_index).u32_ },
            pitch: bo.stride_for_plane(raw_index),
            offset: bo.offset(raw_index),
            modifier,
        });
    }
    ExplicitFramebufferDescriptor::new(bo.width(), bo.height(), bo.format() as u32, &planes)
}

pub(crate) fn add_explicit_framebuffer(
    drm: BorrowedFd<'_>,
    descriptor: &ExplicitFramebufferDescriptor,
) -> io::Result<FramebufferId> {
    let framebuffer = drm_ffi::mode::add_fb2(
        drm,
        descriptor.width(),
        descriptor.height(),
        descriptor.format(),
        descriptor.handles(),
        descriptor.pitches(),
        descriptor.offsets(),
        descriptor.modifiers(),
        descriptor.flags(),
    )?;
    FramebufferId::new(framebuffer.fb_id)
        .ok_or_else(|| io::Error::other("AddFB2 returned framebuffer ID zero"))
}

pub(crate) struct AtomicOutputPool {
    pub(crate) slots: [AtomicOutputSlot; EXPLICIT_OUTPUT_SLOT_CAPACITY],
    pub(crate) pool_generation: u64,
}

impl AtomicOutputPool {
    pub(crate) fn validate_slots(
        slots: &[AtomicOutputSlot],
        pool_generation: u64,
    ) -> io::Result<()> {
        let first = slots
            .first()
            .ok_or_else(|| io::Error::other("explicit output pool has no slots"))?;
        if slots.iter().any(|slot| {
            slot.pool_generation != pool_generation
                || slot.bo.width() != first.bo.width()
                || slot.bo.height() != first.bo.height()
                || slot.bo.format() != first.bo.format()
                || slot.bo.modifier() != first.bo.modifier()
                || slot.bo.plane_count() != first.bo.plane_count()
        }) {
            return Err(io::Error::other(
                "explicit output pool slots do not share identical metadata",
            ));
        }
        Ok(())
    }

    pub(crate) fn from_validated_slots(
        slots: [AtomicOutputSlot; EXPLICIT_OUTPUT_SLOT_CAPACITY],
        pool_generation: u64,
    ) -> Self {
        Self {
            slots,
            pool_generation,
        }
    }

    pub(crate) fn teardown(
        self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        drm: BorrowedFd<'_>,
    ) {
        teardown_atomic_slots(&self.slots, gl, egl, egl_display, drm);
        drop(self)
    }
}

pub(crate) fn teardown_atomic_slots(
    slots: &[AtomicOutputSlot],
    gl: &glow::Context,
    egl: &EglInstance,
    egl_display: egl::Display,
    drm: BorrowedFd<'_>,
) {
    teardown_slot_resources(
        slots,
        |slot| unsafe {
            gl.delete_framebuffer(slot.gl_framebuffer);
            gl.delete_texture(slot.texture);
        },
        |slot| {
            let _ = egl.destroy_image(egl_display, slot.egl_image);
        },
        |slot| {
            let _ = drm_ffi::mode::rm_fb(drm, slot.framebuffer.get());
        },
    );
}

pub(crate) fn teardown_slot_resources<T>(
    slots: &[T],
    mut delete_gl: impl FnMut(&T),
    mut destroy_image: impl FnMut(&T),
    mut remove_framebuffer: impl FnMut(&T),
) {
    for slot in slots {
        delete_gl(slot);
    }
    for slot in slots {
        destroy_image(slot);
    }
    for slot in slots {
        remove_framebuffer(slot);
    }
}
