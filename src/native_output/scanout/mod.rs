use super::*;
use oblivion_one::native::kms::KmsBackendKind;

#[allow(dead_code)] // Runtime attachment is completed after frame-owned rendering is available.
mod atomic_egl_gbm;
mod backend;
mod direct;
mod direct_policy;
mod dumb;
mod egl_gbm;
#[allow(dead_code)] // Negotiation is wired into explicit slot allocation in Task 4.
mod format_negotiation;
mod gbm_cpu;
#[allow(dead_code)] // Concrete slots are attached to runtime ownership after initial modeset.
mod output_slot;
#[allow(dead_code)] // Ownership primitives are wired into the explicit backend in Tasks 4 and 8.
mod output_swapchain;

#[allow(unused_imports)]
pub(crate) use atomic_egl_gbm::*;
pub(crate) use backend::*;
pub(crate) use direct::*;
pub(crate) use direct_policy::*;
pub(crate) use dumb::*;
pub(crate) use egl_gbm::*;
#[allow(unused_imports)]
pub(crate) use format_negotiation::*;
pub(crate) use gbm_cpu::*;
#[allow(unused_imports)]
pub(crate) use output_slot::*;
#[allow(unused_imports)]
pub(crate) use output_swapchain::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeScanoutPreference {
    Auto,
    AtomicEglGbmExplicit,
    NativeEglGbmOpaqueCompatibility,
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
            "native-egl-gbm-opaque" => Self::NativeEglGbmOpaqueCompatibility,
            "gpu" | "native" | "native-gpu" | "native-egl-gbm" | "egl-gbm" | "gles-gbm"
            | "egl-gles-gbm" => Self::AtomicEglGbmExplicit,
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
                | "native-egl-gbm-opaque"
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
            NativeScanoutPreference::AtomicEglGbmExplicit
                if choice.gbm_available && choice.egl_available && choice.page_flip_available =>
            {
                Self {
                    primary: NativeScanoutKind::AtomicEglGbmExplicit,
                    fallbacks: Vec::new(),
                }
            }
            NativeScanoutPreference::AtomicEglGbmExplicit => Self::unavailable(),
            NativeScanoutPreference::NativeEglGbmOpaqueCompatibility
                if choice.gbm_available && choice.egl_available && choice.page_flip_available =>
            {
                Self {
                    primary: NativeScanoutKind::NativeEglGbmOpaqueCompatibility,
                    fallbacks: Vec::new(),
                }
            }
            NativeScanoutPreference::NativeEglGbmOpaqueCompatibility => Self::unavailable(),
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
                    primary: NativeScanoutKind::AtomicEglGbmExplicit,
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
    AtomicEglGbm(Box<AtomicEglGbmScanout>),
    NativeEglGbm(Box<NativeEglGbmScanout>),
    Gbm(NativeGbmScanout),
    Dumb(DumbFramebuffer),
}

