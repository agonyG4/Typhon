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
    time::{Duration, Instant},
};

use crate::egl_renderer::dmabuf::{query_egl_dmabuf_feedback, query_egl_main_device};
use crate::egl_renderer::{
    EglFrameOutcome, EglSceneDrawRequest, GlEglImageTargetTexture2DOes, GlesSceneFrameStats,
    GlesSceneRenderer, OutputDamage, OutputRect as RendererOutputRect, choose_native_egl_config,
    create_gles_context, detect_partial_repaint_capabilities, egl_swap_buffers_with_damage,
    load_egl_image_target_texture_2d, load_swap_buffers_with_damage, native_visual_label,
};
use crate::egl_renderer::{EglInstance, EglSwapBuffersWithDamage};
use gbm::AsRaw as GbmAsRaw;
use khronos_egl as egl;
#[cfg(test)]
use oblivion_one::compositor::OutputRect;
use oblivion_one::compositor::{
    AcquireWatchChange, AstreaShortcutPhase, DesktopComposeRequest, DesktopFrameCopyKind,
    DesktopSceneRebuildKind, DesktopSceneRenderer, DesktopVisualState, FramePresentation,
    FullscreenPresentationRejection, OutputPosition as CompositorOutputPosition, OutputRegion,
    OwnCompositorServer, PointerConstraintBackendId, PointerConstraintBackendRequest,
    PointerConstraintMode, PointerMotionSample as CompositorPointerMotionSample, PresentationClock,
    RelativePointerMotion as CompositorRelativePointerMotion, RenderGenerationCause,
    RenderSceneElement, RenderSceneElementId, RenderableSurface, cursor_texture_pixels,
    cursor_texture_size, render_scene_elements_for_surfaces, resize_debug_log,
};
#[cfg(test)]
use oblivion_one::native::kms::KmsBackendKind;
use oblivion_one::native::kms::{
    AtomicCommitState, AtomicCompletion, AtomicKmsErrorKind, ConnectorId, CrtcId, FramebufferId,
    KmsBackendSelection, KmsPolicy, PageFlipToken,
};
use oblivion_one::native::{
    adaptive_buffering::{
        AdaptiveBufferingController, AdaptiveBufferingMode, AdaptiveRenderJournal,
        AdaptiveTripleBufferPolicy, FenceTimestampQuality, ProvenDeadlineMiss,
        approximate_observation_is_late, render_sample_duration_ns,
    },
    drm::{
        DrmPresentationEvent, DrmTimestampClock, drain_drm_page_flip_events,
        query_drm_timestamp_clock, sample_clock_microseconds,
    },
    event_loop::{
        NativeEventLoop, NativeEventSource, NativeWakeup, ReactorToken, monotonic_now_ns,
    },
    explicit_sync::{
        AcquireReadyResult, AcquireRegistrationResult, DrmAcquirePointNotifier,
        ExplicitSyncWatchRegistry,
    },
    presentation_deadline::{
        MonotonicTimestampNs, PresentationDeadlinePlanner, PresentationTarget,
        PresentationTargetReason,
    },
    scheduler::{
        NativeFrameScheduler, NativeOutputPacingMode, PageFlipCompletionResult,
        PresentationCadenceMetrics, SchedulerCapabilities, SchedulerDecision,
        SchedulerFrameContext,
    },
};
use oblivion_one::process::{ChildSupervisor, ProcessKind, ProcessOptions, RestartPolicy};
use oblivion_one::render_backend::egl_gles::EglGlesDmabufFeedback;
use oblivion_one::session::NativeSessionProbe;
use oblivion_one::syncobj::DrmSyncobjDevice;
use oblivion_one::{
    CompositorAppGpuPreference, EffectiveCompositorAppGpuPolicy,
    compositor_app_command_with_policy, shell_quote,
};
use wayland_server::Resource;

#[cfg(test)]
use std::sync::Mutex;

type NativeResult<T> = Result<T, Box<dyn Error>>;

#[cfg(test)]
pub(crate) static ASTREA_ENV_LOCK: Mutex<()> = Mutex::new(());

mod input;
mod launch;
mod output;
mod pacing;
mod perf;
mod runtime;
mod scanout;

pub(crate) use input::*;
pub(crate) use launch::*;
pub(crate) use output::*;
use pacing::*;
pub(crate) use perf::*;
pub(crate) use runtime::{
    NativeCursorRenderMode, NativeFrameRenderer, NativePointerConstraintBackend,
    native_pointer_debug_log, run,
};
pub(crate) use scanout::*;

#[cfg(test)]
mod tests;
