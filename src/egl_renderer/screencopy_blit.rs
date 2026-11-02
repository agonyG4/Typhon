//! GPU copy of a composited output frame into a client DMA-BUF.

use std::{io, ptr};

use glow::HasContext;
use khronos_egl as egl;
use oblivion_one::render_backend::{
    buffer::DmabufBufferHandle, egl_gles::EglGlesDmabufImportAttributes,
};

use super::{EglImageGuard, EglInstance, GlEglImageTargetTexture2DOes};

const EGL_LINUX_DMA_BUF_EXT: egl::Enum = 0x3270;

/// Physical-pixel rectangle of the source framebuffer, top-left origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScreencopyBlitRegion {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl ScreencopyBlitRegion {
    pub(crate) fn fits_within(self, width: u32, height: u32) -> bool {
        self.width > 0
            && self.height > 0
            && self
                .x
                .checked_add(self.width)
                .is_some_and(|right| right <= width)
            && self
                .y
                .checked_add(self.height)
                .is_some_and(|bottom| bottom <= height)
    }
}

/// A client DMA-BUF imported as a GL color attachment.
pub(crate) struct DmabufRenderTarget {
    egl_image: egl::Image,
    texture: glow::Texture,
    framebuffer: glow::Framebuffer,
}

impl DmabufRenderTarget {
    pub(crate) fn import(
        egl: &EglInstance,
        display: egl::Display,
        gl: &glow::Context,
        image_target: GlEglImageTargetTexture2DOes,
        buffer: &DmabufBufferHandle,
    ) -> io::Result<Self> {
        let attributes = EglGlesDmabufImportAttributes::from_handle(buffer).map_err(|error| {
            io::Error::other(format!("invalid screencopy DMA-BUF import: {error:?}"))
        })?;
        let no_context = unsafe { egl::Context::from_ptr(egl::NO_CONTEXT) };
        let null_buffer = unsafe { egl::ClientBuffer::from_ptr(ptr::null_mut()) };
        let image = egl
            .create_image(
                display,
                no_context,
                EGL_LINUX_DMA_BUF_EXT,
                null_buffer,
                attributes.as_slice(),
            )
            .map_err(|error| {
                io::Error::other(format!("screencopy DMA-BUF EGL import failed: {error}"))
            })?;
        let image_guard = EglImageGuard::new(image, |image| {
            let _ = egl.destroy_image(display, image);
        });
        let texture = unsafe { gl.create_texture().map_err(io::Error::other)? };
        crate::gpu_residency::record_gpu_resource_allocation(
            crate::gpu_residency::GpuResourceClass::GlTextureObject,
            None,
        );
        let delete_texture = || unsafe {
            gl.delete_texture(texture);
            crate::gpu_residency::record_gpu_resource_retirement(
                crate::gpu_residency::GpuResourceClass::GlTextureObject,
                None,
            );
        };
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            image_target(glow::TEXTURE_2D, image_guard.image().as_ptr());
            let error = gl.get_error();
            gl.bind_texture(glow::TEXTURE_2D, None);
            if error != glow::NO_ERROR {
                delete_texture();
                return Err(io::Error::other(format!(
                    "glEGLImageTargetTexture2DOES failed for screencopy DMA-BUF: 0x{error:04x}"
                )));
            }
        }
        let framebuffer = match unsafe { gl.create_framebuffer() } {
            Ok(framebuffer) => framebuffer,
            Err(error) => {
                delete_texture();
                return Err(io::Error::other(error));
            }
        };
        crate::gpu_residency::record_gpu_resource_allocation(
            crate::gpu_residency::GpuResourceClass::FramebufferObject,
            None,
        );
        let status = unsafe {
            let previous = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::DRAW_FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            let status = gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, previous);
            status
        };
        if status != glow::FRAMEBUFFER_COMPLETE {
            unsafe { gl.delete_framebuffer(framebuffer) };
            crate::gpu_residency::record_gpu_resource_retirement(
                crate::gpu_residency::GpuResourceClass::FramebufferObject,
                None,
            );
            delete_texture();
            return Err(io::Error::other(format!(
                "screencopy DMA-BUF framebuffer is incomplete: 0x{status:04x}"
            )));
        }
        Ok(Self {
            egl_image: image_guard.disarm(),
            texture,
            framebuffer,
        })
    }

    pub(crate) const fn framebuffer(&self) -> glow::Framebuffer {
        self.framebuffer
    }

    /// GL objects go before the EGLImage they reference.
    pub(crate) fn destroy(self, gl: &glow::Context, egl: &EglInstance, display: egl::Display) {
        unsafe {
            gl.delete_framebuffer(self.framebuffer);
            gl.delete_texture(self.texture);
        }
        crate::gpu_residency::record_gpu_resource_retirement(
            crate::gpu_residency::GpuResourceClass::FramebufferObject,
            None,
        );
        crate::gpu_residency::record_gpu_resource_retirement(
            crate::gpu_residency::GpuResourceClass::GlTextureObject,
            None,
        );
        let _ = egl.destroy_image(display, self.egl_image);
        crate::gpu_residency::record_gpu_resource_retirement(
            crate::gpu_residency::GpuResourceClass::ImportedEglImageView,
            None,
        );
    }
}

