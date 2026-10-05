use std::{
    cell::Cell,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    rc::Rc,
    time::Instant,
};

use crate::compositor::DrmContentType;

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
fn presentation_flag_contract_is_identical_for_test_only_and_real_submissions() {
    use crate::compositor::OutputPresentationMode::{AdaptiveAsync, AdaptiveSync, Async, Vsync};
    assert_eq!(
        AtomicCommitFlags::for_presentation(Vsync, false,),
        AtomicCommitFlags::page_flip()
    );
    assert_eq!(
        AtomicCommitFlags::for_presentation(Async, false,),
        AtomicCommitFlags::async_page_flip()
    );
    assert_eq!(
        AtomicCommitFlags::for_presentation(Vsync, true,),
        AtomicCommitFlags::test_only_no_modeset()
    );
    let async_test = AtomicCommitFlags::for_presentation(Async, true);
    assert!(async_test.contains_test_only());
    assert!(async_test.contains_pageflip_async());
    assert!(!async_test.contains_allow_modeset());
    assert!(!async_test.contains_nonblock());
    for adaptive_sync in [AdaptiveSync] {
        assert_eq!(
            AtomicCommitFlags::for_presentation(adaptive_sync, false),
            AtomicCommitFlags::page_flip()
        );
        assert_eq!(
            AtomicCommitFlags::for_presentation(adaptive_sync, true),
            AtomicCommitFlags::test_only_no_modeset()
        );
    }
    for adaptive_async in [AdaptiveAsync] {
        assert_eq!(
            AtomicCommitFlags::for_presentation(adaptive_async, false),
            AtomicCommitFlags::async_page_flip()
        );
        assert_eq!(
            AtomicCommitFlags::for_presentation(adaptive_async, true),
            AtomicCommitFlags::test_only_async_page_flip()
        );
    }
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
fn connector_vrr_capability_is_optional_and_preserves_identity_and_value() {
    let missing = AtomicConnectorProperties::discover(&complete_connector_properties()).unwrap();
    assert_eq!(missing.vrr_capable, None);
    assert_eq!(missing.vrr_capable_value, None);

    for value in [0, 1] {
        let mut properties = complete_connector_properties();
        properties.push(property(20, "vrr_capable", value));
        let discovered = AtomicConnectorProperties::discover(&properties).unwrap();
        assert_eq!(discovered.vrr_capable.unwrap().0.get(), 20);
        assert_eq!(discovered.vrr_capable_value, Some(value));
    }
}

#[test]
fn atomic_vrr_requires_connector_true_and_crtc_property() {
    let mut discovered = discovery();
    assert!(!discovered.vrr_capable());

    let connector = AtomicConnectorProperties::discover(&[
        property(1, "CRTC_ID", 42),
        property(20, "vrr_capable", 1),
    ])
    .unwrap();
    discovered.pipeline.connector_props = connector;
    assert!(discovered.vrr_capable());

    discovered.pipeline.connector_props.vrr_capable_value = Some(0);
    assert!(!discovered.vrr_capable());
    discovered.pipeline.connector_props.vrr_capable_value = Some(1);
    discovered.pipeline.crtc_props.vrr_enabled = None;
    assert!(!discovered.vrr_capable());
}

#[test]
fn recovery_refreshes_live_vrr_capability_across_generation_rebinds_only() {
    fn with_live_vrr(
        discovered: &mut AtomicDiscovery,
        connector_value: Option<u64>,
        crtc_property_available: bool,
    ) {
        let mut connector_properties = complete_connector_properties();
        if let Some(value) = connector_value {
            connector_properties.push(property(20, "vrr_capable", value));
        }
        discovered.pipeline.connector_props =
            AtomicConnectorProperties::discover(&connector_properties).unwrap();

        let mut crtc_properties = vec![property(2, "ACTIVE", 1), property(3, "MODE_ID", 99)];
        if crtc_property_available {
            crtc_properties.push(property(4, "VRR_ENABLED", 0));
        }
        discovered.pipeline.crtc_props = AtomicCrtcProperties::discover(&crtc_properties).unwrap();
        discovered.optional.vrr_enabled = crtc_property_available;
    }

    let mut current = discovery();
    with_live_vrr(&mut current, Some(1), true);
    let original_restore_snapshot = current.snapshot;
    assert!(current.vrr_capable());

    for (connector_value, crtc_available, expected) in [
        (Some(1), true, true),
        (Some(0), false, false),
        (Some(1), true, true),
    ] {
        let mut refreshed = discovery();
        with_live_vrr(&mut refreshed, connector_value, crtc_available);
        current.refresh_live_pipeline_state(&refreshed);

        assert_eq!(current.vrr_capable(), expected);
        assert_eq!(current.optional.vrr_enabled, crtc_available);
        assert_eq!(current.snapshot, original_restore_snapshot);
    }

    let mut connector_property_disappeared = discovery();
    with_live_vrr(&mut connector_property_disappeared, None, true);
    current.refresh_live_pipeline_state(&connector_property_disappeared);
    assert!(!current.vrr_capable());
    assert_eq!(current.snapshot, original_restore_snapshot);
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

fn cursor_plane(id: u32, possible_crtcs: u32, formats: &[u32], crtc_id: u32) -> PlaneCandidate {
    plane(id, PlaneType::Cursor, possible_crtcs, formats, crtc_id)
}

#[test]
fn cursor_plane_selection_requires_compatible_crtc_and_argb8888() {
    let argb = u32::from_le_bytes(*b"AR24");
    let crtc = CrtcId::new(42).unwrap();
    let candidates = vec![
        cursor_plane(3, 2, &[argb], 0),
        cursor_plane(7, 1, &[u32::from_le_bytes(*b"XR24")], 0),
        cursor_plane(9, 1, &[argb], 0),
    ];

    assert_eq!(
        select_cursor_plane(&candidates, 0, crtc, argb, PlaneId::new(5).unwrap())
            .unwrap()
            .id,
        PlaneId::new(9).unwrap()
    );
}

#[test]
fn cursor_plane_selection_never_aliases_primary_plane() {
    let argb = u32::from_le_bytes(*b"AR24");
    let crtc = CrtcId::new(42).unwrap();
    let candidates = vec![
        plane(5, PlaneType::Primary, 1, &[argb], 0),
        cursor_plane(5, 1, &[argb], 0),
    ];

    let selected = select_cursor_plane(&candidates, 0, crtc, argb, PlaneId::new(5).unwrap());
    assert!(selected.is_none());
}

#[test]
fn cursor_plane_selection_allows_absence() {
    let argb = u32::from_le_bytes(*b"AR24");
    assert!(
        select_cursor_plane(
            &[],
            0,
            CrtcId::new(42).unwrap(),
            argb,
            PlaneId::new(5).unwrap()
        )
        .is_none()
    );
}

#[test]
fn cursor_dimensions_honor_advertised_caps_and_fallback_only_for_zero_or_unavailable() {
    assert_eq!(cursor_dimension_from_capability(Some(128)), 128);
    assert_eq!(cursor_dimension_from_capability(Some(0)), 64);
    assert_eq!(cursor_dimension_from_capability(None), 64);
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
        select_primary_plane(&[plane(1, PlaneType::Primary, 1, &[0], 0)], 0, crtc, format).is_err()
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

#[test]
fn full_source_to_output_geometry_matches_fullscreen_for_identity_dimensions() {
    let scaled = AtomicPlaneGeometry::full_source_to_output(1920, 1080, 1920, 1080).unwrap();

    assert_eq!(scaled, AtomicPlaneGeometry::fullscreen(1920, 1080).unwrap());
}

#[test]
fn full_source_to_output_geometry_scales_full_buffer_to_full_output() {
    let geometry = AtomicPlaneGeometry::full_source_to_output(1600, 900, 1920, 1080).unwrap();

    assert_eq!(
        geometry,
        AtomicPlaneGeometry {
            src_x: 0,
            src_y: 0,
            src_w: 1600u64 << 16,
            src_h: 900u64 << 16,
            crtc_x: 0,
            crtc_y: 0,
            crtc_w: 1920,
            crtc_h: 1080,
        }
    );
}

#[test]
fn full_source_to_output_geometry_rejects_zero_dimensions_and_source_overflow() {
    for dimensions in [
        (0, 900, 1920, 1080),
        (1600, 0, 1920, 1080),
        (1600, 900, 0, 1080),
        (1600, 900, 1920, 0),
    ] {
        assert!(
            AtomicPlaneGeometry::full_source_to_output(
                dimensions.0,
                dimensions.1,
                dimensions.2,
                dimensions.3,
            )
            .is_err()
        );
    }
    assert!(AtomicPlaneGeometry::full_source_to_output(65_536, 900, 1920, 1080).is_err());
    assert!(AtomicPlaneGeometry::full_source_to_output(1600, 65_536, 1920, 1080).is_err());
}

#[test]
fn primary_geometry_request_assigns_framebuffer_crtc_and_all_geometry_properties() {
    let pipeline = explicit_fence_pipeline();
    let framebuffer = FramebufferId::new(81).unwrap();
    let geometry = AtomicPlaneGeometry::full_source_to_output(1600, 900, 1920, 1080).unwrap();
    let request =
        AtomicRequest::primary_flip_with_geometry(&pipeline, framebuffer, geometry).unwrap();
    let serialized = request.serialize();
    let assignments = serialized
        .properties
        .iter()
        .copied()
        .zip(serialized.values.iter().copied())
        .collect::<std::collections::HashMap<_, _>>();

    assert_eq!(serialized.objects, vec![pipeline.plane.get()]);
    assert_eq!(serialized.property_counts, vec![10]);
    assert_eq!(request.assignment_count(), 10);
    assert_eq!(
        assignments[&pipeline.plane_props.fb_id.0.get()],
        u64::from(framebuffer.get())
    );
    assert_eq!(
        assignments[&pipeline.plane_props.crtc_id.0.get()],
        u64::from(pipeline.crtc.get())
    );
    assert_eq!(
        assignments[&pipeline.plane_props.src_x.0.get()],
        geometry.src_x
    );
    assert_eq!(
        assignments[&pipeline.plane_props.src_y.0.get()],
        geometry.src_y
    );
    assert_eq!(
        assignments[&pipeline.plane_props.src_w.0.get()],
        geometry.src_w
    );
    assert_eq!(
        assignments[&pipeline.plane_props.src_h.0.get()],
        geometry.src_h
    );
    assert_eq!(
        assignments[&pipeline.plane_props.crtc_x.0.get()],
        geometry.crtc_x
    );
    assert_eq!(
        assignments[&pipeline.plane_props.crtc_y.0.get()],
        geometry.crtc_y
    );
    assert_eq!(
        assignments[&pipeline.plane_props.crtc_w.0.get()],
        geometry.crtc_w
    );
    assert_eq!(
        assignments[&pipeline.plane_props.crtc_h.0.get()],
        geometry.crtc_h
    );

    let submission = AtomicSubmission::test_only(request);
    assert!(submission.flags.contains_test_only());
    assert!(!submission.flags.contains_allow_modeset());
    assert!(!submission.flags.contains_pageflip_event());
    assert_eq!(submission.user_data, 0);
}

#[test]
fn primary_geometry_request_forces_rotate_zero_when_rotation_is_supported() {
    let mut pipeline = explicit_fence_pipeline();
    let rotation_property = PlanePropertyId(PropertyId::new(100).unwrap());
    pipeline.plane_props.rotation = Some(rotation_property);
    let request = AtomicRequest::primary_flip_with_geometry(
        &pipeline,
        FramebufferId::new(81).unwrap(),
        AtomicPlaneGeometry::full_source_to_output(1600, 900, 1920, 1080).unwrap(),
    )
    .unwrap();
    let serialized = request.serialize();
    let index = serialized
        .properties
        .iter()
        .position(|property| *property == rotation_property.0.get())
        .expect("rotation property is part of the request");

    assert_eq!(
        serialized.values[index],
        u64::from(drm_sys::DRM_MODE_ROTATE_0)
    );
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

fn cursor_properties() -> AtomicCursorPlaneProperties {
    let properties = complete_plane_properties()
        .into_iter()
        .map(|entry| DrmProperty::new(entry.id(), entry.name(), entry.value))
        .collect::<Vec<_>>();
    AtomicCursorPlaneProperties {
        plane_id: 4,
        crtc_id: 2,
        fb_id: 0,
        crtc_x: 0,
        crtc_y: 0,
        crtc_w: 0,
        crtc_h: 0,
        src_x: 0,
        src_y: 0,
        src_w: 0,
        src_h: 0,
        in_formats: None,
        rotation: None,
        property_ids: AtomicPlaneProperties::discover_cursor(&properties).unwrap(),
        format_modifier: DrmFormatModifierPair {
            fourcc: DRM_FORMAT_ARGB8888,
            modifier: 0,
        },
        alpha_maximum: None,
        pixel_blend_mode_premultiplied: None,
    }
}

fn cursor_properties_with_blend() -> AtomicCursorPlaneProperties {
    let mut properties = complete_plane_properties()
        .into_iter()
        .map(|entry| DrmProperty::new(entry.id(), entry.name(), entry.value))
        .collect::<Vec<_>>();
    properties.push(DrmProperty::with_metadata(
        PropertyId::new(30).unwrap(),
        "alpha",
        0,
        vec![0, 65_535],
        Vec::new(),
    ));
    properties.push(DrmProperty::with_metadata(
        PropertyId::new(31).unwrap(),
        "pixel blend mode",
        17,
        Vec::new(),
        vec![
            DrmPropertyEnum {
                value: 17,
                name: "Coverage".to_string(),
            },
            DrmPropertyEnum {
                value: 41,
                name: "Pre-multiplied".to_string(),
            },
        ],
    ));
    let property_set = PropertySet::new(DrmObjectKind::CursorPlane, properties.clone()).unwrap();
    AtomicCursorPlaneProperties {
        property_ids: AtomicPlaneProperties::discover_cursor(&properties).unwrap(),
        alpha_maximum: property_set.alpha_maximum(),
        pixel_blend_mode_premultiplied: property_set.premultiplied_blend_value().flatten(),
        ..cursor_properties()
    }
}

fn pipeline_with_cursor() -> AtomicPipelineProperties {
    let (connector, crtc, plane, connector_props, crtc_props, plane_props) = ids();
    AtomicPipelineProperties {
        connector,
        crtc,
        plane,
        connector_props,
        crtc_props,
        plane_props,
        cursor_plane: Some(cursor_properties()),
    }
}

fn visible_cursor() -> AtomicCursorVisualState {
    AtomicCursorVisualState {
        visible: true,
        x: 25,
        y: 40,
        hotspot_x: 5,
        hotspot_y: 7,
        width: 64,
        height: 64,
        framebuffer_id: Some(99),
        image_generation: 3,
    }
}

fn explicit_fence_pipeline() -> AtomicPipelineProperties {
    let (connector, crtc, plane, _, _, _) = ids();
    let connector_props = AtomicConnectorProperties::discover(&[
        property(1, "CRTC_ID", 42),
        property(20, "vrr_capable", 1),
    ])
    .unwrap();
    let mut crtc_properties = complete_crtc_properties();
    crtc_properties.push(property(5, "OUT_FENCE_PTR", 0));
    let mut plane_properties = complete_plane_properties();
    plane_properties.push(property(30, "IN_FENCE_FD", 0));
    AtomicPipelineProperties {
        connector,
        crtc,
        plane,
        connector_props,
        crtc_props: AtomicCrtcProperties::discover(&crtc_properties).unwrap(),
        plane_props: AtomicPlaneProperties::discover(&plane_properties).unwrap(),
        cursor_plane: None,
    }
}

fn presentation_state_pipeline() -> AtomicPipelineProperties {
    let (connector, crtc, plane, _, crtc_props, plane_props) = ids();
    let connector_props = AtomicConnectorProperties::discover(&[
        property(1, "CRTC_ID", 42),
        property(20, "vrr_capable", 1),
        DrmProperty::with_metadata(
            PropertyId::new(21).unwrap(),
            "Content Type",
            0,
            Vec::new(),
            vec![
                DrmPropertyEnum {
                    value: 0,
                    name: "Graphics".to_string(),
                },
                DrmPropertyEnum {
                    value: 3,
                    name: "Game".to_string(),
                },
            ],
        ),
    ])
    .unwrap();
    AtomicPipelineProperties {
        connector,
        crtc,
        plane,
        connector_props,
        crtc_props,
        plane_props,
        cursor_plane: None,
    }
}

#[derive(Debug)]
struct FakeRuntimeModeBlobIo {
    next: Cell<u32>,
    destroyed: Rc<std::cell::RefCell<Vec<u32>>>,
}

impl ModeBlobIo for FakeRuntimeModeBlobIo {
    fn create_mode_blob(
        &self,
        _mode: &drm_sys::drm_mode_modeinfo,
    ) -> Result<BlobId, AtomicKmsError> {
        let id = self.next.get();
        self.next.set(id.saturating_add(1));
        Ok(BlobId::new(id).expect("fake blob IDs are nonzero"))
    }

    fn destroy_mode_blob(&self, blob: BlobId) -> Result<(), AtomicKmsError> {
        self.destroyed.borrow_mut().push(blob.get());
        Ok(())
    }
}

fn atomic_request_assignments(
    request: &AtomicRequest,
) -> std::collections::BTreeMap<(u32, u32), u64> {
    let serialized = request.serialize();
    let mut result = std::collections::BTreeMap::new();
    let mut index = 0usize;
    for (object, count) in serialized.objects.iter().zip(&serialized.property_counts) {
        for _ in 0..*count {
            result.insert(
                (*object, serialized.properties[index]),
                serialized.values[index],
            );
            index += 1;
        }
    }
    result
}

#[test]
fn runtime_modeset_test_and_commit_share_one_candidate_and_adopt_after_commit() {
    let mut pipeline = explicit_fence_pipeline();
    pipeline.cursor_plane = Some(cursor_properties());
    let mode = drm_sys::drm_mode_modeinfo {
        clock: 148_352,
        hdisplay: 1920,
        hsync_start: 2008,
        hsync_end: 2052,
        htotal: 2200,
        vdisplay: 1080,
        vsync_start: 1084,
        vsync_end: 1089,
        vtotal: 1125,
        vrefresh: 60,
        ..Default::default()
    };
    let destroyed = Rc::new(std::cell::RefCell::new(Vec::new()));
    let io = FakeRuntimeModeBlobIo {
        next: Cell::new(500),
        destroyed: Rc::clone(&destroyed),
    };
    let current_blob = ModeBlob::create(
        FakeRuntimeModeBlobIo {
            next: Cell::new(499),
            destroyed: Rc::clone(&destroyed),
        },
        &mode,
    )
    .unwrap();
    let previous_blob_id = current_blob.id();
    let framebuffer = FramebufferId::new(73).unwrap();
    let cursor = visible_cursor();
    let mut candidate = PreparedAtomicRuntimeModeset::prepare(
        io,
        &pipeline,
        mode,
        1920,
        1080,
        framebuffer,
        Some(cursor),
    )
    .unwrap();
    let (candidate_blob_id, candidate_mode, candidate_geometry, candidate_fb, candidate_cursor) =
        candidate.candidate_parts();
    assert_ne!(candidate_blob_id, previous_blob_id);
    assert_eq!(candidate_mode.clock, mode.clock);
    assert_eq!(candidate_geometry.crtc_w, 1920);
    assert_eq!(candidate_geometry.crtc_h, 1080);
    assert_eq!(candidate_fb, framebuffer);
    assert_eq!(candidate_cursor, Some(cursor));

    let test_submission = std::cell::RefCell::new(None);
    candidate
        .test_only_with(
            |submission| {
                *test_submission.borrow_mut() = Some(submission.clone());
                assert!(submission.flags.contains_test_only());
                assert!(submission.flags.contains_allow_modeset());
                assert_eq!(submission.user_data, 0);
                Ok(())
            },
            &pipeline,
        )
        .unwrap();
    assert!(destroyed.borrow().is_empty());

    let real_submission = std::cell::RefCell::new(None);
    candidate
        .commit_with(
            |submission| {
                *real_submission.borrow_mut() = Some(submission.clone());
                assert!(!submission.flags.contains_test_only());
                assert!(submission.flags.contains_allow_modeset());
                assert_eq!(submission.user_data, 0);
                Ok(())
            },
            &pipeline,
        )
        .unwrap();
    let test_submission = test_submission.into_inner().unwrap();
    let real_submission = real_submission.into_inner().unwrap();
    let mut test_assignments = atomic_request_assignments(&test_submission.request);
    let real_assignments = atomic_request_assignments(&real_submission.request);
    if let Some(in_fence) = pipeline.plane_props.in_fence_fd {
        test_assignments.remove(&(pipeline.plane.get(), in_fence.0.get()));
    }
    assert_eq!(test_assignments, real_assignments);
    assert_eq!(candidate.request(), &real_submission.request);

    let adopted = candidate.adopt().expect("real commit adopts the candidate");
    assert_eq!(adopted.mode_blob_id(), candidate_blob_id);
    assert_eq!(adopted.mode().clock, mode.clock);
    assert_eq!(adopted.geometry().crtc_w, 1920);
    drop(current_blob);
    assert_eq!(*destroyed.borrow(), vec![previous_blob_id.get()]);
    assert_eq!(adopted.mode_blob_id(), candidate_blob_id);
    drop(adopted);
    assert_eq!(destroyed.borrow().len(), 2);
}

#[test]
fn rejected_runtime_modeset_real_commit_does_not_adopt_and_destroys_its_blob_once() {
    let pipeline = presentation_state_pipeline();
    let mode = drm_sys::drm_mode_modeinfo {
        clock: 74_176,
        hdisplay: 1280,
        hsync_start: 1390,
        hsync_end: 1430,
        htotal: 1650,
        vdisplay: 720,
        vsync_start: 725,
        vsync_end: 730,
        vtotal: 750,
        vrefresh: 60,
        ..Default::default()
    };
    let destroyed = Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut candidate = PreparedAtomicRuntimeModeset::prepare(
        FakeRuntimeModeBlobIo {
            next: Cell::new(650),
            destroyed: Rc::clone(&destroyed),
        },
        &pipeline,
        mode,
        1280,
        720,
        FramebufferId::new(91).unwrap(),
        None,
    )
    .unwrap();
    let candidate_blob_id = candidate.candidate_parts().0;
    candidate
        .test_only_with(|_| Ok(()), &pipeline)
        .expect("the injected TEST_ONLY submission succeeds");

    let rejection = AtomicKmsError::new(
        AtomicKmsErrorKind::InitialCommitRejected,
        "injected real commit rejection",
    );
    assert!(
        candidate
            .commit_with(|_| Err(rejection.clone()), &pipeline)
            .is_err()
    );
    assert!(candidate.adopt().is_err());
    assert_eq!(destroyed.borrow().as_slice(), &[candidate_blob_id.get()]);
}

#[test]
fn rejected_runtime_modeset_candidate_does_not_adopt_and_destroys_its_blob_once() {
    let pipeline = presentation_state_pipeline();
    let mode = drm_sys::drm_mode_modeinfo {
        clock: 74_176,
        hdisplay: 1280,
        hsync_start: 1390,
        hsync_end: 1430,
        htotal: 1650,
        vdisplay: 720,
        vsync_start: 725,
        vsync_end: 730,
        vtotal: 750,
        vrefresh: 60,
        ..Default::default()
    };
    let destroyed = Rc::new(std::cell::RefCell::new(Vec::new()));
    let io = FakeRuntimeModeBlobIo {
        next: Cell::new(600),
        destroyed: Rc::clone(&destroyed),
    };
    let mut candidate = PreparedAtomicRuntimeModeset::prepare(
        io,
        &pipeline,
        mode,
        1280,
        720,
        FramebufferId::new(81).unwrap(),
        None,
    )
    .unwrap();
    let candidate_blob_id = candidate.candidate_parts().0;
    let error = AtomicKmsError::new(AtomicKmsErrorKind::TestOnlyRejected, "injected rejection");
    assert!(
        candidate
            .test_only_with(|_| Err(error.clone()), &pipeline)
            .is_err()
    );
    assert!(destroyed.borrow().is_empty());
    assert!(candidate.adopt().is_err());
    assert_eq!(destroyed.borrow().as_slice(), &[candidate_blob_id.get()]);
}

#[test]
fn adaptive_sync_allows_cursor_mutation_but_adaptive_async_rejects_it() {
    use crate::compositor::OutputPresentationMode::{AdaptiveAsync, AdaptiveSync};

    let mut pipeline = explicit_fence_pipeline();
    pipeline.cursor_plane = Some(cursor_properties());
    let cursor = visible_cursor();
    let called = Cell::new(false);
    let vrr_property = pipeline.crtc_props.vrr_enabled.unwrap().0.get();
    super::submission::submit_atomic_flip_with(
        &pipeline,
        AtomicFlipRequest {
            framebuffer: FramebufferId::new(81).unwrap(),
            token: PageFlipToken::new(82).unwrap(),
            in_fence: pipe_read_end(),
            cursor: Some(cursor.clone()),
            presentation_mode: AdaptiveSync,
            content_type: DrmContentType::Graphics,
        },
        |submission| {
            called.set(true);
            assert!(!submission.flags.contains_pageflip_async());
            let serialized = submission.request.serialize();
            let assignments = serialized
                .properties
                .iter()
                .copied()
                .zip(serialized.values.iter().copied())
                .collect::<std::collections::HashMap<_, _>>();
            assert_eq!(assignments[&vrr_property], 1);
            assert!(
                submission
                    .request
                    .touches_object_kind(DrmObjectKind::CursorPlane)
            );
            Ok(())
        },
    )
    .unwrap();
    assert!(called.get());

    let called = Cell::new(false);
    let error = super::submission::submit_atomic_flip_with(
        &pipeline,
        AtomicFlipRequest {
            framebuffer: FramebufferId::new(81).unwrap(),
            token: PageFlipToken::new(83).unwrap(),
            in_fence: pipe_read_end(),
            cursor: Some(cursor),
            presentation_mode: AdaptiveAsync,
            content_type: DrmContentType::Graphics,
        },
        |_| {
            called.set(true);
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, AtomicKmsErrorKind::Unsupported);
    assert!(!called.get());
}

#[test]
fn atomic_presentation_state_and_test_only_real_requests_match_for_all_modes() {
    use crate::compositor::OutputPresentationMode::{AdaptiveAsync, AdaptiveSync, Async, Vsync};

    let pipeline = presentation_state_pipeline();
    let vrr_property = pipeline.crtc_props.vrr_enabled.unwrap().0.get();
    let content_property = pipeline.connector_props.content_type.unwrap().0.get();
    for (mode, expected_vrr, expected_flags) in [
        (Vsync, 0, AtomicCommitFlags::page_flip()),
        (AdaptiveSync, 1, AtomicCommitFlags::page_flip()),
        (Async, 0, AtomicCommitFlags::async_page_flip()),
        (AdaptiveAsync, 1, AtomicCommitFlags::async_page_flip()),
    ] {
        let mut request = AtomicRequest::new();
        request
            .set_presentation_state(&pipeline, mode, DrmContentType::Game)
            .unwrap();
        let serialized = request.serialize();
        let values = serialized
            .properties
            .iter()
            .copied()
            .zip(serialized.values.iter().copied())
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(values[&vrr_property], expected_vrr);
        assert_eq!(values[&content_property], 3);

        let token = PageFlipToken::new(55).unwrap();
        let test = AtomicSubmission::for_presentation(request.clone(), token, mode, true);
        let real = AtomicSubmission::for_presentation(request, token, mode, false);
        assert_eq!(test.request, real.request);
        assert_eq!(real.flags, expected_flags);
        assert!(test.flags.contains_test_only());
        assert_eq!(test.flags.contains_pageflip_async(), mode.is_async());
        assert!(!test.flags.contains_allow_modeset());
    }
}

#[test]
fn adaptive_presentation_requires_vrr_capability_and_programs_the_crtc_property() {
    use crate::compositor::OutputPresentationMode::{AdaptiveAsync, AdaptiveSync};

    let mut missing_property = presentation_state_pipeline();
    missing_property.crtc_props.vrr_enabled = None;
    let error = AtomicRequest::new()
        .set_presentation_state(&missing_property, AdaptiveSync, DrmContentType::Graphics)
        .unwrap_err();
    assert_eq!(error.kind, AtomicKmsErrorKind::MissingProperty);

    let mut unsupported_connector = presentation_state_pipeline();
    unsupported_connector.connector_props.vrr_capable_value = Some(0);
    let error = AtomicRequest::new()
        .set_presentation_state(
            &unsupported_connector,
            AdaptiveAsync,
            DrmContentType::Graphics,
        )
        .unwrap_err();
    assert_eq!(error.kind, AtomicKmsErrorKind::Unsupported);

    let pipeline = presentation_state_pipeline();
    let vrr_property = pipeline.crtc_props.vrr_enabled.unwrap().0.get();
    let mut request = AtomicRequest::new();
    request
        .set_presentation_state(&pipeline, AdaptiveSync, DrmContentType::Graphics)
        .unwrap();
    let serialized = request.serialize();
    let submitted_vrr_value = serialized
        .properties
        .iter()
        .copied()
        .zip(serialized.values.iter().copied())
        .find_map(|(property, value)| (property == vrr_property).then_some(value));
    assert_eq!(submitted_vrr_value, Some(1));
}

#[test]
fn vsync_presentation_allows_missing_optional_vrr_enabled() {
    let mut pipeline = presentation_state_pipeline();
    let vrr_property = pipeline.crtc_props.vrr_enabled.unwrap().0.get();
    pipeline.crtc_props.vrr_enabled = None;

    let mut request = AtomicRequest::new();
    request
        .set_presentation_state(
            &pipeline,
            crate::compositor::OutputPresentationMode::Vsync,
            DrmContentType::Game,
        )
        .unwrap();
    let serialized = request.serialize();
    assert!(!serialized.values.is_empty());
    assert!(!serialized.properties.contains(&vrr_property));
}

fn pipe_read_end() -> OwnedFd {
    let mut pipe = [-1; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
        0
    );
    unsafe { libc::close(pipe[1]) };
    unsafe { OwnedFd::from_raw_fd(pipe[0]) }
}

fn pipe_read_end_at_least(min_fd: i32) -> OwnedFd {
    let pipe = pipe_read_end();
    // SAFETY: `pipe` owns a valid descriptor, and `F_DUPFD_CLOEXEC` returns a
    // new descriptor referring to the same open file description.
    let duplicated = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_DUPFD_CLOEXEC, min_fd) };
    assert!(duplicated >= min_fd);
    drop(pipe);
    // SAFETY: `duplicated` was returned by `F_DUPFD_CLOEXEC` above and is now
    // the sole owner of the duplicated descriptor.
    unsafe { OwnedFd::from_raw_fd(duplicated) }
}

fn discovery() -> AtomicDiscovery {
    let (connector, crtc, plane, connector_props, crtc_props, plane_props) = ids();
    AtomicDiscovery {
        pipeline: AtomicPipelineProperties {
            connector,
            crtc,
            plane,
            connector_props,
            crtc_props,
            plane_props,
            cursor_plane: None,
        },
        snapshot: AtomicPipelineSnapshot {
            connector_crtc_id: 0,
            connector_content_type: None,
            crtc_active: 0,
            crtc_mode_id: 0,
            crtc_vrr_enabled: Some(0),
            plane_fb_id: 0,
            plane_crtc_id: 0,
            src_x: 0,
            src_y: 0,
            src_w: 0,
            src_h: 0,
            crtc_x: 0,
            crtc_y: 0,
            crtc_w: 0,
            crtc_h: 0,
            cursor: None,
        },
        optional: AtomicOptionalCapabilities {
            vrr_enabled: false,
            in_fence_fd: true,
            out_fence_ptr: false,
            framebuffer_damage_clips: false,
            async_page_flip: false,
        },
        framebuffer_format: u32::from_le_bytes(*b"XR24"),
        plane_possible_crtcs: 1,
        plane_formats: vec![u32::from_le_bytes(*b"XR24")],
        plane_scanout_formats: Vec::new(),
        plane_async_scanout_formats: Vec::new(),
        cursor_plane: None,
        cursor_width: 64,
        cursor_height: 64,
    }
}

fn in_formats_blob(formats: &[u32], modifiers: &[(u64, u32, u64)]) -> Vec<u8> {
    let formats_offset = 24u32;
    let modifiers_offset = formats_offset + u32::try_from(formats.len() * 4).unwrap();
    let mut bytes = Vec::new();
    for value in [
        1,
        0,
        u32::try_from(formats.len()).unwrap(),
        formats_offset,
        u32::try_from(modifiers.len()).unwrap(),
        modifiers_offset,
    ] {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    for format in formats {
        bytes.extend_from_slice(&format.to_ne_bytes());
    }
    for (mask, offset, modifier) in modifiers {
        bytes.extend_from_slice(&mask.to_ne_bytes());
        bytes.extend_from_slice(&offset.to_ne_bytes());
        bytes.extend_from_slice(&0u32.to_ne_bytes());
        bytes.extend_from_slice(&modifier.to_ne_bytes());
    }
    bytes
}

#[test]
fn in_formats_blob_parses_one_format_with_one_modifier() {
    let xr24 = u32::from_le_bytes(*b"XR24");
    let parsed = parse_in_formats_blob(&in_formats_blob(&[xr24], &[(1, 0, 7)])).unwrap();

    assert_eq!(
        parsed,
        vec![DrmFormatModifierPair {
            fourcc: xr24,
            modifier: 7,
        }]
    );
}

#[test]
fn in_formats_blob_parses_shared_and_offset_modifier_masks() {
    let formats = [11, 22, 33];
    let parsed =
        parse_in_formats_blob(&in_formats_blob(&formats, &[(0b11, 0, 7), (0b11, 1, 9)])).unwrap();

    assert_eq!(
        parsed,
        vec![
            DrmFormatModifierPair {
                fourcc: 11,
                modifier: 7,
            },
            DrmFormatModifierPair {
                fourcc: 22,
                modifier: 7,
            },
            DrmFormatModifierPair {
                fourcc: 22,
                modifier: 9,
            },
            DrmFormatModifierPair {
                fourcc: 33,
                modifier: 9,
            },
        ]
    );
}

#[test]
fn in_formats_blob_rejects_malformed_offsets_counts_and_format_bits() {
    let mut bad_offset = in_formats_blob(&[11], &[(1, 0, 7)]);
    bad_offset[12..16].copy_from_slice(&u32::MAX.to_ne_bytes());
    assert!(parse_in_formats_blob(&bad_offset).is_err());

    let mut bad_count = in_formats_blob(&[11], &[(1, 0, 7)]);
    bad_count[8..12].copy_from_slice(&u32::MAX.to_ne_bytes());
    assert!(parse_in_formats_blob(&bad_count).is_err());

    let out_of_range_bit = in_formats_blob(&[11], &[(0b10, 0, 7)]);
    assert!(parse_in_formats_blob(&out_of_range_bit).is_err());
}

#[test]
fn atomic_discovery_does_not_require_an_initial_framebuffer() {
    let request = AtomicDiscoveryRequest::new(
        ConnectorId::new(1).unwrap(),
        CrtcId::new(2).unwrap(),
        u32::from_le_bytes(*b"XR24"),
    );

    assert_eq!(request.connector(), ConnectorId::new(1).unwrap());
    assert_eq!(request.crtc(), CrtcId::new(2).unwrap());
    assert_eq!(request.framebuffer_format(), u32::from_le_bytes(*b"XR24"));
}

#[test]
fn atomic_initialization_reuses_exactly_the_supplied_discovery() {
    let discovery = discovery();
    let request = initial_modeset_request_from_discovery(
        &discovery,
        BlobId::new(90).unwrap(),
        FramebufferId::new(80).unwrap(),
        AtomicPlaneGeometry::fullscreen(1920, 1080).unwrap(),
    )
    .unwrap();

    assert_eq!(
        request.serialize().objects,
        vec![
            discovery.pipeline.connector.get(),
            discovery.pipeline.crtc.get(),
            discovery.pipeline.plane.get(),
        ]
    );
}

#[test]
fn legacy_fallback_is_not_entered_after_successful_atomic_discovery() {
    assert_eq!(
        atomic_failure_action_after_discovery(KmsPolicy::Auto),
        AtomicFailureAction::Fail
    );
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

    assert_eq!(request.assignment_count(), 14);
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
fn direct_test_only_uses_primary_framebuffer_without_modeset_or_event() {
    let pipeline = explicit_fence_pipeline();
    let mut request = AtomicRequest::primary_flip(
        pipeline.plane,
        pipeline.plane_props.fb_id,
        FramebufferId::new(81).unwrap(),
    )
    .unwrap();
    request.set_test_input_fence_none(&pipeline).unwrap();
    let submission = AtomicSubmission::test_only(request);

    assert_eq!(submission.request.assignment_count(), 2);
    assert!(submission.flags.contains_test_only());
    assert!(!submission.flags.contains_allow_modeset());
    assert!(!submission.flags.contains_pageflip_event());
    assert_eq!(submission.user_data, 0);
    assert_eq!(submission.request.serialize().values[0], 81);
    assert_eq!(submission.request.serialize().values[1], u64::MAX);
}

#[test]
fn explicit_atomic_flip_serializes_fb_in_fence_and_out_fence_pointer() {
    let pipeline = explicit_fence_pipeline();
    let mut out_fence = -1i32;
    let request = AtomicRequest::primary_flip_with_fences(
        &pipeline,
        FramebufferId::new(81).unwrap(),
        17,
        Some(std::ptr::addr_of_mut!(out_fence)),
    )
    .unwrap();
    let serialized = request.serialize();

    assert_eq!(request.assignment_count(), 3);
    assert_eq!(
        serialized.objects,
        vec![pipeline.crtc.get(), pipeline.plane.get()]
    );
    assert_eq!(serialized.property_counts, vec![1, 2]);
    assert_eq!(
        serialized.properties,
        vec![
            pipeline.crtc_props.out_fence_ptr.unwrap().0.get(),
            pipeline.plane_props.fb_id.0.get(),
            pipeline.plane_props.in_fence_fd.unwrap().0.get(),
        ]
    );
    assert_eq!(serialized.values[1], 81);
    assert_eq!(serialized.values[2], 17);
    assert_eq!(
        serialized.values[0],
        std::ptr::addr_of_mut!(out_fence) as u64
    );
}

#[test]
fn initial_real_modeset_can_attach_the_render_fence_without_runtime_flip_flags() {
    let pipeline = explicit_fence_pipeline();
    let mut request = AtomicRequest::initial_modeset(
        pipeline.connector,
        pipeline.crtc,
        pipeline.plane,
        &pipeline.connector_props,
        &pipeline.crtc_props,
        &pipeline.plane_props,
        BlobId::new(90).unwrap(),
        FramebufferId::new(80).unwrap(),
        AtomicPlaneGeometry::fullscreen(1920, 1080).unwrap(),
    )
    .unwrap();

    request.set_initial_input_fence(&pipeline, 23).unwrap();
    let serialized = request.serialize();
    let fence_property = pipeline.plane_props.in_fence_fd.unwrap().0.get();
    let fence_index = serialized
        .properties
        .iter()
        .position(|property| *property == fence_property)
        .unwrap();

    assert_eq!(serialized.values[fence_index], 23);
    assert_eq!(
        AtomicCommitFlags::initial_real(),
        AtomicCommitFlags::allow_modeset()
    );
    assert!(!AtomicCommitFlags::initial_real().contains_nonblock());
    assert!(!AtomicCommitFlags::initial_real().contains_pageflip_event());
}

#[test]
fn initial_test_only_modeset_uses_explicit_no_fence_value() {
    let pipeline = explicit_fence_pipeline();
    let mut request = AtomicRequest::initial_modeset(
        pipeline.connector,
        pipeline.crtc,
        pipeline.plane,
        &pipeline.connector_props,
        &pipeline.crtc_props,
        &pipeline.plane_props,
        BlobId::new(90).unwrap(),
        FramebufferId::new(80).unwrap(),
        AtomicPlaneGeometry::fullscreen(1920, 1080).unwrap(),
    )
    .unwrap();
    request.set_test_input_fence_none(&pipeline).unwrap();
    let serialized = request.serialize();
    let fence_property = pipeline.plane_props.in_fence_fd.unwrap().0.get();
    let fence_index = serialized
        .properties
        .iter()
        .position(|property| *property == fence_property)
        .unwrap();

    assert_eq!(serialized.values[fence_index], u64::MAX);
}

#[test]
fn explicit_atomic_flip_adopts_out_fence_and_closes_input_after_success() {
    let pipeline = explicit_fence_pipeline();
    let input = pipe_read_end();
    let input_raw = input.as_raw_fd();
    let returned_out = pipe_read_end_at_least(input_raw + 1);
    let returned_out_raw = returned_out.as_raw_fd();
    assert_ne!(input_raw, returned_out_raw);
    std::mem::forget(returned_out);
    let out_property = pipeline.crtc_props.out_fence_ptr.unwrap().0.get();

    let result = submit_atomic_flip_with(
        &pipeline,
        AtomicFlipRequest {
            framebuffer: FramebufferId::new(81).unwrap(),
            token: PageFlipToken::new(55).unwrap(),
            in_fence: input,
            cursor: None,
            presentation_mode: crate::compositor::OutputPresentationMode::Vsync,
            content_type: crate::compositor::DrmContentType::Graphics,
        },
        |submission| {
            let serialized = submission.request.serialize();
            let index = serialized
                .properties
                .iter()
                .position(|property| *property == out_property)
                .unwrap();
            unsafe { *(serialized.values[index] as *mut i32) = returned_out_raw };
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(
        result.out_fence.as_ref().unwrap().as_raw_fd(),
        returned_out_raw
    );
    assert_eq!(unsafe { libc::fcntl(input_raw, libc::F_GETFD) }, -1);
    drop(result);
    assert_eq!(unsafe { libc::fcntl(returned_out_raw, libc::F_GETFD) }, -1);
}

#[test]
fn async_atomic_flip_does_not_program_in_fence_fd() {
    let pipeline = explicit_fence_pipeline();
    let input = pipe_read_end();
    let input_raw = input.as_raw_fd();
    let fence_property = pipeline.plane_props.in_fence_fd.unwrap().0.get();

    submit_atomic_flip_with(
        &pipeline,
        AtomicFlipRequest {
            framebuffer: FramebufferId::new(81).unwrap(),
            token: PageFlipToken::new(55).unwrap(),
            in_fence: input,
            cursor: None,
            presentation_mode: crate::compositor::OutputPresentationMode::Async,
            content_type: crate::compositor::DrmContentType::Graphics,
        },
        |submission| {
            let serialized = submission.request.serialize();
            assert!(!serialized.properties.contains(&fence_property));
            assert!(submission.flags.contains_pageflip_async());
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(unsafe { libc::fcntl(input_raw, libc::F_GETFD) }, -1);
}

#[test]
fn explicit_atomic_flip_closes_kernel_written_out_fence_on_ioctl_failure() {
    let pipeline = explicit_fence_pipeline();
    let input = pipe_read_end();
    let input_raw = input.as_raw_fd();
    let returned_out = pipe_read_end_at_least(input_raw + 1);
    let returned_out_raw = returned_out.as_raw_fd();
    assert_ne!(input_raw, returned_out_raw);
    std::mem::forget(returned_out);
    let out_property = pipeline.crtc_props.out_fence_ptr.unwrap().0.get();

    let error = submit_atomic_flip_with(
        &pipeline,
        AtomicFlipRequest {
            framebuffer: FramebufferId::new(81).unwrap(),
            token: PageFlipToken::new(55).unwrap(),
            in_fence: input,
            cursor: None,
            presentation_mode: crate::compositor::OutputPresentationMode::Vsync,
            content_type: crate::compositor::DrmContentType::Graphics,
        },
        |submission| {
            let serialized = submission.request.serialize();
            let index = serialized
                .properties
                .iter()
                .position(|property| *property == out_property)
                .unwrap();
            unsafe { *(serialized.values[index] as *mut i32) = returned_out_raw };
            Err(AtomicKmsError::new(
                AtomicKmsErrorKind::FlipRejected,
                "injected failure",
            ))
        },
    )
    .unwrap_err();

    assert_eq!(error.kind, AtomicKmsErrorKind::FlipRejected);
    assert_eq!(unsafe { libc::fcntl(input_raw, libc::F_GETFD) }, -1);
    assert_eq!(unsafe { libc::fcntl(returned_out_raw, libc::F_GETFD) }, -1);
}

#[test]
fn explicit_atomic_flip_ignores_negative_out_fence() {
    let pipeline = explicit_fence_pipeline();
    let result = submit_atomic_flip_with(
        &pipeline,
        AtomicFlipRequest {
            framebuffer: FramebufferId::new(81).unwrap(),
            token: PageFlipToken::new(55).unwrap(),
            in_fence: pipe_read_end(),
            cursor: None,
            presentation_mode: crate::compositor::OutputPresentationMode::Vsync,
            content_type: crate::compositor::DrmContentType::Graphics,
        },
        |_| Ok(()),
    )
    .unwrap();

    assert!(result.out_fence.is_none());
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

    assert_eq!(submission.request.assignment_count(), 14);
    assert!(submission.flags.contains_allow_modeset());
    assert!(!submission.flags.contains_nonblock());
    assert!(!submission.flags.contains_pageflip_event());
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
fn restore_and_safe_disable_requests_restore_cursor_plane() {
    let (connector, crtc, plane, connector_props, crtc_props, plane_props) = ids();
    let pipeline = AtomicPipelineProperties {
        connector,
        crtc,
        plane,
        connector_props,
        crtc_props,
        plane_props,
        cursor_plane: Some(cursor_properties()),
    };
    let snapshot = AtomicPipelineSnapshot {
        connector_crtc_id: 12,
        connector_content_type: None,
        crtc_active: 1,
        crtc_mode_id: 44,
        crtc_vrr_enabled: Some(1),
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
        cursor: Some(AtomicCursorPlaneSnapshot {
            fb_id: 77,
            crtc_id: 2,
            src_x: 0,
            src_y: 0,
            src_w: 64 << 16,
            src_h: 64 << 16,
            crtc_x: 10,
            crtc_y: 11,
            crtc_w: 64,
            crtc_h: 64,
            alpha: None,
            pixel_blend_mode: None,
        }),
    };

    let restore = snapshot.restore_request(&pipeline).unwrap();
    let vrr_property = pipeline.crtc_props.vrr_enabled.unwrap().0.get();
    let restore_values = restore
        .serialize()
        .properties
        .into_iter()
        .zip(restore.serialize().values)
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(restore_values[&vrr_property], 1);
    let mut fixed_snapshot = snapshot;
    fixed_snapshot.crtc_vrr_enabled = Some(0);
    let fixed_restore = fixed_snapshot.restore_request(&pipeline).unwrap();
    let fixed_values = fixed_restore
        .serialize()
        .properties
        .into_iter()
        .zip(fixed_restore.serialize().values)
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(fixed_values[&vrr_property], 0);
    let disable = AtomicRequest::safe_disable(&pipeline).unwrap();
    assert!(restore.touches_object_kind(DrmObjectKind::CursorPlane));
    assert!(disable.touches_object_kind(DrmObjectKind::CursorPlane));
    assert_eq!(disable.serialize().values.len(), 8);
    assert!(disable.serialize().values.iter().all(|value| *value == 0));
}

#[test]
fn visible_cursor_state_uses_hotspot_subtraction_and_normal_geometry() {
    let pipeline = pipeline_with_cursor();
    let request = AtomicRequest::cursor_only(&pipeline, Some(&visible_cursor())).unwrap();
    let serialized = request.serialize();

    assert!(request.touches_object_kind(DrmObjectKind::CursorPlane));
    assert_eq!(
        serialized.values,
        vec![99, 2, 0, 0, 64 << 16, 64 << 16, 20, 33, 64, 64]
    );
}

#[test]
fn cursor_plane_assignment_describes_the_exact_atomic_geometry() {
    let pipeline = pipeline_with_cursor();
    let assignment = cursor_plane_assignment(&pipeline, Some(&visible_cursor())).unwrap();

    assert_eq!(
        assignment,
        AtomicCursorPlaneAssignment::Enabled {
            plane_id: 4,
            framebuffer_id: 99,
            crtc_id: 2,
            src_x: 0,
            src_y: 0,
            src_w: 64 << 16,
            src_h: 64 << 16,
            crtc_x: 20,
            crtc_y: 33,
            pointer_x: 25,
            pointer_y: 40,
            plane_origin_x: 20,
            plane_origin_y: 33,
            crtc_w: 64,
            crtc_h: 64,
            hotspot_x: 5,
            hotspot_y: 7,
            width: 64,
            height: 64,
            image_generation: 3,
            rotation: None,
            alpha: None,
            pixel_blend_mode: None,
        }
    );
}

#[test]
fn cursor_plane_assignment_preserves_disable_and_unavailable_semantics() {
    assert_eq!(
        cursor_plane_assignment(&pipeline_with_cursor(), None).unwrap(),
        AtomicCursorPlaneAssignment::Disabled { plane_id: 4 }
    );
    assert_eq!(
        cursor_plane_assignment(&explicit_fence_pipeline(), None).unwrap(),
        AtomicCursorPlaneAssignment::Unavailable
    );
    assert!(cursor_plane_assignment(&explicit_fence_pipeline(), Some(&visible_cursor())).is_err());
}

#[test]
fn cursor_plane_discovers_alpha_maximum() {
    assert_eq!(cursor_properties_with_blend().alpha_maximum, Some(65_535));
}

#[test]
fn cursor_plane_discovers_premultiplied_blend_enum() {
    assert_eq!(
        cursor_properties_with_blend().pixel_blend_mode_premultiplied,
        Some(41)
    );
}

#[test]
fn visible_cursor_request_sets_alpha_maximum() {
    let pipeline = AtomicPipelineProperties {
        cursor_plane: Some(cursor_properties_with_blend()),
        ..pipeline_with_cursor()
    };
    let request = AtomicRequest::cursor_only(&pipeline, Some(&visible_cursor())).unwrap();

    assert!(request.serialize().values.contains(&65_535));
}

#[test]
fn visible_cursor_request_sets_premultiplied_blend() {
    let pipeline = AtomicPipelineProperties {
        cursor_plane: Some(cursor_properties_with_blend()),
        ..pipeline_with_cursor()
    };
    let request = AtomicRequest::cursor_only(&pipeline, Some(&visible_cursor())).unwrap();

    assert!(request.serialize().values.contains(&41));
}

#[test]
fn missing_optional_alpha_is_supported() {
    assert_eq!(cursor_properties().alpha_maximum, None);
    AtomicRequest::cursor_only(&pipeline_with_cursor(), Some(&visible_cursor())).unwrap();
}

#[test]
fn missing_optional_blend_is_supported() {
    assert_eq!(cursor_properties().pixel_blend_mode_premultiplied, None);
    AtomicRequest::cursor_only(&pipeline_with_cursor(), Some(&visible_cursor())).unwrap();
}

#[test]
fn advertised_blend_without_compatible_value_rejects_cursor_plane() {
    let set = PropertySet::new(
        DrmObjectKind::CursorPlane,
        vec![DrmProperty::with_metadata(
            PropertyId::new(1).unwrap(),
            "pixel blend mode",
            0,
            Vec::new(),
            vec![DrmPropertyEnum {
                value: 0,
                name: "Coverage".to_string(),
            }],
        )],
    )
    .unwrap();
    assert_eq!(set.premultiplied_blend_value(), Some(None));
}

#[test]
fn hidden_cursor_state_disables_both_cursor_plane_ids() {
    let pipeline = pipeline_with_cursor();
    let request = AtomicRequest::cursor_only(&pipeline, None).unwrap();
    let serialized = request.serialize();

    assert!(request.touches_object_kind(DrmObjectKind::CursorPlane));
    assert_eq!(serialized.values[0..2], [0, 0]);
}

#[test]
fn cursor_only_request_does_not_touch_primary_plane() {
    let pipeline = pipeline_with_cursor();
    let request = AtomicRequest::cursor_only(&pipeline, Some(&visible_cursor())).unwrap();

    assert!(!request.touches_object_kind(DrmObjectKind::PrimaryPlane));
    assert!(request.touches_object_kind(DrmObjectKind::CursorPlane));
}

#[test]
fn visible_cursor_geometry_preserves_negative_partially_offscreen_coordinates() {
    let pipeline = pipeline_with_cursor();
    let mut cursor = visible_cursor();
    cursor.x = 2;
    cursor.y = 3;
    cursor.hotspot_x = 7;
    cursor.hotspot_y = 9;
    let request = AtomicRequest::cursor_only(&pipeline, Some(&cursor)).unwrap();
    let assignment = cursor_plane_assignment(&pipeline, Some(&cursor)).unwrap();
    let serialized = request.serialize();

    assert_eq!(serialized.values[6], (-5i64) as u64);
    assert_eq!(serialized.values[7], (-6i64) as u64);
    assert_eq!(
        assignment,
        AtomicCursorPlaneAssignment::Enabled {
            plane_id: 4,
            framebuffer_id: 99,
            crtc_id: 2,
            src_x: 0,
            src_y: 0,
            src_w: 64 << 16,
            src_h: 64 << 16,
            crtc_x: (-5i64) as u64,
            crtc_y: (-6i64) as u64,
            pointer_x: 2,
            pointer_y: 3,
            plane_origin_x: -5,
            plane_origin_y: -6,
            crtc_w: 64,
            crtc_h: 64,
            hotspot_x: 7,
            hotspot_y: 9,
            width: 64,
            height: 64,
            image_generation: 3,
            rotation: None,
            alpha: None,
            pixel_blend_mode: None,
        }
    );
}

#[test]
fn primary_flip_with_cursor_includes_both_planes() {
    let pipeline = pipeline_with_cursor();
    let request = AtomicRequest::primary_flip_with_cursor(
        &pipeline,
        FramebufferId::new(81).unwrap(),
        Some(&visible_cursor()),
    )
    .unwrap();

    assert!(request.touches_object_kind(DrmObjectKind::PrimaryPlane));
    assert!(request.touches_object_kind(DrmObjectKind::CursorPlane));
}

#[test]
fn compatibility_atomic_primary_request_includes_visible_cursor() {
    let pipeline = pipeline_with_cursor();
    let request = AtomicRequest::primary_flip_with_cursor(
        &pipeline,
        FramebufferId::new(81).unwrap(),
        Some(&visible_cursor()),
    )
    .unwrap();

    assert_eq!(request.serialize().values[0], 81);
    assert!(request.touches_object_kind(DrmObjectKind::CursorPlane));
    assert!(request.serialize().values.contains(&99));
}

#[test]
fn compatibility_atomic_primary_request_disables_software_cursor_plane() {
    let pipeline = pipeline_with_cursor();
    let request =
        AtomicRequest::primary_flip_with_cursor(&pipeline, FramebufferId::new(81).unwrap(), None)
            .unwrap();

    assert_eq!(request.serialize().values[0], 81);
    assert_eq!(request.serialize().values[1..3], [0, 0]);
}

#[test]
fn compatibility_atomic_primary_request_disables_client_cursor_plane() {
    let pipeline = pipeline_with_cursor();
    let request =
        AtomicRequest::primary_flip_with_cursor(&pipeline, FramebufferId::new(81).unwrap(), None)
            .unwrap();

    assert!(request.touches_object_kind(DrmObjectKind::CursorPlane));
    assert!(
        request.serialize().values[1..3]
            .iter()
            .all(|value| *value == 0)
    );
}

#[test]
fn compatibility_legacy_primary_does_not_build_atomic_cursor_properties() {
    let (_, _, plane, _, _, plane_props) = ids();
    let request =
        AtomicRequest::primary_flip(plane, plane_props.fb_id, FramebufferId::new(81).unwrap())
            .unwrap();

    assert!(!request.touches_object_kind(DrmObjectKind::CursorPlane));
}

#[test]
fn direct_test_only_includes_visible_cursor_plane() {
    let pipeline = pipeline_with_cursor();
    let request = AtomicRequest::primary_flip_with_cursor(
        &pipeline,
        FramebufferId::new(82).unwrap(),
        Some(&visible_cursor()),
    )
    .unwrap();

    assert!(request.touches_object_kind(DrmObjectKind::PrimaryPlane));
    assert!(request.touches_object_kind(DrmObjectKind::CursorPlane));
}

#[test]
fn initial_modeset_with_cursor_includes_cursor_plane_state() {
    let pipeline = pipeline_with_cursor();
    let request = AtomicRequest::initial_modeset_for_pipeline(
        &pipeline,
        BlobId::new(90).unwrap(),
        FramebufferId::new(80).unwrap(),
        AtomicPlaneGeometry::fullscreen(1920, 1080).unwrap(),
        Some(&visible_cursor()),
    )
    .unwrap();

    assert!(request.touches_object_kind(DrmObjectKind::PrimaryPlane));
    assert!(request.touches_object_kind(DrmObjectKind::CursorPlane));
}
