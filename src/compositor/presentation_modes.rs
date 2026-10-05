//! Typed presentation preferences and effective output-mode policy.
//!
//! Surface hints are intentionally kept separate from the mode selected for an
//! output.  The latter is a compositor decision and must be frozen into the
//! output transaction that owns the KMS submission.

use crate::native::presentation_timing::PresentationDomain;

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
                pending: self.pending,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TearingPolicy {
    #[default]
    Off,
    Auto,
}

/// User configuration for Adaptive Sync. This remains independent from
/// connector/CRTC support and the per-transaction qualification result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum VrrPolicy {
    Off,
    #[default]
    Auto,
    On,
}

impl VrrPolicy {
    pub fn from_environment(value: Option<&str>) -> Self {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("0" | "false" | "no" | "off" | "disable" | "disabled") => Self::Off,
            Some("1" | "true" | "yes" | "on" | "enable" | "enabled") => Self::On,
            _ => Self::Auto,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::On => "on",
        }
    }

    /// Phase 1's shared policy predicate for scheduler and transaction
    /// eligibility. Auto is limited to its caller's solitary-fullscreen hint.
    pub const fn allows_adaptive_sync(self, auto_candidate: bool) -> bool {
        match self {
            Self::Off => false,
            Self::Auto => auto_candidate,
            Self::On => true,
        }
    }
}

/// Name used by the native output policy. `TearingPolicy` remains the
/// compositor-facing spelling for callers that do not care about the layer.
pub type NativeTearingPreference = TearingPolicy;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VrrBlocker {
    PolicyDisabled,
    AutoCandidateIneligible,
    BackendCapabilityUnavailable,
    ConnectorCapabilityUnavailable,
    CrtcPropertyUnavailable,
    OutputGenerationUnqualified,
    ExactKmsQualificationRejected,
    UnsupportedTransition,
}

