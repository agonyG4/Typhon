//! Typed presentation preferences and effective output-mode policy.
//!
//! Surface hints are intentionally kept separate from the mode selected for an
//! output.  The latter is a compositor decision and must be frozen into the
//! output transaction that owns the KMS submission.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SurfacePresentationHint {
    #[default]
    Vsync,
    Async,
}

impl SurfacePresentationHint {
    pub const fn is_async(self) -> bool {
        matches!(self, Self::Async)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SurfaceContentType {
    #[default]
    None,
    Photo,
    Video,
    Game,
}

impl SurfaceContentType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Photo => "photo",
            Self::Video => "video",
            Self::Game => "game",
        }
    }

    /// The DRM connector enum used by the common Content Type property.
    pub const fn drm_value(self) -> DrmContentType {
        match self {
            Self::None => DrmContentType::Graphics,
            Self::Photo => DrmContentType::Photo,
            Self::Video => DrmContentType::Cinema,
            Self::Game => DrmContentType::Game,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DrmContentType {
    Graphics,
    Photo,
    Cinema,
    Game,
}

impl DrmContentType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Graphics => "Graphics",
            Self::Photo => "Photo",
            Self::Cinema => "Cinema",
            Self::Game => "Game",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SurfacePresentationMetadata {
    pub hint: SurfacePresentationHint,
    pub content_type: SurfaceContentType,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SurfacePresentationState {
    current: SurfacePresentationMetadata,
    pending: SurfacePresentationMetadata,
    pending_generation: u64,
}

impl SurfacePresentationState {
    pub const fn current(self) -> SurfacePresentationMetadata {
        self.current
    }

    pub const fn pending(self) -> SurfacePresentationMetadata {
        self.pending
    }

    pub const fn set_pending_hint(mut self, hint: SurfacePresentationHint) -> Self {
        self.pending.hint = hint;
        self.pending_generation = self
            .pending_generation
            .checked_add(1)
            .expect("presentation pending generation exhausted");
        self
    }

    pub const fn set_pending_content_type(mut self, content_type: SurfaceContentType) -> Self {
        self.pending.content_type = content_type;
        self.pending_generation = self
            .pending_generation
            .checked_add(1)
            .expect("presentation pending generation exhausted");
        self
    }

    pub const fn destroy_tearing_object(mut self) -> Self {
        self.pending.hint = SurfacePresentationHint::Vsync;
        self.pending_generation = self
            .pending_generation
            .checked_add(1)
            .expect("presentation pending generation exhausted");
        self
    }

    pub const fn destroy_content_type_object(mut self) -> Self {
        self.pending.content_type = SurfaceContentType::None;
        self.pending_generation = self
            .pending_generation
            .checked_add(1)
            .expect("presentation pending generation exhausted");
        self
    }

    pub const fn capture_pending_and_reset(self) -> (Self, CapturedSurfacePresentation) {
        let captured = CapturedSurfacePresentation {
            metadata: self.pending,
            captured_pending_generation: self.pending_generation,
        };
        (
            Self {
                current: self.current,
                pending: self.current,
                pending_generation: self.pending_generation,
            },
            captured,
        )
    }

    pub const fn apply_captured(mut self, captured: CapturedSurfacePresentation) -> Self {
        self.current = captured.metadata;
        // A new protocol request may arrive after capture but before the
        // synchronized surface tree latches. Preserve it; otherwise latching
        // an older commit would erase the newer double-buffered request.
        if self.pending_generation == captured.captured_pending_generation {
            self.pending = captured.metadata;
        }
        self
    }

    pub const fn commit(mut self) -> (Self, SurfacePresentationMetadata) {
        self.current = self.pending;
        (self, self.current)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CapturedSurfacePresentation {
    pub metadata: SurfacePresentationMetadata,
    captured_pending_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TearingPolicy {
    Off,
    Auto,
}

/// Name used by the native output policy. `TearingPolicy` remains the
/// compositor-facing spelling for callers that do not care about the layer.
pub type NativeTearingPreference = TearingPolicy;

impl Default for TearingPolicy {
    fn default() -> Self {
        Self::Off
    }
}

impl TearingPolicy {
    pub fn from_environment(value: Option<&str>) -> Self {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("auto") => Self::Auto,
            _ => Self::Off,
        }
    }

    pub const fn allows_async_request(self) -> bool {
        !matches!(self, Self::Off)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AsyncBlocker {
    PolicyDisabled,
    NotSolitaryFullscreen,
    SurfaceHintMissing,
    BackendCapabilityUnavailable,
    OutputGenerationUnqualified,
    HardwareCursorVisible,
    CursorTransitionPending,
    NonPrimaryPlaneActive,
    ExplicitSyncNotReady,
    CommitTimingNotSafe,
    KmsLaneBusy,
    AsyncTestOnlyRejected,
    AsyncFormatUnsupported,
    AsyncSubmitRejected,
    ModesetRequired,
}

impl AsyncBlocker {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PolicyDisabled => "policy_disabled",
            Self::NotSolitaryFullscreen => "not_solitary_fullscreen",
            Self::SurfaceHintMissing => "surface_hint_missing",
            Self::BackendCapabilityUnavailable => "backend_capability_unavailable",
            Self::OutputGenerationUnqualified => "output_generation_unqualified",
            Self::HardwareCursorVisible => "hardware_cursor_visible",
            Self::CursorTransitionPending => "cursor_transition_pending",
            Self::NonPrimaryPlaneActive => "non_primary_plane_active",
            Self::ExplicitSyncNotReady => "explicit_sync_not_ready",
            Self::CommitTimingNotSafe => "commit_timing_not_safe",
            Self::KmsLaneBusy => "kms_lane_busy",
            Self::AsyncTestOnlyRejected => "async_test_only_rejected",
            Self::AsyncFormatUnsupported => "async_format_unsupported",
            Self::AsyncSubmitRejected => "async_submit_rejected",
            Self::ModesetRequired => "modeset_required",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AsyncEligibility {
    pub solitary_fullscreen: bool,
    pub async_hint: bool,
    pub backend_capable: bool,
    pub output_generation_qualified: bool,
    pub cursor_visible: bool,
    pub cursor_transition_pending: bool,
    pub non_primary_plane_active: bool,
    pub explicit_sync_ready: bool,
    pub commit_timing_safe: bool,
    pub kms_lane_free: bool,
    pub async_test_only_accepted: bool,
    pub async_format_supported: bool,
    pub modeset_required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputPresentationMode {
    Vsync,
    Async,
}

impl Default for OutputPresentationMode {
    fn default() -> Self {
        Self::Vsync
    }
}

impl OutputPresentationMode {
    pub const fn is_async(self) -> bool {
        matches!(self, Self::Async)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectivePresentation {
    pub mode: OutputPresentationMode,
    pub content_type: SurfaceContentType,
    pub blocker: Option<AsyncBlocker>,
}

impl EffectivePresentation {
    pub fn decide(
        policy: TearingPolicy,
        metadata: SurfacePresentationMetadata,
        eligibility: AsyncEligibility,
    ) -> Self {
        let mut result = Self {
            mode: OutputPresentationMode::Vsync,
            content_type: metadata.content_type,
            blocker: None,
        };
        let blocker = if !policy.allows_async_request() {
            Some(AsyncBlocker::PolicyDisabled)
        } else if !eligibility.solitary_fullscreen {
            Some(AsyncBlocker::NotSolitaryFullscreen)
        } else if !metadata.hint.is_async() || !eligibility.async_hint {
            Some(AsyncBlocker::SurfaceHintMissing)
        } else if !eligibility.backend_capable {
            Some(AsyncBlocker::BackendCapabilityUnavailable)
        } else if !eligibility.async_format_supported {
            Some(AsyncBlocker::AsyncFormatUnsupported)
        } else if !eligibility.output_generation_qualified {
            Some(AsyncBlocker::OutputGenerationUnqualified)
        } else if eligibility.cursor_visible {
            Some(AsyncBlocker::HardwareCursorVisible)
        } else if eligibility.cursor_transition_pending {
            Some(AsyncBlocker::CursorTransitionPending)
        } else if eligibility.non_primary_plane_active {
            Some(AsyncBlocker::NonPrimaryPlaneActive)
        } else if !eligibility.explicit_sync_ready {
            Some(AsyncBlocker::ExplicitSyncNotReady)
        } else if !eligibility.commit_timing_safe {
            Some(AsyncBlocker::CommitTimingNotSafe)
        } else if !eligibility.kms_lane_free {
            Some(AsyncBlocker::KmsLaneBusy)
        } else if !eligibility.async_test_only_accepted {
            Some(AsyncBlocker::AsyncTestOnlyRejected)
        } else if eligibility.modeset_required {
            Some(AsyncBlocker::ModesetRequired)
        } else {
            None
        };
        if blocker.is_none() {
            result.mode = OutputPresentationMode::Async;
        }
        result.blocker = blocker;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_state_is_double_buffered() {
        let state = SurfacePresentationState::default()
            .set_pending_hint(SurfacePresentationHint::Async)
            .set_pending_content_type(SurfaceContentType::Game);
        assert_eq!(state.current(), SurfacePresentationMetadata::default());
        let (state, committed) = state.commit();
        assert_eq!(committed.hint, SurfacePresentationHint::Async);
        assert_eq!(state.current().content_type, SurfaceContentType::Game);
    }

    #[test]
    fn latching_an_older_commit_preserves_a_newer_pending_request() {
        let state =
            SurfacePresentationState::default().set_pending_hint(SurfacePresentationHint::Async);
        let (state, captured) = state.capture_pending_and_reset();
        let state = state.set_pending_content_type(SurfaceContentType::Video);
        let state = state.apply_captured(captured);

        assert_eq!(state.current().hint, SurfacePresentationHint::Async);
        assert_eq!(state.pending().content_type, SurfaceContentType::Video);
    }

    #[test]
    fn same_value_vsync_mutation_after_async_capture_is_not_overwritten() {
        let state =
            SurfacePresentationState::default().set_pending_hint(SurfacePresentationHint::Async);
        let (state, captured) = state.capture_pending_and_reset();
        let state = state.set_pending_hint(SurfacePresentationHint::Vsync);
        let state = state.apply_captured(captured);

        assert_eq!(state.current().hint, SurfacePresentationHint::Async);
        let (state, next) = state.commit();
        assert_eq!(next.hint, SurfacePresentationHint::Vsync);
        assert_eq!(state.current().hint, SurfacePresentationHint::Vsync);
    }

    #[test]
    fn same_value_none_mutation_after_game_capture_is_not_overwritten() {
        let state =
            SurfacePresentationState::default().set_pending_content_type(SurfaceContentType::Game);
        let (state, captured) = state.capture_pending_and_reset();
        let state = state.set_pending_content_type(SurfaceContentType::None);
        let state = state.apply_captured(captured);

        assert_eq!(state.current().content_type, SurfaceContentType::Game);
        let (state, next) = state.commit();
        assert_eq!(next.content_type, SurfaceContentType::None);
        assert_eq!(state.current().content_type, SurfaceContentType::None);
    }

    #[test]
    fn reverse_hint_mutation_sequence_keeps_the_newest_request() {
        let state =
            SurfacePresentationState::default().set_pending_hint(SurfacePresentationHint::Async);
        let (state, captured) = state.capture_pending_and_reset();
        let state = state.set_pending_hint(SurfacePresentationHint::Vsync);
        let state = state.apply_captured(captured);
        let state = state.set_pending_hint(SurfacePresentationHint::Async);
        let (_, next) = state.commit();

        assert_eq!(next.hint, SurfacePresentationHint::Async);
    }

    #[test]
    fn reverse_content_mutation_sequence_keeps_the_newest_request() {
        let state =
            SurfacePresentationState::default().set_pending_content_type(SurfaceContentType::Game);
        let (state, captured) = state.capture_pending_and_reset();
        let state = state.set_pending_content_type(SurfaceContentType::None);
        let state = state.apply_captured(captured);
        let state = state.set_pending_content_type(SurfaceContentType::Video);
        let (_, next) = state.commit();

        assert_eq!(next.content_type, SurfaceContentType::Video);
    }

    #[test]
    fn tearing_object_destruction_reverts_only_pending_hint() {
        let state = SurfacePresentationState::default()
            .set_pending_hint(SurfacePresentationHint::Async)
            .commit()
            .0
            .destroy_tearing_object();
        assert_eq!(state.current().hint, SurfacePresentationHint::Async);
        assert_eq!(state.pending().hint, SurfacePresentationHint::Vsync);
    }

    #[test]
    fn game_content_does_not_enable_async_by_itself() {
        let result = EffectivePresentation::decide(
            TearingPolicy::Auto,
            SurfacePresentationMetadata {
                hint: SurfacePresentationHint::Vsync,
                content_type: SurfaceContentType::Game,
            },
            AsyncEligibility {
                solitary_fullscreen: true,
                backend_capable: true,
                output_generation_qualified: true,
                explicit_sync_ready: true,
                commit_timing_safe: true,
                kms_lane_free: true,
                async_test_only_accepted: true,
                ..AsyncEligibility::default()
            },
        );
        assert_eq!(result.mode, OutputPresentationMode::Vsync);
        assert_eq!(result.blocker, Some(AsyncBlocker::SurfaceHintMissing));
    }

    #[test]
    fn async_format_compatibility_is_an_explicit_blocker() {
        let result = EffectivePresentation::decide(
            TearingPolicy::Auto,
            SurfacePresentationMetadata {
                hint: SurfacePresentationHint::Async,
                content_type: SurfaceContentType::None,
            },
            AsyncEligibility {
                solitary_fullscreen: true,
                async_hint: true,
                backend_capable: true,
                output_generation_qualified: true,
                explicit_sync_ready: true,
                commit_timing_safe: true,
                kms_lane_free: true,
                async_test_only_accepted: true,
                async_format_supported: false,
                ..AsyncEligibility::default()
            },
        );
        assert_eq!(result.mode, OutputPresentationMode::Vsync);
        assert_eq!(result.blocker, Some(AsyncBlocker::AsyncFormatUnsupported));
    }

    #[test]
    fn effective_content_type_maps_to_drm_without_affecting_mode() {
        assert_eq!(
            SurfaceContentType::None.drm_value(),
            DrmContentType::Graphics
        );
        assert_eq!(
            SurfaceContentType::Video.drm_value(),
            DrmContentType::Cinema
        );
    }
}
