use super::*;
use crate::native_output::presentation::plane::{
    CursorCoupling, CursorRevision, KmsCommitBundleId, PlanePageflipIdentity, PlaneWriteSet,
    PresentedCursorPromotion, PresentedCursorState, PresentedPlaneSnapshot,
};
use crate::native_output::presentation::plane_policy::{
    CursorCapabilityKey, CursorCapabilityStatus, CursorDeliveryChoice, CursorFailureDisposition,
    CursorFailureKind, CursorGeometryClass, CursorGeometryInput, CursorHardwareCapability,
    CursorPacingConstraint, CursorPlaneAction, CursorPreference, KmsCursorTestPolicy,
    PlaneCapabilityCache, PlanePrimaryMode, PlaneSchedulingInput, PlaneSchedulingReason,
    PrimaryPlaneAction, classify_cursor_failure, normalize_cursor_geometry, schedule_planes,
};
use oblivion_one::native::kms::AtomicCursorVisualState;
use std::num::NonZeroU64;

fn bundle_id(value: u64) -> KmsCommitBundleId {
    KmsCommitBundleId::new(NonZeroU64::new(value).unwrap())
}

fn token(value: u64) -> PageFlipToken {
    PageFlipToken::new(value).unwrap()
}

#[test]
fn cursor_revision_advances_only_the_changed_field() {
    let initial = CursorRevision::initial();
    let image = initial.advance_image();
    assert_ne!(image.image, initial.image);
    assert_eq!(image.motion, initial.motion);
    assert_eq!(image.visibility, initial.visibility);

    let motion = image.advance_motion();
    assert_eq!(motion.image, image.image);
    assert_ne!(motion.motion, image.motion);
    assert_eq!(motion.visibility, image.visibility);

    let visibility = motion.advance_visibility();
    assert_eq!(visibility.image, motion.image);
    assert_eq!(visibility.motion, motion.motion);
    assert_ne!(visibility.visibility, motion.visibility);
}

#[test]
fn cursor_write_set_rejects_primary_mutation() {
    let plane_delta = PlaneWriteSet::CURSOR;
    assert!(plane_delta.validate_cursor_delta().is_ok());
    assert!(
        (plane_delta | PlaneWriteSet::PRIMARY)
            .validate_cursor_delta()
            .is_err()
    );
}

#[test]
fn presented_cursor_uses_kms_equivalence_not_logical_revision_equality() {
    let first = AtomicCursorVisualState::hidden(64, 64);
    let mut moved_while_hidden = first.clone();
    moved_while_hidden.x = 200;
    moved_while_hidden.y = -50;
    let presented = PresentedCursorState::from_atomic(
        CursorRevision::initial(),
        CursorCoupling::Hidden,
        &first,
    );

    assert!(presented.kms_equivalent_to(&moved_while_hidden));
}

#[test]
fn presented_cursor_promotes_only_on_the_exact_bundle_pageflip() {
    let hidden = PresentedCursorState::from_atomic(
        CursorRevision::initial(),
        CursorCoupling::Hidden,
        &AtomicCursorVisualState::hidden(64, 64),
    );
    let mut snapshot = PresentedPlaneSnapshot::initial(hidden);
    let mut visible = AtomicCursorVisualState::hidden(64, 64);
    visible.visible = true;
    visible.framebuffer_id = Some(91);
    visible.x = 100;
    visible.y = 200;
    let expected_identity = PlanePageflipIdentity {
        bundle_id: bundle_id(7),
        token: token(8),
        output_generation: 9,
        crtc_id: 10,
    };
    let promotion = PresentedCursorPromotion {
        identity: expected_identity,
        cursor: PresentedCursorState::from_atomic(
            CursorRevision::initial().advance_motion(),
            CursorCoupling::IndependentPlane,
            &visible,
        ),
    };

    let stale = PlanePageflipIdentity {
        bundle_id: bundle_id(6),
        ..expected_identity
    };
    assert!(!snapshot.promote_cursor(&promotion, stale));
    assert!(!snapshot.cursor.visible);

    assert!(snapshot.promote_cursor(&promotion, expected_identity));
    assert!(snapshot.cursor.visible);
    assert_eq!(snapshot.cursor.framebuffer_id, Some(91));
}

fn capability_key(class: CursorGeometryClass) -> CursorCapabilityKey {
    CursorCapabilityKey {
        output_generation: 3,
        crtc_id: 4,
        plane_id: 5,
        mode_width: 1920,
        mode_height: 1080,
        output_transform: 0,
        output_scale_milli: 1_000,
        format: DRM_FORMAT_ARGB8888,
        modifier: 0,
        cursor_width: 64,
        cursor_height: 64,
        hotspot_property_available: false,
        geometry_class: class,
    }
}

fn geometry(x: i32, y: i32) -> CursorGeometryInput {
    CursorGeometryInput {
        pointer_x: x,
        pointer_y: y,
        hotspot_x: 0,
        hotspot_y: 0,
        cursor_width: 64,
        cursor_height: 64,
        output_width: 1920,
        output_height: 1080,
    }
}