/// Copy `region` of `source` to the origin of `destination` without scaling.
///
/// Both framebuffers must store image row 0 at GL y = 0: the output slots are
/// drawn with `OutputFramebufferOrigin::TopLeftScanout`, and an imported
/// client DMA-BUF keeps its first memory row at texel row 0. The copy is
/// therefore unflipped and wlr-screencopy reports no `y_invert`.
pub(crate) fn blit_framebuffer_region(
    gl: &glow::Context,
    source: glow::Framebuffer,
    destination: glow::Framebuffer,
    region: ScreencopyBlitRegion,
) -> io::Result<()> {
    let to_i32 = |value: u32| {
        i32::try_from(value).map_err(|_| io::Error::other("screencopy region is out of range"))
    };
    let (x, y) = (to_i32(region.x)?, to_i32(region.y)?);
    let (width, height) = (to_i32(region.width)?, to_i32(region.height)?);
    unsafe {
        let previous_read = gl.get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING);
        let previous_draw = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
        let scissor = gl.is_enabled(glow::SCISSOR_TEST);
        if scissor {
            gl.disable(glow::SCISSOR_TEST);
        }
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(source));
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(destination));
        gl.blit_framebuffer(
            x,
            y,
            x + width,
            y + height,
            0,
            0,
            width,
            height,
            glow::COLOR_BUFFER_BIT,
            glow::NEAREST,
        );
        let error = gl.get_error();
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, previous_read);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, previous_draw);
        if scissor {
            gl.enable(glow::SCISSOR_TEST);
        }
        if error != glow::NO_ERROR {
            return Err(io::Error::other(format!(
                "screencopy blit failed: 0x{error:04x}"
            )));
        }
    }
    Ok(())
}

impl super::GlesSceneRenderer {
    /// Draw the cursor staged for cursor-overlay screencopies of the frame
    /// just rendered into `destination`, which holds `region` of that frame at
    /// its origin. A frame whose output composited the cursor itself stages
    /// none, so this is a no-op there. Every GL binding and capability it
    /// touches is restored.
    pub(crate) fn draw_capture_cursor(
        &mut self,
        destination: glow::Framebuffer,
        region: ScreencopyBlitRegion,
    ) -> io::Result<()> {
        let Some(quad) = self.scene_state.capture_cursor else {
            return Ok(());
        };
        let textures = self.resources.texture_view();
        let texture = match quad.layer {
            super::EglDrawLayer::Cursor => textures.cursor_texture(),
            super::EglDrawLayer::Surface(surface_id) => textures.texture_for_surface(surface_id),
            _ => None,
        };
        let Some(texture) = texture else {
            return Ok(());
        };
        // Both the output slot and the client buffer keep image row 0 at GL
        // y = 0 (see `blit_framebuffer_region`).
        let rect = super::EglRect::new(
            quad.rect.x() - region.x as f32,
            quad.rect.y() - region.y as f32,
            quad.rect.width(),
            quad.rect.height(),
        );
        let mut vertices = Vec::new();
        let mut commands = Vec::new();
        super::push_draw_command_with_uv(
            &mut vertices,
            &mut commands,
            quad.layer,
            rect,
            quad.uv,
            quad.sampling,
            region.width,
            region.height,
            super::OutputFramebufferOrigin::TopLeftScanout,
        );
        let Some(command) = commands.first() else {
            return Ok(());
        };
        if !command
            .bounds
            .intersects_output_rect(super::OutputRect::new(0, 0, region.width, region.height))
        {
            return Ok(());
        }
        let (width, height) = (
            i32::try_from(region.width)
                .map_err(|_| io::Error::other("screencopy region is out of range"))?,
            i32::try_from(region.height)
                .map_err(|_| io::Error::other("screencopy region is out of range"))?,
        );
        let gl = &self.gl;
        let scene = &mut self.scene_state;
        let filter = match command.sampling {
            super::SurfaceSampling::ExactNearest => glow::NEAREST,
            super::SurfaceSampling::ScaledLinear => glow::LINEAR,
        } as i32;
        unsafe {
            let saved = SavedCursorDrawState::capture(
                gl,
                scene.program,
                &scene.presentation_opacity_location,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(destination));
            gl.viewport(0, 0, width, height);
            gl.disable(glow::SCISSOR_TEST);
            gl.enable(glow::BLEND);
            gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
            gl.use_program(Some(scene.program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter);
            if let Some(location) = scene.presentation_opacity_location.as_ref() {
                gl.uniform_1_f32(Some(location), 1.0);
            }
            // The overlay geometry buffer is reused; the next frame re-uploads
            // its own cursor geometry.
            let required_size = std::mem::size_of_val(vertices.as_slice());
            super::geometry::ensure_vertex_buffer_capacity(
                gl,
                scene.overlay_vertex_buffer,
                &mut scene.overlay_vertex_buffer_capacity,
                required_size,
            );
            scene.overlay_geometry_dirty = true;
            gl.bind_vertex_array(Some(scene.overlay_vertex_array));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(scene.overlay_vertex_buffer));
            gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytemuck::cast_slice(&vertices));
            gl.draw_arrays(
                glow::TRIANGLES,
                command.vertex_start as i32,
                command.vertex_count as i32,
            );
            let error = gl.get_error();
            saved.restore(gl, &scene.presentation_opacity_location);
            if error != glow::NO_ERROR {
                return Err(io::Error::other(format!(
                    "screencopy cursor overlay failed: 0x{error:04x}"
                )));
            }
        }
        Ok(())
    }
}

