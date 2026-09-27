use super::*;
use crate::compositor::{PresentedCanonicalSceneSnapshot, PresentedSurfaceContentEvidence};

pub(super) fn prove_window_exit_source(
    promoted: Option<&PresentedCanonicalSceneSnapshot>,
    output_id: OutputId,
    root_surface_id: u32,
    current_render_generation: u64,
    current_effect_identity_signature: u64,
    current_surfaces: &[PresentedSurfaceContentEvidence],
    has_physically_presented_geometry: bool,
) -> bool {
    let Some(promoted) = promoted else {
        return false;
    };
    if !has_physically_presented_geometry
        || promoted.output_id != output_id
        || promoted.render_generation != current_render_generation
        || promoted.effect_identity_signature != current_effect_identity_signature
    {
        return false;
    }

    let expected = promoted.surfaces_for_owner(root_surface_id);
    let mut current = current_surfaces
        .iter()
        .copied()
        .filter(|surface| surface.presentation_owner_root_surface_id == root_surface_id)
        .collect::<Vec<_>>();
    if expected.is_empty() || current.is_empty() {
        return false;
    }

    current.sort_unstable_by_key(|surface| (surface.key.surface_id, surface.key.generation));
    if expected
        .windows(2)
        .any(|pair| pair[0].key.surface_id == pair[1].key.surface_id)
        || current
            .windows(2)
            .any(|pair| pair[0].key.surface_id == pair[1].key.surface_id)
    {
        return false;
    }

    expected == current
}

impl CompositorState {
    pub(in crate::compositor) fn settle_window_exit_physical(
        &mut self,
        _frame_id: u64,
        presentation: &crate::presentation_animation::PresentationFrameSnapshot,
        rendered_exits: &[crate::compositor::WindowExitFrameEvidence],
    ) {
        for evidence in rendered_exits {
            let Some(payload) = self.window_exit_payloads.get_exact(evidence.identity) else {
                continue;
            };
            let Some(revisions) = payload.property_revisions else {
                continue;
            };
            if payload.payload_id != evidence.payload_id
                || payload.content.root_surface_id != evidence.root_surface_id
                || payload.content.scene_node_id != evidence.scene_node_id
                || evidence.identity.scene_node_id() != evidence.scene_node_id
                || evidence.identity.kind()
                    != crate::presentation_animation::PresentationRetainedVisualKind::WindowExit
                || self.presentation_animator.active_retained_visual(
                    evidence.scene_node_id,
                    crate::presentation_animation::PresentationRetainedVisualKind::WindowExit,
                ) != Some(evidence.identity)
            {
                continue;
            }

            let geometry_settled = presentation.transforms.iter().any(|transform| {
                transform.scene_node_id == evidence.scene_node_id
                    && transform.root_surface_id == evidence.root_surface_id
                    && transform.transaction_id == revisions.transaction_id
                    && transform.revision_id == revisions.geometry_revision_id
                    && transform.mathematically_settled
            });
            let opacity_settled = presentation.opacities.iter().any(|opacity| {
                opacity.scene_node_id == evidence.scene_node_id
                    && opacity.root_surface_id == evidence.root_surface_id
                    && opacity.transition.is_some_and(|transition| {
                        transition.transaction_id == revisions.transaction_id
                            && transition.revision_id == revisions.opacity_revision_id
                            && transition.mathematically_settled
                    })
            });
            if !geometry_settled
                || !opacity_settled
                || self
                    .presentation_animator
                    .transaction_record(revisions.transaction_id)
                    .is_some()
                || self
                    .presentation_animator
                    .has_geometry_track(evidence.scene_node_id)
                || self
                    .presentation_animator
                    .has_opacity_track(evidence.scene_node_id)
            {
                continue;
            }
            if !self
                .presentation_animator
                .retire_active_retained_visual_exact(evidence.identity)
            {
                continue;
            }
            let Some(payload) = self.window_exit_payloads.retire_exact(evidence.identity) else {
                continue;
            };
            self.release_retired_window_exit(payload);
            self.advance_scene_render_generation_for_window_exit();
        }
    }
}
