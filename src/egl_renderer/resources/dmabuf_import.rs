use std::{error::Error, io, ptr};

use glow::HasContext;
use khronos_egl as egl;
use oblivion_one::{
    compositor::RenderableSurface,
    render_backend::{
        buffer::DrmModifier,
        egl_gles::{EGL_LINUX_DMA_BUF_EXT, EglGlesDmabufImportAttributes, EglGlesImportError},
    },
};

use super::super::{
    EglInstance, GlEglImageTargetTexture2DOes, RendererResult, native_egl_debug_enabled,
};
use super::ResourceTelemetry;
use super::image::{EglImageGuard, EglImageResource, configure_texture};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DmabufImportGlStage {
    TextureCreation,
    Bind,
    TextureConfiguration,
    ImageTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DmabufImportFailureClass {
    BufferIncompatible,
    RendererFatal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DmabufImportPath {
    Initial,
    Replacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DmabufImportCacheState {
    NotChecked,
    Miss,
    Hit,
}

#[derive(Debug)]
pub(super) enum DmabufTextureImportError {
    InvalidAttributes(EglGlesImportError),
    EglImageCreation(egl::Error),
    TextureCreation(String),
    Gl {
        stage: DmabufImportGlStage,
        error: u32,
    },
}

impl DmabufTextureImportError {
    pub(super) fn classification(&self) -> DmabufImportFailureClass {
        match self {
            Self::InvalidAttributes(_) => DmabufImportFailureClass::BufferIncompatible,
            Self::EglImageCreation(
                egl::Error::BadAttribute
                | egl::Error::BadMatch
                | egl::Error::BadNativePixmap
                | egl::Error::BadParameter,
            ) => DmabufImportFailureClass::BufferIncompatible,
            Self::Gl {
                stage: DmabufImportGlStage::ImageTarget,
                error: glow::INVALID_OPERATION,
            } => DmabufImportFailureClass::BufferIncompatible,
            _ => DmabufImportFailureClass::RendererFatal,
        }
    }

    fn stage_name(&self) -> &'static str {
        match self {
            Self::InvalidAttributes(_) => "attributes",
            Self::EglImageCreation(_) => "egl_create_image",
            Self::TextureCreation(_) => "texture_creation",
            Self::Gl { stage, .. } => match stage {
                DmabufImportGlStage::TextureCreation => "texture_creation",
                DmabufImportGlStage::Bind => "bind",
                DmabufImportGlStage::TextureConfiguration => "texture_configuration",
                DmabufImportGlStage::ImageTarget => "image_target",
            },
        }
    }

    fn error_code(&self) -> Option<u32> {
        match self {
            Self::EglImageCreation(error) => Some(error.native() as u32),
            Self::Gl { error, .. } => Some(*error),
            Self::InvalidAttributes(_) | Self::TextureCreation(_) => None,
        }
    }

    fn egl_image_created(&self) -> bool {
        matches!(self, Self::TextureCreation(_) | Self::Gl { .. })
    }
}

impl std::fmt::Display for DmabufTextureImportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAttributes(error) => {
                write!(formatter, "invalid DMA-BUF import attributes: {error:?}")
            }
            Self::EglImageCreation(error) => write!(formatter, "eglCreateImage failed: {error}"),
            Self::TextureCreation(error) => write!(formatter, "texture creation failed: {error}"),
            Self::Gl { stage, error } => {
                write!(formatter, "GL {:?} failed with error 0x{error:04x}", stage)
            }
        }
    }
}

impl Error for DmabufTextureImportError {}

#[derive(Debug, Clone, Copy)]
pub(super) struct DmabufImportDiagnosticContext {
    pub(super) surface_id: u32,
    pub(super) generation: u64,
    pub(super) buffer_id: u64,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) fourcc: u32,
    pub(super) modifier: u64,
    pub(super) planes: usize,
    pub(super) path: DmabufImportPath,
    pub(super) cache: DmabufImportCacheState,
}

impl DmabufImportDiagnosticContext {
    pub(super) fn from_surface(
        surface: &RenderableSurface,
        path: DmabufImportPath,
        cache: DmabufImportCacheState,
    ) -> Option<Self> {
        let handle = surface.dmabuf_handle()?;
        let modifier = handle
            .planes()
            .first()
            .map(|plane| plane.descriptor().modifier.0)?;
        let size = handle.size();
        Some(Self {
            surface_id: surface.surface_id,
            generation: surface.generation,
            buffer_id: surface.buffer_id().get(),
            width: size.width,
            height: size.height,
            fourcc: handle.format().as_fourcc(),
            modifier,
            planes: handle.planes().len(),
            path,
            cache,
        })
    }
}

pub(super) fn log_dmabuf_import_context(
    context: DmabufImportDiagnosticContext,
    event: &'static str,
    stage: &'static str,
    egl_image_created: bool,
    error_code: Option<u32>,
    classification: &'static str,
) {
    let error_code = error_code.map_or_else(|| "none".to_owned(), |code| format!("0x{code:04x}"));
    eprintln!(
        "oblivion-one compositor: dmabuf {event}: surface={} generation={} buffer_id={} size={}x{} fourcc=0x{:08x} modifier=0x{:016x} implicit={} planes={} path={:?} cache={:?} egl_image={} stage={} gl_or_egl_error={} classification={classification}",
        context.surface_id,
        context.generation,
        context.buffer_id,
        context.width,
        context.height,
        context.fourcc,
        context.modifier,
        context.modifier == DrmModifier::INVALID.0,
        context.planes,
        context.path,
        context.cache,
        egl_image_created,
        stage,
        error_code,
    );
}