/// GL state `draw_capture_cursor` changes, restored afterwards.
struct SavedCursorDrawState {
    read_framebuffer: Option<glow::Framebuffer>,
    draw_framebuffer: Option<glow::Framebuffer>,
    viewport: [i32; 4],
    scissor: bool,
    blend: bool,
    blend_func: [i32; 4],
    program: Option<glow::Program>,
    active_texture: u32,
    texture: Option<glow::Texture>,
    vertex_array: Option<glow::VertexArray>,
    array_buffer: Option<glow::Buffer>,
    opacity: Option<f32>,
}

impl SavedCursorDrawState {
    unsafe fn capture(
        gl: &glow::Context,
        scene_program: glow::Program,
        opacity_location: &Option<glow::UniformLocation>,
    ) -> Self {
        unsafe {
            let mut viewport = [0; 4];
            gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
            let active_texture = gl.get_parameter_i32(glow::ACTIVE_TEXTURE) as u32;
            gl.active_texture(glow::TEXTURE0);
            let texture = gl.get_parameter_texture(glow::TEXTURE_BINDING_2D);
            gl.active_texture(active_texture);
            let opacity = opacity_location.as_ref().map(|location| {
                let mut value = [1.0_f32];
                gl.get_uniform_f32(scene_program, location, &mut value);
                value[0]
            });
            Self {
                read_framebuffer: gl.get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING),
                draw_framebuffer: gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING),
                viewport,
                scissor: gl.is_enabled(glow::SCISSOR_TEST),
                blend: gl.is_enabled(glow::BLEND),
                blend_func: [
                    gl.get_parameter_i32(glow::BLEND_SRC_RGB),
                    gl.get_parameter_i32(glow::BLEND_DST_RGB),
                    gl.get_parameter_i32(glow::BLEND_SRC_ALPHA),
                    gl.get_parameter_i32(glow::BLEND_DST_ALPHA),
                ],
                program: gl.get_parameter_program(glow::CURRENT_PROGRAM),
                active_texture,
                texture,
                vertex_array: gl.get_parameter_vertex_array(glow::VERTEX_ARRAY_BINDING),
                array_buffer: gl.get_parameter_buffer(glow::ARRAY_BUFFER_BINDING),
                opacity,
            }
        }
    }

    unsafe fn restore(self, gl: &glow::Context, opacity_location: &Option<glow::UniformLocation>) {
        unsafe {
            if let (Some(location), Some(opacity)) = (opacity_location.as_ref(), self.opacity) {
                // `u_opacity` is program state; the scene program is current.
                gl.uniform_1_f32(Some(location), opacity);
            }
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, self.read_framebuffer);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, self.draw_framebuffer);
            let [x, y, width, height] = self.viewport;
            gl.viewport(x, y, width, height);
            if self.scissor {
                gl.enable(glow::SCISSOR_TEST);
            } else {
                gl.disable(glow::SCISSOR_TEST);
            }
            if self.blend {
                gl.enable(glow::BLEND);
            } else {
                gl.disable(glow::BLEND);
            }
            let [src_rgb, dst_rgb, src_alpha, dst_alpha] =
                self.blend_func.map(|value| value as u32);
            gl.blend_func_separate(src_rgb, dst_rgb, src_alpha, dst_alpha);
            gl.use_program(self.program);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, self.texture);
            gl.active_texture(self.active_texture);
            gl.bind_vertex_array(self.vertex_array);
            gl.bind_buffer(glow::ARRAY_BUFFER, self.array_buffer);
        }
    }
}
