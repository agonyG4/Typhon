mod atomic;
mod backend;
mod ioctl;
mod legacy;
mod properties;
mod state;

pub use atomic::*;
pub use backend::*;
pub use ioctl::*;
pub use legacy::*;
pub use properties::*;
pub use state::*;

use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KmsPolicy {
    Auto,
    Atomic,
    Legacy,
}

impl KmsPolicy {
    pub fn parse(value: Option<&str>) -> Result<Self, AtomicKmsError> {
        match value.unwrap_or("auto") {
            "auto" => Ok(Self::Auto),
            "atomic" => Ok(Self::Atomic),
            "legacy" => Ok(Self::Legacy),
            value => Err(AtomicKmsError::new(
                AtomicKmsErrorKind::InvalidPolicy,
                format!(
                    "invalid OBLIVION_ONE_KMS_MODE={value:?}; expected auto, atomic, or legacy"
                ),
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Atomic => "atomic",
            Self::Legacy => "legacy",
        }
    }

    pub const fn on_atomic_failure(self, phase: AtomicFailurePhase) -> AtomicFailureAction {
        if matches!(self, Self::Auto)
            && matches!(
                phase,
                AtomicFailurePhase::Capability
                    | AtomicFailurePhase::Discovery
                    | AtomicFailurePhase::TestOnly
            )
        {
            AtomicFailureAction::UseLegacy
        } else {
            AtomicFailureAction::Fail
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KmsBackendKind {
    Atomic,
    Legacy,
}

impl KmsBackendKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Atomic => "atomic",
            Self::Legacy => "legacy",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicFailurePhase {
    Capability,
    Discovery,
    TestOnly,
    InitialCommit,
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicFailureAction {
    UseLegacy,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicKmsErrorKind {
    InvalidPolicy,
    Unsupported,
    MissingObject,
    MissingProperty,
    DuplicateProperty,
    NoCompatiblePrimaryPlane,
    InvalidGeometry,
    DuplicateAssignment,
    AlreadyPending,
    BlobCreation,
    TestOnlyRejected,
    InitialCommitRejected,
    FlipRejected,
    Busy,
    PermissionOrSession,
    DeviceLost,
    RestoreFailed,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicKmsError {
    pub kind: AtomicKmsErrorKind,
    pub detail: String,
}

impl AtomicKmsError {
    pub fn new(kind: AtomicKmsErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for AtomicKmsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.detail)
    }
}

impl Error for AtomicKmsError {}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc, time::Instant};

    use super::*;

    fn property(id: u32, name: &str, value: u64) -> DrmProperty {
        DrmProperty::new(PropertyId::new(id).unwrap(), name, value)
    }

    fn complete_connector_properties() -> Vec<DrmProperty> {
        vec![property(1, "CRTC_ID", 42)]
    }

    fn complete_crtc_properties() -> Vec<DrmProperty> {
        vec![
            property(2, "ACTIVE", 1),
            property(3, "MODE_ID", 99),
            property(4, "VRR_ENABLED", 0),
        ]
    }

    fn complete_plane_properties() -> Vec<DrmProperty> {
        [
            "FB_ID", "CRTC_ID", "SRC_X", "SRC_Y", "SRC_W", "SRC_H", "CRTC_X", "CRTC_Y", "CRTC_W",
            "CRTC_H", "type",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, name)| property(10 + index as u32, name, 0))
        .collect()
    }

    #[test]
    fn kms_policy_parses_auto_atomic_and_legacy() {
        assert_eq!(KmsPolicy::parse(None).unwrap(), KmsPolicy::Auto);
        assert_eq!(KmsPolicy::parse(Some("auto")).unwrap(), KmsPolicy::Auto);
        assert_eq!(KmsPolicy::parse(Some("atomic")).unwrap(), KmsPolicy::Atomic);
        assert_eq!(KmsPolicy::parse(Some("legacy")).unwrap(), KmsPolicy::Legacy);
        assert!(KmsPolicy::parse(Some("invalid")).is_err());
    }

    #[test]
    fn legacy_kms_never_enables_explicit_triple_buffering() {
        let capabilities =
            crate::native::scheduler::SchedulerCapabilities::for_backend(KmsBackendKind::Legacy)
                .with_primary_plane_in_fence(true)
                .with_explicit_output_swapchain(true);

        assert!(!capabilities.render_ahead_allowed());
    }

    #[test]
    fn startup_policy_allows_only_pre_takeover_auto_fallback() {
        assert_eq!(
            KmsPolicy::Auto.on_atomic_failure(AtomicFailurePhase::Capability),
            AtomicFailureAction::UseLegacy
        );
        assert_eq!(
            KmsPolicy::Auto.on_atomic_failure(AtomicFailurePhase::TestOnly),
            AtomicFailureAction::UseLegacy
        );
        assert_eq!(
            KmsPolicy::Atomic.on_atomic_failure(AtomicFailurePhase::Capability),
            AtomicFailureAction::Fail
        );
        assert_eq!(
            KmsPolicy::Auto.on_atomic_failure(AtomicFailurePhase::InitialCommit),
            AtomicFailureAction::Fail
        );
        assert_eq!(
            KmsPolicy::Auto.on_atomic_failure(AtomicFailurePhase::Runtime),
            AtomicFailureAction::Fail
        );
    }

    #[test]
    fn property_discovery_requires_exact_object_specific_names() {
        assert!(AtomicConnectorProperties::discover(&complete_connector_properties()).is_ok());
        assert!(AtomicConnectorProperties::discover(&[]).is_err());
        assert!(AtomicCrtcProperties::discover(&complete_crtc_properties()).is_ok());
        assert!(AtomicCrtcProperties::discover(&[property(2, "ACTIVE", 1)]).is_err());
        assert!(AtomicPlaneProperties::discover(&complete_plane_properties()).is_ok());
        let mut missing = complete_plane_properties();
        missing.retain(|entry| entry.name() != "SRC_W");
        assert!(AtomicPlaneProperties::discover(&missing).is_err());
    }

    #[test]
    fn optional_properties_are_recorded_without_becoming_required() {
        let crtc = AtomicCrtcProperties::discover(&complete_crtc_properties()).unwrap();
        let plane = AtomicPlaneProperties::discover(&complete_plane_properties()).unwrap();

        assert!(crtc.vrr_enabled.is_some());
        assert!(crtc.out_fence_ptr.is_none());
        assert!(plane.in_fence_fd.is_none());
        assert!(plane.damage_clips.is_none());
    }

    #[test]
    fn duplicate_property_names_or_ids_are_rejected() {
        let duplicate_name = vec![property(1, "CRTC_ID", 0), property(2, "CRTC_ID", 0)];
        let duplicate_id = vec![property(1, "CRTC_ID", 0), property(1, "OTHER", 0)];

        assert!(PropertySet::new(DrmObjectKind::Connector, duplicate_name).is_err());
        assert!(PropertySet::new(DrmObjectKind::Connector, duplicate_id).is_err());
    }

    fn plane(
        id: u32,
        plane_type: PlaneType,
        possible_crtcs: u32,
        formats: &[u32],
        crtc_id: u32,
    ) -> PlaneCandidate {
        PlaneCandidate {
            id: PlaneId::new(id).unwrap(),
            plane_type,
            possible_crtcs,
            formats: formats.to_vec(),
            current_crtc: (crtc_id != 0).then(|| CrtcId::new(crtc_id).unwrap()),
        }
    }

    #[test]
    fn primary_plane_selection_is_compatible_and_deterministic() {
        let format = u32::from_le_bytes(*b"XR24");
        let candidates = vec![
            plane(9, PlaneType::Overlay, 1, &[format], 0),
            plane(7, PlaneType::Primary, 1, &[format], 0),
            plane(5, PlaneType::Primary, 1, &[format], 0),
        ];

        assert_eq!(
            select_primary_plane(&candidates, 0, CrtcId::new(42).unwrap(), format)
                .unwrap()
                .id,
            PlaneId::new(5).unwrap()
        );
    }

    #[test]
    fn primary_plane_selection_prefers_the_plane_already_on_the_selected_crtc() {
        let crtc = CrtcId::new(42).unwrap();
        let candidates = vec![
            PlaneCandidate {
                id: PlaneId::new(3).unwrap(),
                plane_type: PlaneType::Primary,
                possible_crtcs: 1,
                formats: vec![0x3432_5258],
                current_crtc: None,
            },
            PlaneCandidate {
                id: PlaneId::new(9).unwrap(),
                plane_type: PlaneType::Primary,
                possible_crtcs: 1,
                formats: vec![0x3432_5258],
                current_crtc: Some(crtc),
            },
        ];

        assert_eq!(
            select_primary_plane(&candidates, 0, crtc, 0x3432_5258)
                .unwrap()
                .id,
            PlaneId::new(9).unwrap()
        );
    }

    #[test]
    fn primary_plane_selection_rejects_wrong_type_mask_format_or_active_crtc() {
        let format = u32::from_le_bytes(*b"XR24");
        let crtc = CrtcId::new(42).unwrap();
        assert!(
            select_primary_plane(
                &[plane(1, PlaneType::Overlay, 1, &[format], 0)],
                0,
                crtc,
                format
            )
            .is_err()
        );
        assert!(
            select_primary_plane(
                &[plane(1, PlaneType::Primary, 2, &[format], 0)],
                0,
                crtc,
                format
            )
            .is_err()
        );
        assert!(
            select_primary_plane(&[plane(1, PlaneType::Primary, 1, &[0], 0)], 0, crtc, format)
                .is_err()
        );
        assert!(
            select_primary_plane(
                &[plane(1, PlaneType::Primary, 1, &[format], 77)],
                0,
                crtc,
                format
            )
            .is_err()
        );
    }

    #[test]
    fn fullscreen_geometry_uses_checked_unsigned_16_16_source_units() {
        let geometry = AtomicPlaneGeometry::fullscreen(3840, 2160).unwrap();
        assert_eq!(geometry.src_w, 3840u64 << 16);
        assert_eq!(geometry.src_h, 2160u64 << 16);
        assert_eq!(geometry.crtc_w, 3840);
        assert_eq!(geometry.crtc_h, 2160);
        assert!(AtomicPlaneGeometry::fullscreen(0, 2160).is_err());
        assert!(AtomicPlaneGeometry::fullscreen(u32::MAX, 1).is_err());
    }

    fn ids() -> (
        ConnectorId,
        CrtcId,
        PlaneId,
        AtomicConnectorProperties,
        AtomicCrtcProperties,
        AtomicPlaneProperties,
    ) {
        (
            ConnectorId::new(1).unwrap(),
            CrtcId::new(2).unwrap(),
            PlaneId::new(3).unwrap(),
            AtomicConnectorProperties::discover(&complete_connector_properties()).unwrap(),
            AtomicCrtcProperties::discover(&complete_crtc_properties()).unwrap(),
            AtomicPlaneProperties::discover(&complete_plane_properties()).unwrap(),
        )
    }

    #[test]
    fn initial_request_contains_exact_connector_crtc_and_primary_plane_state() {
        let (connector, crtc, plane, connector_props, crtc_props, plane_props) = ids();
        let request = AtomicRequest::initial_modeset(
            connector,
            crtc,
            plane,
            &connector_props,
            &crtc_props,
            &plane_props,
            BlobId::new(90).unwrap(),
            FramebufferId::new(80).unwrap(),
            AtomicPlaneGeometry::fullscreen(1920, 1080).unwrap(),
        )
        .unwrap();

        assert_eq!(request.assignment_count(), 13);
        assert_eq!(request.serialize().objects, vec![1, 2, 3]);
        assert!(!request.touches_object_kind(DrmObjectKind::CursorPlane));
    }

    #[test]
    fn flip_request_changes_only_primary_fb_and_preserves_token_and_flags() {
        let (_, _, plane, _, _, plane_props) = ids();
        let request =
            AtomicRequest::primary_flip(plane, plane_props.fb_id, FramebufferId::new(81).unwrap())
                .unwrap();
        let submission = AtomicSubmission::page_flip(request, PageFlipToken::new(55).unwrap());

        assert_eq!(submission.request.assignment_count(), 1);
        assert_eq!(submission.user_data, 55);
        assert_eq!(submission.flags, AtomicCommitFlags::page_flip());
        assert!(!submission.flags.contains_allow_modeset());
        assert!(submission.flags.contains_nonblock());
        assert!(submission.flags.contains_pageflip_event());
        assert_eq!(
            AtomicCommitFlags::initial_test(),
            AtomicCommitFlags::test_only_allow_modeset()
        );
        assert_eq!(
            AtomicCommitFlags::initial_real(),
            AtomicCommitFlags::allow_modeset()
        );
    }

    #[test]
    fn resume_modeset_rebuilds_complete_pipeline_with_allow_modeset() {
        let (connector, crtc, plane, connector_props, crtc_props, plane_props) = ids();
        let request = AtomicRequest::resume_modeset(
            connector,
            crtc,
            plane,
            &connector_props,
            &crtc_props,
            &plane_props,
            BlobId::new(91).unwrap(),
            FramebufferId::new(81).unwrap(),
            AtomicPlaneGeometry::fullscreen(1920, 1080).unwrap(),
        )
        .unwrap();
        let submission = AtomicSubmission::resume_modeset(request);

        assert_eq!(submission.request.assignment_count(), 13);
        assert!(submission.flags.contains_allow_modeset());
        assert_eq!(submission.user_data, 0);
    }

    #[test]
    fn pageflip_submission_preserves_full_nonzero_u64_token() {
        let (_, _, plane, _, _, plane_props) = ids();
        let request =
            AtomicRequest::primary_flip(plane, plane_props.fb_id, FramebufferId::new(81).unwrap())
                .unwrap();
        let token = u64::from(u32::MAX) + 17;
        let submission = AtomicSubmission::page_flip(request, PageFlipToken::new(token).unwrap());

        assert_eq!(submission.user_data, token);
    }

    #[test]
    fn request_rejects_duplicate_assignment_and_keeps_deterministic_order() {
        let (connector, _, _, connector_props, _, _) = ids();
        let mut request = AtomicRequest::new();
        request
            .set_connector(connector, connector_props.crtc_id, 7)
            .unwrap();
        assert!(
            request
                .set_connector(connector, connector_props.crtc_id, 8)
                .is_err()
        );
        assert_eq!(request.serialize().values, vec![7]);
    }

    #[test]
    fn commit_state_allows_one_pending_submission_and_exact_completion() {
        let mut state = AtomicCommitState::Idle;
        let token = PageFlipToken::new(41).unwrap();
        let framebuffer = FramebufferId::new(80).unwrap();
        state.begin(token, framebuffer, 9, Instant::now()).unwrap();

        assert!(
            state
                .begin(
                    PageFlipToken::new(42).unwrap(),
                    framebuffer,
                    9,
                    Instant::now()
                )
                .is_err()
        );
        assert_eq!(
            state.complete(PageFlipToken::new(42).unwrap(), 9),
            AtomicCompletion::Mismatched
        );
        assert!(state.is_pending());
        assert_eq!(state.complete(token, 8), AtomicCompletion::StaleGeneration);
        assert!(state.is_pending());
        assert_eq!(
            state.complete(token, 9),
            AtomicCompletion::Completed { framebuffer }
        );
        assert!(!state.is_pending());
        assert_eq!(state.complete(token, 9), AtomicCompletion::Stale);
    }

    #[test]
    fn failed_submission_returns_commit_state_to_idle_without_completion() {
        let mut state = AtomicCommitState::Idle;
        let token = PageFlipToken::new(41).unwrap();
        state
            .begin(token, FramebufferId::new(80).unwrap(), 9, Instant::now())
            .unwrap();
        assert!(state.submission_failed(token));
        assert!(!state.is_pending());
    }

    #[test]
    fn resumed_pageflip_generation_rejects_old_generation_events() {
        let mut state = AtomicCommitState::Idle;
        let old = PageFlipToken::new(51).unwrap();
        let resumed = PageFlipToken::new(52).unwrap();
        state
            .begin(old, FramebufferId::new(80).unwrap(), 7, Instant::now())
            .unwrap();

        state.abandon();
        state
            .begin(resumed, FramebufferId::new(81).unwrap(), 8, Instant::now())
            .unwrap();

        assert_eq!(
            state.complete(resumed, 7),
            AtomicCompletion::StaleGeneration
        );
        assert_eq!(state.complete(old, 8), AtomicCompletion::Mismatched);
        assert_eq!(
            state.complete(resumed, 8),
            AtomicCompletion::Completed {
                framebuffer: FramebufferId::new(81).unwrap()
            }
        );
    }

    #[derive(Clone)]
    struct CountingBlobIo {
        creates: Rc<Cell<u32>>,
        destroys: Rc<Cell<u32>>,
    }

    impl ModeBlobIo for CountingBlobIo {
        fn create_mode_blob(
            &self,
            _mode: &drm_sys::drm_mode_modeinfo,
        ) -> Result<BlobId, AtomicKmsError> {
            self.creates.set(self.creates.get() + 1);
            Ok(BlobId::new(77).unwrap())
        }

        fn destroy_mode_blob(&self, _blob: BlobId) -> Result<(), AtomicKmsError> {
            self.destroys.set(self.destroys.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn mode_blob_is_owned_and_destroyed_exactly_once() {
        let creates = Rc::new(Cell::new(0));
        let destroys = Rc::new(Cell::new(0));
        let io = CountingBlobIo {
            creates: Rc::clone(&creates),
            destroys: Rc::clone(&destroys),
        };
        let blob = ModeBlob::create(io, &drm_sys::drm_mode_modeinfo::default()).unwrap();
        assert_eq!(blob.id(), BlobId::new(77).unwrap());
        assert_eq!(creates.get(), 1);
        drop(blob);
        assert_eq!(destroys.get(), 1);
    }

    #[test]
    fn restore_and_safe_disable_requests_are_complete_and_cursor_free() {
        let (connector, crtc, plane, connector_props, crtc_props, plane_props) = ids();
        let pipeline = AtomicPipelineProperties {
            connector,
            crtc,
            plane,
            connector_props,
            crtc_props,
            plane_props,
        };
        let snapshot = AtomicPipelineSnapshot {
            connector_crtc_id: 12,
            crtc_active: 1,
            crtc_mode_id: 44,
            plane_fb_id: 55,
            plane_crtc_id: 12,
            src_x: 0,
            src_y: 0,
            src_w: 1920 << 16,
            src_h: 1080 << 16,
            crtc_x: 0,
            crtc_y: 0,
            crtc_w: 1920,
            crtc_h: 1080,
        };

        let restore = snapshot.restore_request(&pipeline).unwrap();
        let disable = AtomicRequest::safe_disable(&pipeline).unwrap();
        assert_eq!(restore.assignment_count(), 13);
        assert_eq!(disable.assignment_count(), 5);
        assert!(!restore.touches_object_kind(DrmObjectKind::CursorPlane));
        assert!(!disable.touches_object_kind(DrmObjectKind::CursorPlane));
        assert_eq!(disable.serialize().values, vec![0, 0, 0, 0, 0]);
    }
}