#[test]
fn cursor_geometry_normalizes_center_edges_corners_and_outside() {
    let centered = normalize_cursor_geometry(geometry(100, 100)).unwrap();
    assert_eq!(centered.class, CursorGeometryClass::FullyVisible);
    assert_eq!((centered.destination.x, centered.destination.y), (100, 100));
    assert_eq!((centered.source.x, centered.source.y), (0, 0));

    let left = normalize_cursor_geometry(geometry(-10, 100)).unwrap();
    assert_eq!(left.class, CursorGeometryClass::EdgeClipped);
    assert_eq!(left.destination.x, 0);
    assert_eq!(left.destination.width, 54);
    assert_eq!(left.source.x, 10 << 16);

    let corner = normalize_cursor_geometry(geometry(-10, -20)).unwrap();
    assert_eq!(corner.class, CursorGeometryClass::CornerClipped);
    assert_eq!(
        (corner.destination.width, corner.destination.height),
        (54, 44)
    );
    assert_eq!((corner.source.x, corner.source.y), (10 << 16, 20 << 16));

    assert!(normalize_cursor_geometry(geometry(-64, 100)).is_none());
    assert!(normalize_cursor_geometry(geometry(1920, 100)).is_none());
}

#[test]
fn plane_policy_exhaustively_preserves_delivery_invariants() {
    let preferences = [
        CursorPreference::Auto,
        CursorPreference::Hardware,
        CursorPreference::Software,
    ];
    let statuses = [
        None,
        Some(CursorCapabilityStatus::Unknown),
        Some(CursorCapabilityStatus::Proven),
        Some(CursorCapabilityStatus::Quarantined {
            reason: crate::native_output::presentation::plane_policy::CursorQuarantineReason::TestOnlyRejected,
            failure_count: 1,
        }),
    ];
    let mut explored = 0;

    for preference in preferences {
        for visible in [false, true] {
            for status in statuses {
                for geometry_valid in [false, true] {
                    for primary_mode in [PlanePrimaryMode::Composed, PlanePrimaryMode::Direct] {
                        for software_allowed in [false, true] {
                            for predictive_triple_active in [false, true] {
                                let mut cache = PlaneCapabilityCache::default();
                                let key = capability_key(CursorGeometryClass::FullyVisible);
                                if let Some(status) = status {
                                    cache.set_status(key, status);
                                }
                                let decision = schedule_planes(PlaneSchedulingInput {
                                    revision: CursorRevision::initial(),
                                    preference,
                                    visible,
                                    geometry: geometry(100, 100),
                                    geometry_valid,
                                    hardware: status.map(|_| CursorHardwareCapability { key }),
                                    capabilities: &cache,
                                    primary_mode,
                                    software_allowed,
                                    predictive_triple_active,
                                    cursor_kms_changed: true,
                                    hardware_plane_visible: false,
                                    transition_primary: None,
                                });
                                explored += 1;

                                let actual_mode = match decision.delivery {
                                    CursorDeliveryChoice::Hidden { .. } => "hidden",
                                    CursorDeliveryChoice::Hardware { .. } => "hardware",
                                    CursorDeliveryChoice::Software { .. } => "software",
                                    CursorDeliveryChoice::Rejected { .. } => "rejected",
                                };
                                let hardware_usable = geometry_valid
                                    && matches!(
                                        status,
                                        Some(CursorCapabilityStatus::Unknown)
                                            | Some(CursorCapabilityStatus::Proven)
                                    );
                                let expected_mode = if !visible {
                                    "hidden"
                                } else if preference == CursorPreference::Software {
                                    if software_allowed {
                                        "software"
                                    } else {
                                        "rejected"
                                    }
                                } else if hardware_usable {
                                    "hardware"
                                } else if preference == CursorPreference::Hardware
                                    || !software_allowed
                                {
                                    "rejected"
                                } else {
                                    "software"
                                };
                                assert_eq!(
                                    actual_mode, expected_mode,
                                    "unexpected policy result for {preference:?} {visible:?} {status:?} geometry_valid={geometry_valid} primary={primary_mode:?} software_allowed={software_allowed} predictive={predictive_triple_active}"
                                );
                                if !visible {
                                    assert!(matches!(
                                        decision.delivery,
                                        CursorDeliveryChoice::Hidden { .. }
                                    ));
                                    assert!(decision.direct_scanout_compatible);
                                }
                                if matches!(
                                    decision.delivery,
                                    CursorDeliveryChoice::Software { .. }
                                ) {
                                    assert!(!decision.direct_scanout_compatible);
                                    assert_eq!(
                                        decision.pacing_constraint,
                                        CursorPacingConstraint::ReactiveDouble
                                    );
                                    assert!(software_allowed);
                                    if primary_mode == PlanePrimaryMode::Direct {
                                        assert_eq!(
                                            decision.primary_action,
                                            PrimaryPlaneAction::TransitionToComposed
                                        );
                                    }
                                }
                                assert!(!matches!(
                                    (decision.delivery, decision.direct_scanout_compatible),
                                    (CursorDeliveryChoice::Software { .. }, true)
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(explored, 384);
}

#[test]
fn proven_motion_skips_test_but_new_geometry_class_requires_it() {
    let mut cache = PlaneCapabilityCache::default();
    let centered_key = capability_key(CursorGeometryClass::FullyVisible);
    cache.mark_proven(centered_key);
    let centered = schedule_planes(PlaneSchedulingInput {
        revision: CursorRevision::initial().advance_motion(),
        preference: CursorPreference::Auto,
        visible: true,
        geometry: geometry(100, 100),
        geometry_valid: true,
        hardware: Some(CursorHardwareCapability { key: centered_key }),
        capabilities: &cache,
        primary_mode: PlanePrimaryMode::Direct,
        software_allowed: true,
        predictive_triple_active: true,
        cursor_kms_changed: true,
        hardware_plane_visible: true,
        transition_primary: None,
    });
    assert_eq!(centered.test_policy, KmsCursorTestPolicy::SkipProven);
    assert_eq!(
        centered.reason,
        PlaneSchedulingReason::HardwareCapabilityProven
    );

    let edge_key = capability_key(CursorGeometryClass::EdgeClipped);
    let edge = schedule_planes(PlaneSchedulingInput {
        revision: CursorRevision::initial().advance_motion(),
        preference: CursorPreference::Auto,
        visible: true,
        geometry: geometry(-1, 100),
        geometry_valid: true,
        hardware: Some(CursorHardwareCapability { key: edge_key }),
        capabilities: &cache,
        primary_mode: PlanePrimaryMode::Direct,
        software_allowed: true,
        predictive_triple_active: true,
        cursor_kms_changed: true,
        hardware_plane_visible: true,
        transition_primary: None,
    });
    assert_eq!(edge.test_policy, KmsCursorTestPolicy::Required);
    assert_eq!(
        edge.reason,
        PlaneSchedulingReason::HardwareCapabilityUnknown
    );
}

#[test]
fn capability_failures_are_classified_without_quarantining_busy() {
    assert_eq!(
        classify_cursor_failure(CursorFailureKind::Busy),
        CursorFailureDisposition::Defer
    );
    assert_eq!(
        classify_cursor_failure(CursorFailureKind::AdmissionContention),
        CursorFailureDisposition::Defer
    );
    assert!(matches!(
        classify_cursor_failure(CursorFailureKind::TestOnlyInvalid),
        CursorFailureDisposition::Quarantine(_)
    ));
    assert_eq!(
        classify_cursor_failure(CursorFailureKind::GenerationMismatch),
        CursorFailureDisposition::Invalidate
    );
}

#[test]
fn capability_cache_quarantines_exact_keys_and_invalidates_old_generations() {
    let mut cache = PlaneCapabilityCache::default();
    let old = capability_key(CursorGeometryClass::FullyVisible);
    let mut current = old;
    current.output_generation = 4;
    cache.mark_proven(old);
    cache.quarantine(
        current,
        crate::native_output::presentation::plane_policy::CursorQuarantineReason::UnsupportedSize,
    );
    cache.quarantine(
        current,
        crate::native_output::presentation::plane_policy::CursorQuarantineReason::UnsupportedSize,
    );

    assert_eq!(cache.status(old), CursorCapabilityStatus::Proven);
    assert!(matches!(
        cache.status(current),
        CursorCapabilityStatus::Quarantined {
            failure_count: 2,
            ..
        }
    ));
    assert_eq!(cache.invalidate_generation(4), 1);
    assert_eq!(cache.status(old), CursorCapabilityStatus::Unknown);
    assert!(matches!(
        cache.status(current),
        CursorCapabilityStatus::Quarantined { .. }
    ));
}

#[test]
fn software_direct_transition_requires_exact_primary_coupling() {
    let primary = OutputTransactionId::new(NonZeroU64::new(51).unwrap());
    let cache = PlaneCapabilityCache::default();
    let decision = schedule_planes(PlaneSchedulingInput {
        revision: CursorRevision::initial(),
        preference: CursorPreference::Software,
        visible: true,
        geometry: geometry(100, 100),
        geometry_valid: true,
        hardware: None,
        capabilities: &cache,
        primary_mode: PlanePrimaryMode::Direct,
        software_allowed: true,
        predictive_triple_active: true,
        cursor_kms_changed: true,
        hardware_plane_visible: true,
        transition_primary: Some(primary),
    });

    assert_eq!(
        decision.cursor_action,
        CursorPlaneAction::MustBundleWith(primary)
    );
    assert_eq!(
        decision.primary_action,
        PrimaryPlaneAction::TransitionToComposed
    );
}