pub(crate) fn direct_cursor_plan_key(
    cursor: Option<&AtomicCursorVisualState>,
    compatible: bool,
) -> Option<u64> {
    compatible.then_some(match cursor {
        None => 0,
        Some(cursor) => {
            let framebuffer = u64::from(cursor.framebuffer_id.unwrap_or_default());
            cursor
                .image_generation
                .rotate_left(17)
                .wrapping_add(framebuffer.rotate_left(31))
                .wrapping_add(u64::from(cursor.visible))
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeScanoutRecovery {
    AtomicEglGbm(AtomicExplicitRecovery),
    NativeEglGbm(NativePageFlipRecovery),
    Gbm(NativeIndexedScanoutRecovery),
    Dumb(FramebufferId),
}

impl NativeScanoutRecovery {
    pub(crate) fn framebuffer_id(self) -> FramebufferId {
        match self {
            Self::AtomicEglGbm(recovery) => recovery.framebuffer,
            Self::NativeEglGbm(recovery) => {
                // A recovery token cannot be created with a zero framebuffer ID.
                FramebufferId::new(recovery.framebuffer_id).expect("validated recovery framebuffer")
            }
            Self::Gbm(recovery) => {
                FramebufferId::new(recovery.framebuffer_id).expect("validated recovery framebuffer")
            }
            Self::Dumb(framebuffer) => framebuffer,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AtomicExplicitRecovery {
    framebuffer: FramebufferId,
    current: OutputSlotId,
    pool_generation: u64,
}

impl NativeScanoutBackend {
    pub(crate) fn explicit_output_counters(&self) -> Option<ExplicitOutputCounters> {
        match self {
            Self::AtomicEglGbm(scanout) => Some(scanout.counters()),
            _ => None,
        }
    }
    pub(crate) fn from_atomic_explicit(scanout: AtomicEglGbmScanout) -> Self {
        Self::AtomicEglGbm(Box::new(scanout))
    }

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
            NativeScanoutKind::AtomicEglGbmExplicit => Err(io::Error::other(
                "explicit Atomic EGL/GBM requires pre-discovered Atomic plane capabilities",
            )),
            NativeScanoutKind::NativeEglGbmOpaqueCompatibility => {
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
            Self::AtomicEglGbm(_) => NativeScanoutKind::AtomicEglGbmExplicit,
            Self::NativeEglGbm(_) => NativeScanoutKind::NativeEglGbmOpaqueCompatibility,
            Self::Gbm(_) => NativeScanoutKind::GbmCpuWritePageFlip,
            Self::Dumb(_) => NativeScanoutKind::DumbFramebuffer,
        }
    }

    pub(crate) const fn supports_gpu_buffer_protocols(&self) -> bool {
        matches!(self, Self::AtomicEglGbm(_) | Self::NativeEglGbm(_))
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
            Self::AtomicEglGbm(_) => Err(io::Error::other(
                "explicit Atomic output requires a frame-owned render transaction",
            )),
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
            Self::AtomicEglGbm(scanout) => scanout
                .swapchain()
                .and_then(|swapchain| scanout.framebuffer(swapchain.current()))
                .map(FramebufferId::get)
                .unwrap_or(0),
            Self::NativeEglGbm(scanout) => scanout.fb_id(),
            Self::Gbm(scanout) => scanout.fb_id(),
            Self::Dumb(framebuffer) => framebuffer.fb_id,
        }
    }

    pub(crate) fn prepare_session_recovery(&self) -> io::Result<NativeScanoutRecovery> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout
                .prepare_session_recovery()
                .map(NativeScanoutRecovery::AtomicEglGbm),
            Self::NativeEglGbm(scanout) => scanout
                .prepare_session_recovery()
                .map(NativeScanoutRecovery::NativeEglGbm),
            Self::Gbm(scanout) => scanout
                .prepare_session_recovery()
                .map(NativeScanoutRecovery::Gbm),
            Self::Dumb(framebuffer) => FramebufferId::new(framebuffer.fb_id)
                .map(NativeScanoutRecovery::Dumb)
                .ok_or_else(|| io::Error::other("dumb recovery framebuffer ID is zero")),
        }
    }

    pub(crate) fn complete_session_recovery(
        &mut self,
        recovery: NativeScanoutRecovery,
        server: &mut OwnCompositorServer,
    ) -> io::Result<()> {
        match (self, recovery) {
            (Self::AtomicEglGbm(scanout), NativeScanoutRecovery::AtomicEglGbm(recovery)) => {
                scanout.complete_session_recovery(recovery, server)
            }
            (Self::NativeEglGbm(scanout), NativeScanoutRecovery::NativeEglGbm(recovery)) => {
                scanout.complete_session_recovery(recovery)
            }
            (Self::Gbm(scanout), NativeScanoutRecovery::Gbm(recovery)) => {
                scanout.complete_session_recovery(recovery)
            }
            (Self::Dumb(framebuffer), NativeScanoutRecovery::Dumb(recovery))
                if framebuffer.fb_id == recovery.get() =>
            {
                Ok(())
            }
            _ => Err(io::Error::other(
                "scanout recovery token does not match the active backend",
            )),
        }
    }

    pub(crate) fn scanout_format(&self) -> u32 {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.format_modifier.fourcc,
            Self::NativeEglGbm(scanout) => scanout.format as u32,
            Self::Gbm(_) | Self::Dumb(_) => u32::from_le_bytes(*b"XR24"),
        }
    }

    pub(crate) fn finish_initial_scanout(&mut self) {
        match self {
            Self::AtomicEglGbm(_) => {}
            Self::NativeEglGbm(scanout) => scanout.finish_initial_scanout(),
            Self::Gbm(scanout) => scanout.finish_initial_scanout(),
            Self::Dumb(_) => {}
        }
    }

    pub(crate) fn present(
        &mut self,
        kms: &KmsBackendSelection,
        cursor: Option<&AtomicCursorVisualState>,
    ) -> io::Result<NativePresentResult> {
        let submitted_token = match self {
            Self::AtomicEglGbm(_) => {
                return Err(io::Error::other(
                    "explicit Atomic output requires IN_FENCE_FD submission",
                ));
            }
            Self::NativeEglGbm(scanout) => scanout.present(kms, cursor)?,
            Self::Gbm(scanout) => scanout.present(kms, cursor)?,
            Self::Dumb(_) => return Ok(NativePresentResult::Immediate),
        };
        match submitted_token {
            Some((token, framebuffer_id)) => Ok(NativePresentResult::AsyncSubmitted {
                token,
                framebuffer_id,
            }),
            None => Ok(NativePresentResult::Noop),
        }
    }

    pub(crate) fn drain_page_flip_events(
        &mut self,
        fd: RawFd,
        backend_kind: KmsBackendKind,
    ) -> io::Result<NativePageFlipDrain> {
        match self {
            Self::AtomicEglGbm(_) => {
                let events = drain_drm_page_flip_events(fd)?;
                Ok(NativePageFlipDrain {
                    completion: events.into_iter().next(),
                    ..NativePageFlipDrain::default()
                })
            }
            Self::NativeEglGbm(scanout) => scanout.drain_page_flip_events(fd, backend_kind),
            Self::Gbm(scanout) => scanout.drain_page_flip_events(fd, backend_kind),
            Self::Dumb(_) => Ok(NativePageFlipDrain::default()),
        }
    }

    pub(crate) fn promote_page_flip(&mut self, token: PageFlipToken) -> io::Result<()> {
        match self {
            Self::NativeEglGbm(scanout) => scanout.promote_page_flip(token),
            Self::Gbm(scanout) => scanout.promote_page_flip(token),
            Self::AtomicEglGbm(_) | Self::Dumb(_) => Ok(()),
        }
    }

    pub(crate) fn page_flip_pending(&self) -> bool {
        match self {
            Self::AtomicEglGbm(scanout) => {
                let composited_pending = scanout
                    .swapchain()
                    .is_ok_and(|swapchain| swapchain.pending_slot().is_some());
                let direct_pending = scanout.direct_scanout_pending();
                debug_assert!(!(composited_pending && direct_pending));
                composited_pending || direct_pending
            }
            Self::NativeEglGbm(scanout) => scanout.page_flip_pending(),
            Self::Gbm(scanout) => scanout.page_flip_pending(),
            Self::Dumb(_) => false,
        }
    }

    pub(crate) fn suspend_page_flip(&mut self, server: &mut OwnCompositorServer) -> io::Result<()> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.suspend_for_session(server)?,
            Self::NativeEglGbm(scanout) => scanout.suspend_page_flip(),
            Self::Gbm(scanout) => scanout.suspend_page_flip(),
            Self::Dumb(_) => {}
        }
        Ok(())
    }

    pub(crate) fn rebind_session_generation(&mut self, generation: u64) {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.rebind_session_generation(generation),
            Self::NativeEglGbm(scanout) => scanout.rebind_session_generation(generation),
            Self::Gbm(scanout) => scanout.rebind_session_generation(generation),
            Self::Dumb(_) => {}
        }
    }

    pub(crate) fn disarm_drm_cleanup(&mut self) {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.disarm_drm_cleanup(),
            Self::NativeEglGbm(scanout) => scanout.disarm_drm_cleanup(),
            Self::Gbm(scanout) => scanout.disarm_drm_cleanup(),
            Self::Dumb(framebuffer) => framebuffer.drm_cleanup_armed = false,
        }
    }

    pub(crate) fn ready_frame_queued(&self) -> bool {
        match self {
            Self::AtomicEglGbm(scanout) => scanout
                .swapchain()
                .is_ok_and(|swapchain| swapchain.ready_slot().is_some()),
            Self::NativeEglGbm(scanout) => scanout.ready_frame_queued(),
            Self::Gbm(scanout) => scanout.ready_frame_queued(),
            Self::Dumb(_) => false,
        }
    }

    pub(crate) fn discard_ready_frame_before_direct(
        &mut self,
        server: &mut OwnCompositorServer,
    ) -> io::Result<bool> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.discard_ready_frame_before_direct(server),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => Ok(false),
        }
    }

    pub(crate) fn output_render_in_progress(&self) -> bool {
        match self {
            Self::AtomicEglGbm(scanout) => scanout
                .swapchain()
                .is_ok_and(|swapchain| swapchain.rendering_slot().is_some()),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => false,
        }
    }

    pub(crate) fn third_slot_owned(&self) -> bool {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.swapchain().is_ok_and(|swapchain| {
                swapchain.ready_slot().is_some() || swapchain.rendering_slot().is_some()
            }),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => false,
        }
    }

    pub(crate) fn render_target_available(&self) -> bool {
        self.render_target_available_for(NativeOutputPacingMode::PredictiveTriple)
    }

    pub(crate) fn render_target_available_for(&self, pacing_mode: NativeOutputPacingMode) -> bool {
        match self {
            Self::AtomicEglGbm(scanout) => scanout
                .swapchain()
                .is_ok_and(|swapchain| swapchain.render_target_available_for(pacing_mode)),
            Self::NativeEglGbm(scanout) => scanout.render_target_available(),
            Self::Gbm(scanout) => scanout.render_target_available(),
            Self::Dumb(_) => true,
        }
    }

    pub(crate) fn buffer_snapshot(&self) -> NativeScanoutBufferSnapshot {
        match self {
            Self::AtomicEglGbm(scanout) => {
                let swapchain = scanout.swapchain().ok();
                NativeScanoutBufferSnapshot {
                    backend: NativeScanoutKind::AtomicEglGbmExplicit,
                    capacity: Some(EXPLICIT_OUTPUT_SLOT_CAPACITY),
                    current: swapchain.map(|state| usize::from(state.current().get())),
                    pending: swapchain
                        .and_then(AtomicOutputSwapchain::pending_slot)
                        .map(|slot| usize::from(slot.get())),
                    ready: swapchain
                        .and_then(AtomicOutputSwapchain::ready_slot)
                        .map(|slot| usize::from(slot.get())),
                    free_count: swapchain.map(AtomicOutputSwapchain::free_slot_count),
                    gbm_surface_has_free_buffers: None,
                }
            }
            Self::NativeEglGbm(scanout) => NativeScanoutBufferSnapshot {
                backend: NativeScanoutKind::NativeEglGbmOpaqueCompatibility,
                capacity: None,
                current: None,
                pending: None,
                ready: None,
                free_count: None,
                gbm_surface_has_free_buffers: Some(scanout.surface.has_free_buffers()),
            },
            Self::Gbm(scanout) => {
                let occupied = [
                    Some(scanout.current_index),
                    scanout.pending_index,
                    scanout.ready_index,
                ]
                .into_iter()
                .flatten()
                .collect::<std::collections::BTreeSet<_>>()
                .len();
                NativeScanoutBufferSnapshot {
                    backend: NativeScanoutKind::GbmCpuWritePageFlip,
                    capacity: Some(scanout.buffers.len()),
                    current: Some(scanout.current_index),
                    pending: scanout.pending_index,
                    ready: scanout.ready_index,
                    free_count: Some(scanout.buffers.len().saturating_sub(occupied)),
                    gbm_surface_has_free_buffers: None,
                }
            }
            Self::Dumb(_) => NativeScanoutBufferSnapshot {
                backend: NativeScanoutKind::DumbFramebuffer,
                capacity: Some(1),
                current: Some(0),
                pending: None,
                ready: None,
                free_count: Some(0),
                gbm_surface_has_free_buffers: None,
            },
        }
    }

    pub(crate) fn pending_page_flip_token(&self) -> Option<u64> {
        match self {
            Self::AtomicEglGbm(scanout) => {
                let composited = scanout
                    .swapchain()
                    .ok()
                    .and_then(AtomicOutputSwapchain::pending_token);
                let direct = scanout.direct_scanout_pending_token();
                debug_assert!(!(composited.is_some() && direct.is_some()));
                composited.or(direct).map(PageFlipToken::get)
            }
            Self::NativeEglGbm(scanout) => {
                scanout.page_flip.pending_token().map(PageFlipToken::get)
            }
            Self::Gbm(scanout) => scanout.page_flip.pending_token().map(PageFlipToken::get),
            Self::Dumb(_) => None,
        }
    }

    pub(crate) fn direct_scanout_active(&self) -> bool {
        matches!(self, Self::AtomicEglGbm(scanout) if scanout.direct_scanout_active())
    }

    pub(crate) fn direct_scanout_pending(&self) -> bool {
        matches!(self, Self::AtomicEglGbm(scanout) if scanout.direct_scanout_pending())
    }

    pub(crate) fn direct_scanout_surface(&self) -> Option<u32> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.direct_scanout_surface(),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => None,
        }
    }

    pub(crate) fn direct_scanout_info(&self) -> Option<(u64, u32, u32, u64)> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.direct_scanout_info(),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => None,
        }
    }

    pub(crate) fn direct_scanout_inhibited(&self) -> bool {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.direct_scanout_inhibited(),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => true,
        }
    }

    pub(crate) fn direct_scanout_counters(&self) -> Option<DirectScanoutCounters> {
        match self {
            Self::AtomicEglGbm(scanout) => Some(scanout.direct_scanout_counters()),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => None,
        }
    }

    pub(crate) fn note_direct_composited_render_ahead_suppressed(&mut self) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_composited_render_ahead_suppressed();
        }
    }

    pub(crate) fn try_direct_scanout(
        &mut self,
        kms: &KmsBackendSelection,
        server: &mut OwnCompositorServer,
        target: oblivion_one::native::presentation_deadline::PresentationTarget,
        cursor: Option<&AtomicCursorVisualState>,
    ) -> io::Result<DirectScanoutAttempt> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.try_direct_scanout(kms, server, target, cursor),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => Err(io::Error::other(
                "direct scanout is unsupported by this backend",
            )),
        }
    }

    pub(crate) fn complete_direct_pageflip(
        &mut self,
        token: PageFlipToken,
        presentation: FramePresentation,
        server: &mut OwnCompositorServer,
    ) -> io::Result<PresentedDirectFrame> {
        match self {
            Self::AtomicEglGbm(scanout) => {
                scanout.complete_direct_pageflip(token, presentation, server)
            }
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => Err(io::Error::other(
                "direct pageflip is unsupported by this backend",
            )),
        }
    }

    pub(crate) fn dmabuf_feedback(&self) -> EglGlesDmabufFeedback {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.dmabuf_feedback(),
            Self::NativeEglGbm(scanout) => scanout.dmabuf_feedback.clone(),
            Self::Gbm(_) | Self::Dumb(_) => EglGlesDmabufFeedback::new(Vec::new()),
        }
    }

    pub(crate) fn dmabuf_main_device(&self) -> Option<u64> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.dmabuf_main_device(),
            Self::NativeEglGbm(scanout) => scanout.dmabuf_main_device,
            Self::Gbm(_) | Self::Dumb(_) => None,
        }
    }

    pub(crate) fn dmabuf_main_device_path(&self) -> Option<String> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.dmabuf_main_device_path(),
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
