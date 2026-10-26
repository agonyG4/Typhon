use super::*;
use crate::compositor::presentation_coverage::{
    PresentationCoverageApplicationGroup, PresentationCoverageOpacity, PresentationCoverageSurface,
};
use crate::compositor::surface::SurfaceRenderBackend;
use crate::render_backend::buffer::{DrmFormat, DrmModifier};

fn pending_fullscreen_analysis() -> DirectScanoutSceneAnalysis {
    let source = PresentationCoverageSurface {
        surface_id: 7,
        target: SurfaceTargetRect::new(0, 0, 1280, 800),
        opacity: PresentationCoverageOpacity::OpaqueRgb8888,
        backend: SurfaceRenderBackend::NativeWayland,
        buffer_source: SurfaceBufferSource::Dmabuf,
        format: Some(DrmFormat::Xrgb8888),
        modifier: Some(DrmModifier(0x0300_0000_0060_6014)),
    };
    let mut blockers = DirectScanoutSceneBlockers::default();
    blockers.push(DirectScanoutSceneRejection::PendingOrUnpublishedWork);
    DirectScanoutSceneAnalysis {
        coverage: PresentationCoverageAnalysis {
            covering_application_group: Some(PresentationCoverageApplicationGroup {
                root_surface_id: 3,
                surface_ids: vec![3, 7],
                surface_details: vec![source.clone()],
                covering_surface: Some(source),
                visible_surface_ids_above_covering: Vec::new(),
            }),
            opacity: PresentationCoverageOpacity::OpaqueRgb8888,
            visible_content_above: Vec::new(),
        },
        effects: DirectScanoutEffectAnalysis::default(),
        candidate: None,
        probe_candidate: None,
        blockers,
    }
}

#[test]
fn pending_frame_keeps_feedback_for_the_covering_source_without_admitting_scanout() {
    let analysis = pending_fullscreen_analysis();

    assert!(analysis.candidate.is_none());
    assert_eq!(analysis.feedback_surface_id(), Some(7));
}

#[test]
fn pending_work_does_not_retract_an_active_scanout_hint() {
    let mut state = CompositorState::default();
    state.activate_surface_scanout_hint(7);
    state.reconcile_surface_scanout_candidate(Some(7));

    for _ in 0..8 {
        state.reconcile_surface_scanout_candidate(
            pending_fullscreen_analysis().feedback_surface_id(),
        );
        assert!(state.dmabuf_surface_scanout_hints.contains(&7));
        state.reconcile_surface_scanout_candidate(Some(7));
    }
}

#[test]
fn actual_scene_blockers_withdraw_scanout_feedback_even_with_pending_work() {
    for blocker in [
        DirectScanoutSceneRejection::OverlayVisible,
        DirectScanoutSceneRejection::EffectRequiresComposition,
        DirectScanoutSceneRejection::BufferSizeMismatch,
        DirectScanoutSceneRejection::NonDmabuf,
        DirectScanoutSceneRejection::ResizePreviewActive,
        DirectScanoutSceneRejection::OwnerRootBufferMissing,
        DirectScanoutSceneRejection::WorkspacePresentationTransition,
    ] {
        let mut analysis = pending_fullscreen_analysis();
        analysis.blockers.push(blocker);

        assert_eq!(analysis.feedback_surface_id(), None, "blocker={blocker:?}");
    }
}

#[test]
fn missing_covering_source_does_not_receive_scanout_feedback() {
    let mut analysis = pending_fullscreen_analysis();
    analysis
        .coverage
        .covering_application_group
        .as_mut()
        .unwrap()
        .covering_surface = None;

    assert_eq!(analysis.feedback_surface_id(), None);
}

#[test]
fn pending_scene_without_a_covering_application_has_no_feedback_candidate() {
    let mut analysis = pending_fullscreen_analysis();
    analysis.coverage = PresentationCoverageAnalysis::default();

    assert_eq!(analysis.feedback_surface_id(), None);
}