impl VrrBlocker {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PolicyDisabled => "policy_disabled",
            Self::AutoCandidateIneligible => "auto_candidate_ineligible",
            Self::BackendCapabilityUnavailable => "backend_capability_unavailable",
            Self::ConnectorCapabilityUnavailable => "connector_capability_unavailable",
            Self::CrtcPropertyUnavailable => "crtc_property_unavailable",
            Self::OutputGenerationUnqualified => "output_generation_unqualified",
            Self::ExactKmsQualificationRejected => "exact_kms_qualification_rejected",
            Self::UnsupportedTransition => "unsupported_transition",
        }
    }
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VrrEligibility {
    /// Phase 1's conservative Auto candidate: a solitary fullscreen tree.
    pub auto_candidate: bool,
    pub backend_capable: bool,
    pub connector_capable: bool,
    pub crtc_property_available: bool,
    pub output_generation_qualified: bool,
    pub exact_kms_qualified: bool,
    pub transition_supported: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum OutputPresentationMode {
    #[default]
    Vsync,
    AdaptiveSync,
    Async,
    AdaptiveAsync,
}

impl OutputPresentationMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vsync => "vsync",
            Self::AdaptiveSync => "adaptive_sync",
            Self::Async => "async",
            Self::AdaptiveAsync => "adaptive_async",
        }
    }

    pub const fn is_async(self) -> bool {
        matches!(self, Self::Async | Self::AdaptiveAsync)
    }

    pub const fn uses_vrr(self) -> bool {
        matches!(self, Self::AdaptiveSync | Self::AdaptiveAsync)
    }

    pub const fn presentation_domain(self) -> PresentationDomain {
        match self {
            Self::Vsync => PresentationDomain::FixedVsync,
            Self::AdaptiveSync | Self::AdaptiveAsync => PresentationDomain::VrrWindow,
            Self::Async => PresentationDomain::AsyncImmediate,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectivePresentation {
    pub mode: OutputPresentationMode,
    pub content_type: SurfaceContentType,
    pub vrr_blocker: Option<VrrBlocker>,
    pub async_blocker: Option<AsyncBlocker>,
}

impl EffectivePresentation {
    pub fn decide(
        policy: TearingPolicy,
        vrr_policy: VrrPolicy,
        metadata: SurfacePresentationMetadata,
        eligibility: AsyncEligibility,
        vrr_eligibility: VrrEligibility,
    ) -> Self {
        let vrr_blocker = if matches!(vrr_policy, VrrPolicy::Off) {
            Some(VrrBlocker::PolicyDisabled)
        } else if matches!(vrr_policy, VrrPolicy::Auto) && !vrr_eligibility.auto_candidate {
            Some(VrrBlocker::AutoCandidateIneligible)
        } else if !vrr_eligibility.backend_capable {
            Some(VrrBlocker::BackendCapabilityUnavailable)
        } else if !vrr_eligibility.connector_capable {
            Some(VrrBlocker::ConnectorCapabilityUnavailable)
        } else if !vrr_eligibility.crtc_property_available {
            Some(VrrBlocker::CrtcPropertyUnavailable)
        } else if !vrr_eligibility.output_generation_qualified {
            Some(VrrBlocker::OutputGenerationUnqualified)
        } else if !vrr_eligibility.exact_kms_qualified {
            Some(VrrBlocker::ExactKmsQualificationRejected)
        } else if !vrr_eligibility.transition_supported {
            Some(VrrBlocker::UnsupportedTransition)
        } else {
            None
        };
        let async_blocker = if !policy.allows_async_request() {
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
        let uses_vrr = vrr_blocker.is_none();
        let is_async = async_blocker.is_none();
        let mode = match (uses_vrr, is_async) {
            (false, false) => OutputPresentationMode::Vsync,
            (true, false) => OutputPresentationMode::AdaptiveSync,
            (false, true) => OutputPresentationMode::Async,
            (true, true) => OutputPresentationMode::AdaptiveAsync,
        };
        Self {
            mode,
            content_type: metadata.content_type,
            vrr_blocker,
            async_blocker,
        }
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
    fn captured_presentation_is_the_baseline_for_the_next_commit() {
        let state =
            SurfacePresentationState::default().set_pending_hint(SurfacePresentationHint::Async);
        let (state, first) = state.capture_pending_and_reset();
        let state = state.set_pending_content_type(SurfaceContentType::Video);
        let (state, second) = state.capture_pending_and_reset();
        let state = state.apply_captured(first).apply_captured(second);

        assert_eq!(first.metadata.hint, SurfacePresentationHint::Async);
        assert_eq!(first.metadata.content_type, SurfaceContentType::None);
        assert_eq!(second.metadata.hint, SurfacePresentationHint::Async);
        assert_eq!(second.metadata.content_type, SurfaceContentType::Video);
        assert_eq!(state.current(), second.metadata);
    }

    #[test]
    fn captured_content_type_is_the_baseline_for_an_unrelated_hint_change() {
        let state =
            SurfacePresentationState::default().set_pending_content_type(SurfaceContentType::Game);
        let (state, first) = state.capture_pending_and_reset();
        let state = state.set_pending_hint(SurfacePresentationHint::Async);
        let (state, second) = state.capture_pending_and_reset();
        let state = state.apply_captured(first).apply_captured(second);

        assert_eq!(first.metadata.hint, SurfacePresentationHint::Vsync);
        assert_eq!(first.metadata.content_type, SurfaceContentType::Game);
        assert_eq!(second.metadata.hint, SurfacePresentationHint::Async);
        assert_eq!(second.metadata.content_type, SurfaceContentType::Game);
        assert_eq!(state.current(), second.metadata);
    }

    #[test]
    fn buffer_only_capture_inherits_persistent_presentation_metadata() {
        let state = SurfacePresentationState::default()
            .set_pending_hint(SurfacePresentationHint::Async)
            .set_pending_content_type(SurfaceContentType::Photo);
        let (state, first) = state.capture_pending_and_reset();
        let (_, second) = state.capture_pending_and_reset();

        assert_eq!(second.metadata, first.metadata);
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
            VrrPolicy::Off,
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
            VrrEligibility::default(),
        );
        assert_eq!(result.mode, OutputPresentationMode::Vsync);
        assert_eq!(result.async_blocker, Some(AsyncBlocker::SurfaceHintMissing));
    }

    #[test]
    fn borderless_scanout_candidate_remains_vsync_only_under_auto_tearing() {
        let result = EffectivePresentation::decide(
            TearingPolicy::Auto,
            VrrPolicy::Off,
            SurfacePresentationMetadata {
                hint: SurfacePresentationHint::Async,
                content_type: SurfaceContentType::None,
            },
            AsyncEligibility {
                solitary_fullscreen: false,
                async_hint: true,
                backend_capable: true,
                output_generation_qualified: true,
                explicit_sync_ready: true,
                commit_timing_safe: true,
                kms_lane_free: true,
                async_test_only_accepted: true,
                async_format_supported: true,
                ..AsyncEligibility::default()
            },
            VrrEligibility::default(),
        );

        assert_eq!(result.mode, OutputPresentationMode::Vsync);
        assert_eq!(
            result.async_blocker,
            Some(AsyncBlocker::NotSolitaryFullscreen)
        );
    }

    #[test]
    fn solitary_fullscreen_scanout_candidate_remains_async_eligible() {
        let result = EffectivePresentation::decide(
            TearingPolicy::Auto,
            VrrPolicy::Off,
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
                async_format_supported: true,
                ..AsyncEligibility::default()
            },
            VrrEligibility::default(),
        );

        assert_eq!(result.mode, OutputPresentationMode::Async);
        assert_eq!(result.async_blocker, None);
    }

    #[test]
    fn async_format_compatibility_is_an_explicit_blocker() {
        let result = EffectivePresentation::decide(
            TearingPolicy::Auto,
            VrrPolicy::Off,
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
            VrrEligibility::default(),
        );
        assert_eq!(result.mode, OutputPresentationMode::Vsync);
        assert_eq!(
            result.async_blocker,
            Some(AsyncBlocker::AsyncFormatUnsupported)
        );
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

#[cfg(test)]
mod vrr_phase1_regression_tests {
    use super::*;

    fn async_eligible() -> AsyncEligibility {
        AsyncEligibility {
            solitary_fullscreen: true,
            async_hint: true,
            backend_capable: true,
            output_generation_qualified: true,
            cursor_visible: false,
            cursor_transition_pending: false,
            non_primary_plane_active: false,
            explicit_sync_ready: true,
            commit_timing_safe: true,
            kms_lane_free: true,
            async_test_only_accepted: true,
            async_format_supported: true,
            modeset_required: false,
        }
    }

    fn vrr_eligible() -> VrrEligibility {
        VrrEligibility {
            auto_candidate: true,
            backend_capable: true,
            connector_capable: true,
            crtc_property_available: true,
            output_generation_qualified: true,
            exact_kms_qualified: true,
            transition_supported: true,
        }
    }

    fn decide(vrr: bool, async_mode: bool) -> EffectivePresentation {
        let mut vrr_eligibility = vrr_eligible();
        if !vrr {
            vrr_eligibility.exact_kms_qualified = false;
        }
        let mut async_eligibility = async_eligible();
        if !async_mode {
            async_eligibility.async_test_only_accepted = false;
        }
        EffectivePresentation::decide(
            TearingPolicy::Auto,
            VrrPolicy::On,
            SurfacePresentationMetadata {
                hint: SurfacePresentationHint::Async,
                content_type: SurfaceContentType::Game,
            },
            async_eligibility,
            vrr_eligibility,
        )
    }

    #[test]
    fn output_modes_map_independent_vrr_and_async_qualification() {
        for (vrr, async_mode, expected) in [
            (false, false, OutputPresentationMode::Vsync),
            (true, false, OutputPresentationMode::AdaptiveSync),
            (false, true, OutputPresentationMode::Async),
            (true, true, OutputPresentationMode::AdaptiveAsync),
        ] {
            assert_eq!(decide(vrr, async_mode).mode, expected);
        }
    }

    #[test]
    fn vrr_and_async_failures_keep_independent_blockers() {
        let vrr_failed = decide(false, true);
        assert_eq!(vrr_failed.mode, OutputPresentationMode::Async);
        assert!(vrr_failed.vrr_blocker.is_some());
        assert_eq!(vrr_failed.async_blocker, None);

        let async_failed = decide(true, false);
        assert_eq!(async_failed.mode, OutputPresentationMode::AdaptiveSync);
        assert_eq!(async_failed.vrr_blocker, None);
        assert!(async_failed.async_blocker.is_some());

        let both_failed = decide(false, false);
        assert_eq!(both_failed.mode, OutputPresentationMode::Vsync);
        assert!(both_failed.vrr_blocker.is_some());
        assert!(both_failed.async_blocker.is_some());
    }

    #[test]
    fn auto_requires_the_solitary_fullscreen_candidate_but_on_does_not() {
        let mut eligibility = vrr_eligible();
        eligibility.auto_candidate = false;
        let auto = EffectivePresentation::decide(
            TearingPolicy::Off,
            VrrPolicy::Auto,
            SurfacePresentationMetadata {
                hint: SurfacePresentationHint::Vsync,
                content_type: SurfaceContentType::Game,
            },
            AsyncEligibility::default(),
            eligibility,
        );
        assert_eq!(auto.mode, OutputPresentationMode::Vsync);
        assert_eq!(auto.vrr_blocker, Some(VrrBlocker::AutoCandidateIneligible));

        let on = EffectivePresentation::decide(
            TearingPolicy::Off,
            VrrPolicy::On,
            SurfacePresentationMetadata::default(),
            AsyncEligibility::default(),
            eligibility,
        );
        assert_eq!(on.mode, OutputPresentationMode::AdaptiveSync);
        assert_eq!(on.vrr_blocker, None);

        let off = EffectivePresentation::decide(
            TearingPolicy::Off,
            VrrPolicy::Off,
            SurfacePresentationMetadata::default(),
            AsyncEligibility::default(),
            vrr_eligible(),
        );
        assert_eq!(off.mode, OutputPresentationMode::Vsync);
        assert_eq!(off.vrr_blocker, Some(VrrBlocker::PolicyDisabled));
    }

    #[test]
    fn environment_vrr_policy_keeps_auto_on_and_off_distinct() {
        assert_eq!(VrrPolicy::from_environment(Some("auto")), VrrPolicy::Auto);
        assert_eq!(VrrPolicy::from_environment(Some("on")), VrrPolicy::On);
        assert_eq!(VrrPolicy::from_environment(Some("off")), VrrPolicy::Off);
        assert_eq!(VrrPolicy::from_environment(None), VrrPolicy::Auto);
    }

    #[test]
    fn all_vrr_aliases_have_identical_scheduler_composited_and_direct_semantics() {
        let parse = |value| VrrPolicy::from_environment(Some(value));
        for alias in ["0", "false", "no", "off", "disable", "disabled"] {
            assert_eq!(parse(alias), VrrPolicy::Off, "alias {alias}");
        }
        for alias in ["1", "true", "yes", "on", "enable", "enabled"] {
            assert_eq!(parse(alias), VrrPolicy::On, "alias {alias}");
        }
        for alias in ["auto", "unknown", ""] {
            assert_eq!(parse(alias), VrrPolicy::Auto, "alias {alias}");
        }

        let mut eligibility = vrr_eligible();
        eligibility.auto_candidate = false;
        for alias in ["0", "false", "no", "off", "disable", "disabled"] {
            assert_eq!(
                EffectivePresentation::decide(
                    TearingPolicy::Off,
                    parse(alias),
                    SurfacePresentationMetadata::default(),
                    AsyncEligibility {
                        solitary_fullscreen: true,
                        ..AsyncEligibility::default()
                    },
                    vrr_eligible(),
                )
                .mode,
                OutputPresentationMode::Vsync,
                "Off alias {alias} must not produce an Adaptive transaction",
            );
        }
        for alias in ["1", "true", "yes", "on", "enable", "enabled"] {
            assert_eq!(
                EffectivePresentation::decide(
                    TearingPolicy::Off,
                    parse(alias),
                    SurfacePresentationMetadata::default(),
                    AsyncEligibility::default(),
                    eligibility,
                )
                .mode,
                OutputPresentationMode::AdaptiveSync,
                "On alias {alias} must not fall back to Auto eligibility",
            );
        }

        // Scheduler, composited output, and Direct Scanout all consume this
        // same immutable enum. Check the policy-to-mode result for each
        // accepted alias so no caller can silently narrow the vocabulary.
        for alias in [
            "0", "false", "no", "off", "disable", "disabled", "1", "true", "yes", "on", "enable",
            "enabled", "auto",
        ] {
            let policy = parse(alias);
            let expected = match policy {
                VrrPolicy::Off => OutputPresentationMode::Vsync,
                VrrPolicy::On => OutputPresentationMode::AdaptiveSync,
                VrrPolicy::Auto => OutputPresentationMode::AdaptiveSync,
            };
            let actual = EffectivePresentation::decide(
                TearingPolicy::Off,
                policy,
                SurfacePresentationMetadata::default(),
                AsyncEligibility::default(),
                vrr_eligible(),
            )
            .mode;
            assert_eq!(
                policy.allows_adaptive_sync(true),
                expected.uses_vrr(),
                "scheduler predicate for alias {alias}"
            );
            assert_eq!(
                policy.allows_adaptive_sync(false),
                policy == VrrPolicy::On,
                "non-fullscreen scheduler predicate for alias {alias}"
            );
            assert_eq!(actual, expected, "policy alias {alias}");
        }
    }

    #[test]
    fn visible_hardware_cursor_does_not_block_adaptive_sync() {
        let mut async_eligibility = async_eligible();
        async_eligibility.cursor_visible = true;
        let effective = EffectivePresentation::decide(
            TearingPolicy::Auto,
            VrrPolicy::On,
            SurfacePresentationMetadata {
                hint: SurfacePresentationHint::Async,
                content_type: SurfaceContentType::None,
            },
            async_eligibility,
            vrr_eligible(),
        );
        assert_eq!(effective.mode, OutputPresentationMode::AdaptiveSync);
        assert_eq!(effective.vrr_blocker, None);
        assert_eq!(
            effective.async_blocker,
            Some(AsyncBlocker::HardwareCursorVisible)
        );
    }

    #[test]
    fn presentation_mode_semantics_cover_four_transaction_modes() {
        for (mode, is_async, uses_vrr, domain) in [
            (
                OutputPresentationMode::Vsync,
                false,
                false,
                PresentationDomain::FixedVsync,
            ),
            (
                OutputPresentationMode::AdaptiveSync,
                false,
                true,
                PresentationDomain::VrrWindow,
            ),
            (
                OutputPresentationMode::Async,
                true,
                false,
                PresentationDomain::AsyncImmediate,
            ),
            (
                OutputPresentationMode::AdaptiveAsync,
                true,
                true,
                PresentationDomain::VrrWindow,
            ),
        ] {
            assert_eq!(mode.is_async(), is_async);
            assert_eq!(mode.uses_vrr(), uses_vrr);
            assert_eq!(mode.presentation_domain(), domain);
        }
    }
}