pub(super) fn settle_dmabuf_import_result<T>(
    result: RendererResult<T>,
    context: DmabufImportDiagnosticContext,
    telemetry: &mut ResourceTelemetry<'_>,
    failed_surface_generations: &mut std::collections::HashMap<u32, u64>,
) -> RendererResult<Option<T>> {
    match result {
        Ok(resource) => {
            failed_surface_generations.remove(&context.surface_id);
            if native_egl_debug_enabled() {
                log_dmabuf_import_context(
                    context,
                    "import_accepted",
                    "complete",
                    true,
                    None,
                    "success",
                );
            }
            Ok(Some(resource))
        }
        Err(error) => {
            let Some(import_error) = error.downcast_ref::<DmabufTextureImportError>() else {
                return Err(error);
            };
            let classification = import_error.classification();
            let should_log = failed_surface_generations
                .get(&context.surface_id)
                .is_none_or(|generation| *generation != context.generation);
            if should_log || classification == DmabufImportFailureClass::RendererFatal {
                log_dmabuf_import_context(
                    context,
                    "import_rejected",
                    import_error.stage_name(),
                    import_error.egl_image_created(),
                    import_error.error_code(),
                    match classification {
                        DmabufImportFailureClass::BufferIncompatible => "buffer_incompatible",
                        DmabufImportFailureClass::RendererFatal => "renderer_fatal",
                    },
                );
            }
            if classification == DmabufImportFailureClass::RendererFatal {
                return Err(error);
            }
            telemetry.dmabuf_import_failure();
            if should_log {
                failed_surface_generations.insert(context.surface_id, context.generation);
            }
            Ok(None)
        }
    }
}

fn drain_gl_errors(gl: &glow::Context) -> Option<u32> {
    first_drained_gl_error(|| unsafe { gl.get_error() })
}

pub(super) fn first_drained_gl_error(mut next_error: impl FnMut() -> u32) -> Option<u32> {
    let mut first_error = None;
    loop {
        let error = next_error();
        if error == glow::NO_ERROR {
            return first_error;
        }
        first_error.get_or_insert(error);
    }
}

fn check_dmabuf_gl_stage(
    gl: &glow::Context,
    stage: DmabufImportGlStage,
) -> Result<(), DmabufTextureImportError> {
    drain_gl_errors(gl).map_or(Ok(()), |error| {
        Err(DmabufTextureImportError::Gl { stage, error })
    })
}

pub(super) struct DmabufImporter {
    image_target_texture_2d: Option<GlEglImageTargetTexture2DOes>,
}

impl DmabufImporter {
    pub(super) const fn new(image_target_texture_2d: Option<GlEglImageTargetTexture2DOes>) -> Self {
        Self {
            image_target_texture_2d,
        }
    }

    pub(super) fn create_resource(
        &self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        handle: &oblivion_one::render_backend::buffer::DmabufBufferHandle,
        generation: u64,
    ) -> RendererResult<EglImageResource> {
        let preexisting_gl_error = drain_gl_errors(gl);
        if native_egl_debug_enabled()
            && let Some(error) = preexisting_gl_error
        {
            eprintln!(
                "oblivion-one compositor: dmabuf import cleared preexisting GL error 0x{error:04x}"
            );
        }
        let Some(image_target_texture_2d) = self.image_target_texture_2d else {
            return Err(io::Error::other("GL_OES_EGL_image is unavailable").into());
        };
        let attributes = EglGlesDmabufImportAttributes::from_handle(handle)
            .map_err(DmabufTextureImportError::InvalidAttributes)?;
        let no_context = unsafe { egl::Context::from_ptr(egl::NO_CONTEXT) };
        let null_client_buffer = unsafe { egl::ClientBuffer::from_ptr(ptr::null_mut()) };
        let image = egl
            .create_image(
                egl_display,
                no_context,
                EGL_LINUX_DMA_BUF_EXT,
                null_client_buffer,
                attributes.as_slice(),
            )
            .map_err(DmabufTextureImportError::EglImageCreation)?;
        let image_guard = EglImageGuard::new(image, |image| {
            let _ = egl.destroy_image(egl_display, image);
        });
        let texture = unsafe {
            gl.create_texture()
                .map_err(|error| DmabufTextureImportError::TextureCreation(error.to_owned()))?
        };
        if let Err(error) = check_dmabuf_gl_stage(gl, DmabufImportGlStage::TextureCreation) {
            unsafe { gl.delete_texture(texture) };
            return Err(error.into());
        }
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        }
        if let Err(error) = check_dmabuf_gl_stage(gl, DmabufImportGlStage::Bind) {
            unsafe { gl.delete_texture(texture) };
            return Err(error.into());
        }
        configure_texture(gl);
        if let Err(error) = check_dmabuf_gl_stage(gl, DmabufImportGlStage::TextureConfiguration) {
            unsafe { gl.delete_texture(texture) };
            return Err(error.into());
        }
        unsafe {
            image_target_texture_2d(glow::TEXTURE_2D, image_guard.image().as_ptr());
        }
        if let Err(error) = check_dmabuf_gl_stage(gl, DmabufImportGlStage::ImageTarget) {
            unsafe { gl.delete_texture(texture) };
            return Err(error.into());
        }

        let size = handle.size();
        Ok(EglImageResource {
            texture,
            size: (size.width, size.height),
            generation,
            egl_image: Some(image_guard.disarm()),
        })
    }
}
