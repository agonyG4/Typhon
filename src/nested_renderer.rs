use std::{error::Error, io, num::NonZeroU32, sync::Arc};

use crate::egl_renderer::{EglFrameOutcome, EglGlesFrameRenderer, EglSceneDrawRequest};
use oblivion_one::compositor::{
    DesktopComposeRequest, DesktopSceneRenderer, DesktopVisualState, RenderableSurface,
    ShellOverlayImage,
};
use oblivion_one::render_backend::egl_gles::EglGlesDmabufFeedback;
use softbuffer::{Context, Surface};
use winit::window::Window;

type RendererResult<T> = Result<T, Box<dyn Error>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NestedPaintOutcome {
    Skipped,
    Rendered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputRendererPreference {
    Auto,
    Gpu,
    Cpu,
}

impl OutputRendererPreference {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "gpu" => Some(Self::Gpu),
            "cpu" => Some(Self::Cpu),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Gpu => "gpu",
            Self::Cpu => "cpu",
        }
    }
}

pub struct NestedSceneDrawRequest<'a> {
    pub width: u32,
    pub height: u32,
    pub output_scale: f64,
    pub surfaces: &'a [RenderableSurface],
    pub content_generation: u64,
    pub visual_state: DesktopVisualState,
    pub shell_overlay: Option<&'a ShellOverlayImage>,
    pub client_cursor: Option<oblivion_one::compositor::ClientCursorRenderState<'a>>,
    pub cpu_scene_renderer: &'a mut DesktopSceneRenderer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputRendererBackend {
    Gpu,
    Cpu,
}

impl OutputRendererBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gpu => "gpu",
            Self::Cpu => "cpu",
        }
    }
}

pub fn choose_output_renderer_backend(
    preference: OutputRendererPreference,
    gpu_available: bool,
) -> Option<OutputRendererBackend> {
    match (preference, gpu_available) {
        (OutputRendererPreference::Auto, true) | (OutputRendererPreference::Gpu, true) => {
            Some(OutputRendererBackend::Gpu)
        }
        (OutputRendererPreference::Auto | OutputRendererPreference::Cpu, _) => {
            Some(OutputRendererBackend::Cpu)
        }
        (OutputRendererPreference::Gpu, false) => None,
    }
}

pub struct NestedOutputRenderer {
    backend: OutputRendererBackend,
    inner: OutputRendererInner,
}

impl NestedOutputRenderer {
    pub fn new(
        window: Arc<Window>,
        renderer_preference: OutputRendererPreference,
    ) -> RendererResult<Self> {
        match choose_output_renderer_backend(renderer_preference, true) {
            Some(OutputRendererBackend::Cpu) => Self::new_cpu(window),
            Some(OutputRendererBackend::Gpu) => match Self::new_gpu(Arc::clone(&window)) {
                Ok(inner) => Ok(Self {
                    backend: OutputRendererBackend::Gpu,
                    inner,
                }),
                Err(error)
                    if choose_output_renderer_backend(renderer_preference, false)
                        == Some(OutputRendererBackend::Cpu) =>
                {
                    eprintln!(
                        "oblivion-one compositor: GPU renderer unavailable, falling back to CPU: {error}"
                    );
                    Self::new_cpu(window)
                }
                Err(error) => Err(error),
            },
            None => Err(io::Error::other("renderer selection did not choose a backend").into()),
        }
    }

    pub const fn backend(&self) -> OutputRendererBackend {
        self.backend
    }

    pub fn dmabuf_feedback(&self) -> EglGlesDmabufFeedback {
        match &self.inner {
            OutputRendererInner::EglGles(renderer) => renderer.dmabuf_feedback().clone(),
            OutputRendererInner::Cpu(_) => EglGlesDmabufFeedback::default(),
        }
    }

    pub const fn dmabuf_main_device(&self) -> Option<u64> {
        match &self.inner {
            OutputRendererInner::EglGles(renderer) => renderer.dmabuf_main_device(),
            OutputRendererInner::Cpu(_) => None,
        }
    }

    pub fn dmabuf_main_device_path(&self) -> Option<String> {
        match &self.inner {
            OutputRendererInner::EglGles(renderer) => {
                renderer.dmabuf_main_device_path().map(str::to_string)
            }
            OutputRendererInner::Cpu(_) => None,
        }
    }

