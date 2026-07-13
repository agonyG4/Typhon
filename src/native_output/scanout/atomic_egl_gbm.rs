use std::{
    fs, io, iter,
    os::fd::{AsFd, AsRawFd},
    ptr,
};

use gbm::AsRaw as _;
use glow::HasContext;
use khronos_egl as egl;
use oblivion_one::native::kms::{AtomicDiscovery, DrmFormatModifierPair};

use crate::egl_renderer::dmabuf::query_egl_renderable_dmabuf_formats;
use crate::egl_renderer::{
    EglInstance, choose_native_egl_config, create_gles_context, load_egl_image_target_texture_2d,
};

use super::*;

pub(crate) struct AtomicEglGbmScanout {
    _device: gbm::Device<std::os::fd::OwnedFd>,
    egl: EglInstance,
    egl_display: egl::Display,
    egl_context: egl::Context,
    gl: glow::Context,
    pool: Option<AtomicOutputPool>,
    pub(crate) format_modifier: DrmFormatModifierPair,
    drm_cleanup_armed: bool,
}

struct DeviceAllocationProbe<'a> {
    device: &'a gbm::Device<std::os::fd::OwnedFd>,
    width: u32,
    height: u32,
}

impl GbmAllocationProbe for DeviceAllocationProbe<'_> {
    fn supports(&mut self, candidate: DrmFormatModifierPair) -> bool {
        let Ok(format) = gbm::Format::try_from(candidate.fourcc) else {
            return false;
        };
        self.device
            .create_buffer_object_with_modifiers2::<()>(
                self.width,
                self.height,
                format,
                iter::once(gbm::Modifier::from(candidate.modifier)),
                gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING,
            )
            .is_ok()
    }
}

