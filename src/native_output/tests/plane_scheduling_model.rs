use super::*;
use crate::native_output::presentation::plane::{
    CursorCoupling, CursorRevision, KmsCommitBundleId, PlanePageflipIdentity, PlaneWriteSet,
    PresentedCursorPromotion, PresentedCursorState, PresentedPlaneSnapshot,
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
    let cursor_only = PlaneWriteSet::CURSOR;
    assert!(cursor_only.validate_cursor_delta().is_ok());
    assert!(
        (cursor_only | PlaneWriteSet::PRIMARY)
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