    pub fn draw_desktop_scene(
        &mut self,
        request: NestedSceneDrawRequest<'_>,
    ) -> RendererResult<NestedPaintOutcome> {
        let NestedSceneDrawRequest {
            width,
            height,
            output_scale,
            surfaces,
            content_generation,
            visual_state,
            shell_overlay,
            client_cursor,
            cpu_scene_renderer,
        } = request;

        match &mut self.inner {
            OutputRendererInner::EglGles(renderer) => renderer
                .draw_scene(EglSceneDrawRequest {
                    width,
                    height,
                    surfaces,
                    content_generation,
                    visual_state,
                    output_scale,
                    shell_overlay,
                    client_cursor,
                    current_damage: None,
                })
                .map(|outcome| match outcome {
                    EglFrameOutcome::Skipped { .. } => NestedPaintOutcome::Skipped,
                    EglFrameOutcome::Rendered { .. } => NestedPaintOutcome::Rendered,
                }),
            OutputRendererInner::Cpu(renderer) => renderer
                .draw(width, height, |frame| {
                    cpu_scene_renderer.compose_request(DesktopComposeRequest {
                        frame,
                        frame_width: width,
                        frame_height: height,
                        output_scale,
                        surfaces,
                        content_generation,
                        visual_state,
                        shell_overlay,
                        client_cursor,
                    });
                })
                .map(|()| NestedPaintOutcome::Rendered),
        }
    }

    fn new_cpu(window: Arc<Window>) -> RendererResult<Self> {
        let cpu = CpuFrameRenderer::new(window)?;
        Ok(Self {
            backend: OutputRendererBackend::Cpu,
            inner: OutputRendererInner::Cpu(cpu),
        })
    }

    fn new_gpu(window: Arc<Window>) -> RendererResult<OutputRendererInner> {
        EglGlesFrameRenderer::new(window)
            .map(|renderer| OutputRendererInner::EglGles(Box::new(renderer)))
    }
}

enum OutputRendererInner {
    EglGles(Box<EglGlesFrameRenderer>),
    Cpu(CpuFrameRenderer),
}

struct CpuFrameRenderer {
    surface: Surface<Arc<Window>, Arc<Window>>,
    buffer_size: (u32, u32),
}

impl CpuFrameRenderer {
    fn new(window: Arc<Window>) -> RendererResult<Self> {
        let context = Context::new(Arc::clone(&window))?;
        let surface = Surface::new(&context, window)?;
        Ok(Self {
            surface,
            buffer_size: (0, 0),
        })
    }

    fn draw<F>(&mut self, width: u32, height: u32, fill_frame: F) -> RendererResult<()>
    where
        F: FnOnce(&mut [u32]),
    {
        let width = width.max(1);
        let height = height.max(1);
        if self.buffer_size != (width, height) {
            self.surface.resize(
                NonZeroU32::new(width).expect("width is clamped above zero"),
                NonZeroU32::new(height).expect("height is clamped above zero"),
            )?;
            self.buffer_size = (width, height);
        }

        let mut buffer = self.surface.buffer_mut()?;
        fill_frame(&mut buffer);
        buffer.present()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_renderer_prefers_gpu_when_available() {
        assert_eq!(
            choose_output_renderer_backend(OutputRendererPreference::Auto, true),
            Some(OutputRendererBackend::Gpu)
        );
    }

    #[test]
    fn auto_renderer_falls_back_to_cpu_when_gpu_is_unavailable() {
        assert_eq!(
            choose_output_renderer_backend(OutputRendererPreference::Auto, false),
            Some(OutputRendererBackend::Cpu)
        );
    }

    #[test]
    fn explicit_gpu_renderer_does_not_fall_back_in_selection_policy() {
        assert_eq!(
            choose_output_renderer_backend(OutputRendererPreference::Gpu, false),
            None
        );
    }

    #[test]
    fn renderer_preference_parser_exposes_gl_first_modes_only() {
        assert_eq!(
            OutputRendererPreference::parse("auto"),
            Some(OutputRendererPreference::Auto)
        );
        assert_eq!(
            OutputRendererPreference::parse("gpu"),
            Some(OutputRendererPreference::Gpu)
        );
        assert_eq!(
            OutputRendererPreference::parse("cpu"),
            Some(OutputRendererPreference::Cpu)
        );
        assert_eq!(OutputRendererPreference::parse("vulkan"), None);
    }
}
