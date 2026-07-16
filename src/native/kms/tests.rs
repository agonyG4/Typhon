use std::{
    cell::Cell,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    rc::Rc,
    time::Instant,
};

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
    let (connector, crtc, plane, connector_props, _, _) = ids();
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

fn pipe_read_end() -> OwnedFd {
    let mut pipe = [-1; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
        0
    );
    unsafe { libc::close(pipe[1]) };
    unsafe { OwnedFd::from_raw_fd(pipe[0]) }
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
            crtc_active: 0,
            crtc_mode_id: 0,
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
        },
        framebuffer_format: u32::from_le_bytes(*b"XR24"),
        plane_possible_crtcs: 1,
        plane_formats: vec![u32::from_le_bytes(*b"XR24")],
        plane_scanout_formats: Vec::new(),
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
    let returned_out = pipe_read_end();
    let returned_out_raw = returned_out.as_raw_fd();
    std::mem::forget(returned_out);
    let out_property = pipeline.crtc_props.out_fence_ptr.unwrap().0.get();

    let result = submit_atomic_flip_with(
        &pipeline,
        AtomicFlipRequest {
            framebuffer: FramebufferId::new(81).unwrap(),
            token: PageFlipToken::new(55).unwrap(),
            in_fence: input,
            cursor: None,
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
fn explicit_atomic_flip_closes_kernel_written_out_fence_on_ioctl_failure() {
    let pipeline = explicit_fence_pipeline();
    let input = pipe_read_end();
    let input_raw = input.as_raw_fd();
    let returned_out = pipe_read_end();
    let returned_out_raw = returned_out.as_raw_fd();
    std::mem::forget(returned_out);
    let out_property = pipeline.crtc_props.out_fence_ptr.unwrap().0.get();

    let error = submit_atomic_flip_with(
        &pipeline,
        AtomicFlipRequest {
            framebuffer: FramebufferId::new(81).unwrap(),
            token: PageFlipToken::new(55).unwrap(),
            in_fence: input,
            cursor: None,
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

    assert_eq!(submission.request.assignment_count(), 13);
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
    let disable = AtomicRequest::safe_disable(&pipeline).unwrap();
    assert!(restore.touches_object_kind(DrmObjectKind::CursorPlane));
    assert!(disable.touches_object_kind(DrmObjectKind::CursorPlane));
    assert_eq!(disable.serialize().values.len(), 7);
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
    let serialized = request.serialize();

    assert_eq!(serialized.values[6], (-5i64) as u64);
    assert_eq!(serialized.values[7], (-6i64) as u64);
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
