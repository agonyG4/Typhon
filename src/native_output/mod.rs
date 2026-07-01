use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    error::Error,
    ffi::c_void,
    fs::{self, OpenOptions},
    io, mem,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        unix::fs::OpenOptionsExt,
    },
    path::{Path, PathBuf},
    ptr,
    rc::Rc,
    slice,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use crate::egl_renderer::dmabuf::{query_egl_dmabuf_feedback, query_egl_main_device};
use crate::egl_renderer::{
    EglFrameOutcome, EglSceneDrawRequest, GlEglImageTargetTexture2DOes, GlesSceneFrameStats,
    GlesSceneRenderer, OutputDamage, OutputRect as RendererOutputRect, choose_egl_config,
    choose_native_egl_config, create_gles_context, detect_partial_repaint_capabilities,
    egl_swap_buffers_with_damage, load_egl_image_target_texture_2d, load_swap_buffers_with_damage,
    native_visual_label,
};
use crate::egl_renderer::{EglInstance, EglSwapBuffersWithDamage};
use gbm::AsRaw as GbmAsRaw;
use khronos_egl as egl;
#[cfg(test)]
use oblivion_one::compositor::OutputRect;
use oblivion_one::compositor::{
    AcquireWatchChange, DesktopComposeRequest, DesktopFrameCopyKind, DesktopSceneRebuildKind,
    DesktopSceneRenderer, DesktopVisualState, FramePresentation,
    OutputPosition as CompositorOutputPosition, OutputRegion, OwnCompositorServer,
    PointerConstraintBackendId, PointerConstraintBackendRequest, PointerConstraintMode,
    PointerMotionSample as CompositorPointerMotionSample, PresentationClock,
    RelativePointerMotion as CompositorRelativePointerMotion, RenderGenerationCause,
    RenderSceneElement, RenderSceneElementId, RenderableSurface, cursor_texture_pixels,
    cursor_texture_size, render_scene_elements_for_surfaces,
};
use oblivion_one::native::kms::{
    AtomicCommitState, AtomicCompletion, ConnectorId, CrtcId, FramebufferId, KmsBackendSelection,
    KmsPolicy, PageFlipToken,
};
use oblivion_one::native::{
    drm::{
        DrmPresentationEvent, DrmTimestampClock, drain_drm_page_flip_events,
        query_drm_timestamp_clock, sample_clock_microseconds,
    },
    event_loop::{NativeEventLoop, NativeEventSource, NativeWakeup, monotonic_now_ns},
    explicit_sync::{
        AcquireReadyResult, AcquireRegistrationResult, DrmAcquirePointNotifier,
        ExplicitSyncWatchRegistry,
    },
    scheduler::{NativeFrameScheduler, PageFlipCompletionResult, SchedulerDecision},
};
use oblivion_one::render_backend::egl_gles::EglGlesDmabufFeedback;
use oblivion_one::session::NativeSessionProbe;
use oblivion_one::syncobj::DrmSyncobjDevice;
use oblivion_one::{
    CompositorAppGpuPreference, EffectiveCompositorAppGpuPolicy, shell_quote,
    spawn_compositor_app_with_policy,
};

type NativeResult<T> = Result<T, Box<dyn Error>>;

mod input;
mod launch;
mod output;
mod perf;
mod runtime;
mod scanout;

pub(crate) use input::*;
pub(crate) use launch::*;
pub(crate) use output::*;
pub(crate) use perf::*;
pub(crate) use runtime::{
    NativeCursorRenderMode, NativeFrameRenderer, NativePointerConstraintBackend,
    native_pointer_debug_log, run,
};
pub(crate) use scanout::*;

#[cfg(test)]
mod tests;