impl AtomicEglGbmScanout {
    pub(crate) fn create_unattached_pool(
        kms: &fs::File,
        discovery: &AtomicDiscovery,
        width: u32,
        height: u32,
        pool_generation: u64,
    ) -> io::Result<Self> {
        let gbm_fd = duplicate_fd_cloexec(kms.as_raw_fd()).map_err(io::Error::from_raw_os_error)?;
        let device = gbm::Device::new(gbm_fd)?;
        let egl = unsafe { EglInstance::load_required() }.map_err(native_egl_io_error)?;
        const EGL_PLATFORM_GBM_KHR: egl::Enum = 0x31d7;
        let egl_display = unsafe {
            egl.get_platform_display(
                EGL_PLATFORM_GBM_KHR,
                device.as_raw_mut() as egl::NativeDisplayType,
                &[egl::ATTRIB_NONE],
            )
        }
        .map_err(native_egl_io_error)?;
        egl.initialize(egl_display).map_err(native_egl_io_error)?;
        let mut created_context = None;
        let result = (|| {
            egl.bind_api(egl::OPENGL_ES_API)
                .map_err(native_egl_io_error)?;
            let extensions = egl
                .query_string(Some(egl_display), egl::EXTENSIONS)
                .map_err(native_egl_io_error)?
                .to_string_lossy();
            for required in [
                "EGL_KHR_surfaceless_context",
                "EGL_KHR_image_base",
                "EGL_EXT_image_dma_buf_import",
                "EGL_EXT_image_dma_buf_import_modifiers",
            ] {
                if !extensions
                    .split_ascii_whitespace()
                    .any(|entry| entry == required)
                {
                    return Err(io::Error::other(format!(
                        "explicit Atomic EGL/GBM requires {required}"
                    )));
                }
            }

            let egl_formats = query_egl_renderable_dmabuf_formats(&egl, egl_display);
            let mut probe = DeviceAllocationProbe {
                device: &device,
                width,
                height,
            };
            let format_modifier = select_output_format_modifier(
                &discovery.plane_scanout_formats,
                &egl_formats,
                &mut probe,
            )?;
            let config = choose_native_egl_config(&egl, egl_display, format_modifier.fourcc)
                .map_err(native_egl_io_error)?;
            let egl_context =
                create_gles_context(&egl, egl_display, config).map_err(native_egl_io_error)?;
            created_context = Some(egl_context);
            if let Err(error) = egl.make_current(egl_display, None, None, Some(egl_context)) {
                return Err(native_egl_io_error(error));
            }
            let image_target = load_egl_image_target_texture_2d(&egl).ok_or_else(|| {
                io::Error::other("explicit Atomic EGL/GBM requires GL_OES_EGL_image")
            })?;
            let gl = unsafe {
                glow::Context::from_loader_function(|name| {
                    egl.get_proc_address(name)
                        .map(|symbol| symbol as *const _)
                        .unwrap_or(ptr::null())
                })
            };
            let format = gbm::Format::try_from(format_modifier.fourcc)
                .map_err(|_| io::Error::other("selected output FourCC is unsupported by GBM"))?;
            let usage = gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING;
            let drm = kms.as_fd();
            let mut slots = Vec::with_capacity(EXPLICIT_OUTPUT_SLOT_CAPACITY);
            for raw_id in 0..EXPLICIT_OUTPUT_SLOT_CAPACITY {
                let slot = (|| {
                    let bo = device.create_buffer_object_with_modifiers2::<()>(
                        width,
                        height,
                        format,
                        iter::once(gbm::Modifier::from(format_modifier.modifier)),
                        usage,
                    )?;
                    let descriptor = explicit_framebuffer_descriptor(&bo)?;
                    let framebuffer = add_explicit_framebuffer(drm, &descriptor)?;
                    let id = OutputSlotId::new(u8::try_from(raw_id).unwrap()).unwrap();
                    AtomicOutputSlot::import(
                        id,
                        pool_generation,
                        bo,
                        framebuffer,
                        &egl,
                        egl_display,
                        &gl,
                        image_target,
                    )
                    .inspect_err(|_| {
                        let _ = drm_ffi::mode::rm_fb(drm, framebuffer.get());
                    })
                })();
                match slot {
                    Ok(slot) => slots.push(slot),
                    Err(error) => {
                        teardown_atomic_slots(&slots, &gl, &egl, egl_display, drm);
                        return Err(error);
                    }
                }
            }
            if let Err(error) = AtomicOutputPool::validate_slots(&slots, pool_generation) {
                teardown_atomic_slots(&slots, &gl, &egl, egl_display, drm);
                return Err(error);
            }
            let slots: [AtomicOutputSlot; EXPLICIT_OUTPUT_SLOT_CAPACITY] = match slots.try_into() {
                Ok(slots) => slots,
                Err(slots) => {
                    teardown_atomic_slots(&slots, &gl, &egl, egl_display, drm);
                    return Err(io::Error::other(
                        "explicit output pool did not construct 3 slots",
                    ));
                }
            };
            let pool = AtomicOutputPool::from_validated_slots(slots, pool_generation);
            Ok((egl_context, gl, pool, format_modifier))
        })();

        match result {
            Ok((egl_context, gl, pool, format_modifier)) => Ok(Self {
                _device: device,
                egl,
                egl_display,
                egl_context,
                gl,
                pool: Some(pool),
                format_modifier,
                drm_cleanup_armed: true,
            }),
            Err(error) => {
                let _ = egl.make_current(egl_display, None, None, None);
                if let Some(context) = created_context {
                    let _ = egl.destroy_context(egl_display, context);
                }
                let _ = egl.terminate(egl_display);
                Err(error)
            }
        }
    }
}

impl Drop for AtomicEglGbmScanout {
    fn drop(&mut self) {
        let _ = self
            .egl
            .make_current(self.egl_display, None, None, Some(self.egl_context));
        if let Some(pool) = self.pool.take() {
            let drm = self._device.as_fd();
            if self.drm_cleanup_armed {
                pool.teardown(&self.gl, &self.egl, self.egl_display, drm);
            } else {
                // GL/EGL resources must still be deleted; revoked DRM skips rm_fb.
                unsafe {
                    for slot in &pool.slots {
                        self.gl.delete_framebuffer(slot.gl_framebuffer);
                        self.gl.delete_texture(slot.texture);
                    }
                }
                for slot in &pool.slots {
                    let _ = self.egl.destroy_image(self.egl_display, slot.egl_image);
                }
                drop(pool);
            }
        }
        let _ = self.egl.make_current(self.egl_display, None, None, None);
        let _ = self.egl.destroy_context(self.egl_display, self.egl_context);
        let _ = self.egl.terminate(self.egl_display);
    }
}
