use super::*;

mod backend;
mod dumb;
mod egl_gbm;
mod gbm_cpu;

pub(crate) use backend::*;
pub(crate) use dumb::*;
pub(crate) use egl_gbm::*;
pub(crate) use gbm_cpu::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeScanoutPreference {
    Auto,
    NativeEglGbm,
    GbmCpuWritePageFlip,
    DumbFramebuffer,
}

impl NativeScanoutPreference {
    pub(crate) fn from_env() -> Self {
        match std::env::var("OBLIVION_ONE_SCANOUT_BACKEND") {
            Ok(value) if Self::is_known_value(&value) => Self::parse(&value),
            Ok(value) => {
                eprintln!(
                    "native scanout: unknown OBLIVION_ONE_SCANOUT_BACKEND={value:?}; using auto"
                );
                Self::Auto
            }
            Err(_) => Self::Auto,
        }
    }

    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "auto" => Self::Auto,
            "gpu" | "native" | "native-gpu" | "native-egl-gbm" | "egl-gbm" | "gles-gbm"
            | "egl-gles-gbm" => Self::NativeEglGbm,
            "gbm-cpu-write"
            | "gbm-cpu-write-pageflip"
            | "cpu-gbm-write"
            | "cpu-gbm-pageflip"
            | "cpu"
            | "cpu-gbm"
            | "gbm"
            | "egl"
            | "pageflip"
            | "gbm-egl"
            | "gbm-egl-pageflip" => Self::GbmCpuWritePageFlip,
            "dumb" | "framebuffer" | "legacy" => Self::DumbFramebuffer,
            _ => Self::Auto,
        }
    }

    pub(crate) fn is_known_value(value: &str) -> bool {
        matches!(
            value,
            "auto"
                | "gpu"
                | "native"
                | "native-gpu"
                | "native-egl-gbm"
                | "egl-gbm"
                | "gles-gbm"
                | "egl-gles-gbm"
                | "gbm-cpu-write"
                | "gbm-cpu-write-pageflip"
                | "cpu-gbm-write"
                | "cpu-gbm-pageflip"
                | "cpu"
                | "cpu-gbm"
                | "gbm"
                | "egl"
                | "pageflip"
                | "gbm-egl"
                | "gbm-egl-pageflip"
                | "dumb"
                | "framebuffer"
                | "legacy"
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeScanoutChoice {
    pub(crate) preference: NativeScanoutPreference,
    pub(crate) gbm_available: bool,
    pub(crate) egl_available: bool,
    pub(crate) page_flip_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeScanoutPlan {
    pub(crate) primary: NativeScanoutKind,
    pub(crate) fallbacks: Vec<NativeScanoutKind>,
}

impl NativeScanoutPlan {
    pub(crate) fn choose(choice: NativeScanoutChoice) -> Self {
        match choice.preference {
            NativeScanoutPreference::NativeEglGbm
                if choice.gbm_available && choice.egl_available && choice.page_flip_available =>
            {
                Self {
                    primary: NativeScanoutKind::NativeEglGbm,
                    fallbacks: Vec::new(),
                }
            }
            NativeScanoutPreference::NativeEglGbm => Self::unavailable(),
            NativeScanoutPreference::GbmCpuWritePageFlip
                if choice.gbm_available && choice.page_flip_available =>
            {
                Self {
                    primary: NativeScanoutKind::GbmCpuWritePageFlip,
                    fallbacks: Vec::new(),
                }
            }
            NativeScanoutPreference::GbmCpuWritePageFlip => Self::unavailable(),
            NativeScanoutPreference::DumbFramebuffer => Self {
                primary: NativeScanoutKind::DumbFramebuffer,
                fallbacks: Vec::new(),
            },
            NativeScanoutPreference::Auto
                if choice.gbm_available && choice.egl_available && choice.page_flip_available =>
            {
                Self {
                    primary: NativeScanoutKind::NativeEglGbm,
                    fallbacks: vec![
                        NativeScanoutKind::GbmCpuWritePageFlip,
                        NativeScanoutKind::DumbFramebuffer,
                    ],
                }
            }
            NativeScanoutPreference::Auto if choice.gbm_available && choice.page_flip_available => {
                Self {
                    primary: NativeScanoutKind::GbmCpuWritePageFlip,
                    fallbacks: vec![NativeScanoutKind::DumbFramebuffer],
                }
            }
            NativeScanoutPreference::Auto => Self {
                primary: NativeScanoutKind::DumbFramebuffer,
                fallbacks: Vec::new(),
            },
        }
    }

    pub(crate) fn unavailable() -> Self {
        Self {
            primary: NativeScanoutKind::Unavailable,
            fallbacks: Vec::new(),
        }
    }

    pub(crate) fn candidates(&self) -> impl Iterator<Item = NativeScanoutKind> + '_ {
        std::iter::once(self.primary).chain(self.fallbacks.iter().copied())
    }

    pub(crate) fn after_failed(&self, failed: NativeScanoutKind) -> Self {
        let mut remaining = self
            .candidates()
            .skip_while(|candidate| *candidate != failed)
            .skip(1)
            .collect::<Vec<_>>();
        if remaining.is_empty() {
            return Self::unavailable();
        }
        let primary = remaining.remove(0);
        Self {
            primary,
            fallbacks: remaining,
        }
    }
}

pub(crate) enum NativeScanoutBackend {
    NativeEglGbm(Box<NativeEglGbmScanout>),
    Gbm(NativeGbmScanout),
    Dumb(DumbFramebuffer),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativePresentResult {
    Noop,
    AsyncSubmitted { token: u64 },
    Immediate,
}

#[derive(Debug, Default)]
pub(crate) struct NativePageFlipDrain {
    pub(crate) completion: Option<DrmPresentationEvent>,
    pub(crate) mismatched_events: u64,
    pub(crate) stale_events: u64,
    pub(crate) last_mismatch: Option<(u64, u64)>,
    pub(crate) last_stale_token: Option<u64>,
}

impl NativeScanoutBackend {
    pub(crate) fn open(
        plan: NativeScanoutPlan,
        kms: &fs::File,
        width: u32,
        height: u32,
        backend_generation: u64,
    ) -> io::Result<Self> {
        let mut last_error = None;
        for candidate in plan.candidates() {
            match Self::open_kind(candidate, kms, width, height, backend_generation) {
                Ok(backend) => return Ok(backend),
                Err(error) => {
                    eprintln!(
                        "native scanout: {} backend failed: {error}",
                        candidate.as_str()
                    );
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| io::Error::other("native scanout backend unavailable")))
    }

    pub(crate) fn open_kind(
        kind: NativeScanoutKind,
        kms: &fs::File,
        width: u32,
        height: u32,
        backend_generation: u64,
    ) -> io::Result<Self> {
        match kind {
            NativeScanoutKind::NativeEglGbm => {
                if native_test_fail_native_egl_gbm_enabled() {
                    return Err(io::Error::other(
                        "native EGL/GBM failure injected by OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM",
                    ));
                }
                Ok(Self::NativeEglGbm(Box::new(NativeEglGbmScanout::create(
                    kms,
                    width,
                    height,
                    backend_generation,
                )?)))
            }
            NativeScanoutKind::GbmCpuWritePageFlip => Ok(Self::Gbm(NativeGbmScanout::create(
                kms,
                width,
                height,
                backend_generation,
            )?)),
            NativeScanoutKind::DumbFramebuffer => {
                Ok(Self::Dumb(DumbFramebuffer::create(kms, width, height)?))
            }
            NativeScanoutKind::Unavailable => {
                Err(io::Error::other("native scanout backend unavailable"))
            }
        }
    }

    pub(crate) const fn kind(&self) -> NativeScanoutKind {
        match self {
            Self::NativeEglGbm(_) => NativeScanoutKind::NativeEglGbm,
            Self::Gbm(_) => NativeScanoutKind::GbmCpuWritePageFlip,
            Self::Dumb(_) => NativeScanoutKind::DumbFramebuffer,
        }
    }

    pub(crate) const fn supports_gpu_buffer_protocols(&self) -> bool {
        matches!(self, Self::NativeEglGbm(_))
    }

    pub(crate) fn paint_server_frame(
        &mut self,
        renderer: &mut NativeFrameRenderer,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        damage: &NativeOutputDamage,
    ) -> io::Result<NativePaintOutcome> {
        match self {
            Self::NativeEglGbm(scanout) => {
                scanout.paint_server_frame(renderer, server, input_state, cursor_mode, damage)
            }
            Self::Gbm(scanout) => scanout
                .paint_server_frame(renderer, server, input_state, cursor_mode, damage)
                .map(NativePaintOutcome::Rendered),
            Self::Dumb(framebuffer) => framebuffer
                .paint_server_frame(renderer, server, input_state, cursor_mode, damage)
                .map(NativePaintOutcome::Rendered),
        }
    }

    pub(crate) fn fb_id(&self) -> u32 {
        match self {
            Self::NativeEglGbm(scanout) => scanout.fb_id(),
            Self::Gbm(scanout) => scanout.fb_id(),
            Self::Dumb(framebuffer) => framebuffer.fb_id,
        }
    }

    pub(crate) fn scanout_format(&self) -> u32 {
        match self {
            Self::NativeEglGbm(scanout) => scanout.format as u32,
            Self::Gbm(_) | Self::Dumb(_) => u32::from_le_bytes(*b"XR24"),
        }
    }

    pub(crate) fn finish_initial_scanout(&mut self) {
        match self {
            Self::NativeEglGbm(scanout) => scanout.finish_initial_scanout(),
            Self::Gbm(scanout) => scanout.finish_initial_scanout(),
            Self::Dumb(_) => {}
        }
    }

    pub(crate) fn present(&mut self, kms: &KmsBackendSelection) -> io::Result<NativePresentResult> {
        let submitted_token = match self {
            Self::NativeEglGbm(scanout) => scanout.present(kms)?,
            Self::Gbm(scanout) => scanout.present(kms)?,
            Self::Dumb(_) => return Ok(NativePresentResult::Immediate),
        };
        match submitted_token {
            Some(token) => Ok(NativePresentResult::AsyncSubmitted { token }),
            None => Ok(NativePresentResult::Noop),
        }
    }

    pub(crate) fn drain_page_flip_events(&mut self, fd: RawFd) -> io::Result<NativePageFlipDrain> {
        match self {
            Self::NativeEglGbm(scanout) => scanout.drain_page_flip_events(fd),
            Self::Gbm(scanout) => scanout.drain_page_flip_events(fd),
            Self::Dumb(_) => Ok(NativePageFlipDrain::default()),
        }
    }

    pub(crate) fn page_flip_pending(&self) -> bool {
        match self {
            Self::NativeEglGbm(scanout) => scanout.page_flip_pending(),
            Self::Gbm(scanout) => scanout.page_flip_pending(),
            Self::Dumb(_) => false,
        }
    }

    pub(crate) fn pending_page_flip_token(&self) -> Option<u64> {
        match self {
            Self::NativeEglGbm(scanout) => {
                scanout.page_flip.pending_token().map(PageFlipToken::get)
            }
            Self::Gbm(scanout) => scanout.page_flip.pending_token().map(PageFlipToken::get),
            Self::Dumb(_) => None,
        }
    }

    pub(crate) fn dmabuf_feedback(&self) -> EglGlesDmabufFeedback {
        match self {
            Self::NativeEglGbm(scanout) => scanout.dmabuf_feedback.clone(),
            Self::Gbm(_) | Self::Dumb(_) => EglGlesDmabufFeedback::new(Vec::new()),
        }
    }

    pub(crate) fn dmabuf_main_device(&self) -> Option<u64> {
        match self {
            Self::NativeEglGbm(scanout) => scanout.dmabuf_main_device,
            Self::Gbm(_) | Self::Dumb(_) => None,
        }
    }

    pub(crate) fn dmabuf_main_device_path(&self) -> Option<String> {
        match self {
            Self::NativeEglGbm(scanout) => scanout.dmabuf_main_device_path.clone(),
            Self::Gbm(_) | Self::Dumb(_) => None,
        }
    }
}

pub(crate) fn apply_native_scanout_feedback(
    server: &mut OwnCompositorServer,
    scanout: &NativeScanoutBackend,
) {
    server.set_dmabuf_feedback(
        scanout.dmabuf_feedback(),
        scanout.dmabuf_main_device(),
        scanout.dmabuf_main_device_path(),
    );
}

pub(crate) static NEXT_NATIVE_PAGE_FLIP_TOKEN: AtomicU64 = AtomicU64::new(1);
pub(crate) static NEXT_NATIVE_DRM_FILE_GENERATION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn allocate_native_page_flip_token() -> u64 {
    loop {
        let current = NEXT_NATIVE_PAGE_FLIP_TOKEN.load(Ordering::Relaxed);
        let token = current.max(1);
        let next = next_nonzero_page_flip_token(token);
        if NEXT_NATIVE_PAGE_FLIP_TOKEN
            .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return token;
        }
    }
}

pub(crate) fn allocate_native_drm_file_generation() -> u64 {
    loop {
        let current = NEXT_NATIVE_DRM_FILE_GENERATION.load(Ordering::Relaxed);
        let generation = current.max(1);
        let next = generation.checked_add(1).unwrap_or(1);
        if NEXT_NATIVE_DRM_FILE_GENERATION
            .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return generation;
        }
    }
}

pub(crate) const fn next_nonzero_page_flip_token(token: u64) -> u64 {
    let next = token.wrapping_add(1);
    if next == 0 { 1 } else { next }
}
