use crate::compositor::{PopupLifecycle, RenderableSurface, SceneNodeId};
use crate::presentation_animation::{AnimationCurve, AnimationTime};
use crate::wm::{WorkspaceId, WorkspaceLocation, WorkspaceManager, WorkspaceSwitchOutcome};
use std::collections::HashMap;
use std::num::NonZeroU64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum WorkspacePresentationFallbackReason {
    FullscreenTransition,
}

impl WorkspacePresentationFallbackReason {
    const fn id(self) -> &'static str {
        match self {
            Self::FullscreenTransition => "fullscreen_transition",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkspacePresentationTransitionId(NonZeroU64);

impl WorkspacePresentationTransitionId {
    pub const fn new(value: u64) -> Self {
        Self(NonZeroU64::new(value).expect("workspace transition IDs are non-zero"))
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceLogicalClip {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspacePresentationParticipant {
    pub workspace: WorkspaceId,
    pub workspace_index: usize,
    pub offset_units: f64,
    pub translation_x: f64,
    pub translation_y: f64,
    pub logical_cell_clip: WorkspaceLogicalClip,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspacePresentationSample {
    pub transition_id: WorkspacePresentationTransitionId,
    pub sampled_at: AnimationTime,
    pub position: f64,
    pub velocity: f64,
    pub target_position: f64,
    pub target_workspace: WorkspaceId,
    pub mathematically_settled: bool,
    pub participants: Vec<WorkspacePresentationParticipant>,
    pub signature: u64,
}

#[derive(Debug, Clone)]
pub struct WorkspaceFramePresentationScene {
    pub workspace_sample: WorkspacePresentationSample,
    pub surfaces: Vec<RenderableSurface>,
    pub scene_node_ids: Vec<SceneNodeId>,
    pub presentation_owner_root_surface_ids: Vec<u32>,
    pub workspace_by_owner_root: HashMap<u32, WorkspaceId>,
    pub surface_origins: Vec<(i32, i32)>,
    pub popup_surface_ids: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::compositor) struct WorkspacePresentationTransition {
    pub id: WorkspacePresentationTransitionId,
    pub start_position: f64,
    pub target_position: f64,
    pub start_velocity: f64,
    pub started_at: AnimationTime,
    pub curve: AnimationCurve,
    pub target_workspace: WorkspaceId,
}

impl WorkspacePresentationTransition {
    pub const fn new(
        id: WorkspacePresentationTransitionId,
        start_position: f64,
        target_position: f64,
        start_velocity: f64,
        started_at: AnimationTime,
        curve: AnimationCurve,
        target_workspace: WorkspaceId,
    ) -> Self {
        Self {
            id,
            start_position,
            target_position,
            start_velocity,
            started_at,
            curve,
            target_workspace,
        }
    }

    pub fn retarget(
        self,
        id: WorkspacePresentationTransitionId,
        target_workspace: WorkspaceId,
        target_position: f64,
        started_at: AnimationTime,
        curve: AnimationCurve,
    ) -> Self {
        let (start_position, start_velocity, _) = self.sample_motion(started_at);
        Self::new(
            id,
            start_position,
            target_position,
            start_velocity,
            started_at,
            curve,
            target_workspace,
        )
    }

    pub fn sample_at(
        self,
        workspaces: &WorkspaceManager,
        sampled_at: AnimationTime,
        logical_output_width: f64,
        logical_output_height: f64,
    ) -> WorkspacePresentationSample {
        let (position, velocity, mathematically_settled) = self.sample_motion(sampled_at);
        let mut participants = Vec::with_capacity(2);
        if logical_output_width.is_finite()
            && logical_output_height.is_finite()
            && logical_output_width > 0.0
            && logical_output_height > 0.0
        {
            for workspace in workspaces.workspaces() {
                let Some(workspace_index) = workspaces.workspace_axis_position(workspace) else {
                    continue;
                };
                let offset_units = workspace_index - position;
                let cell_right = offset_units + 1.0;
                if cell_right <= 0.0 || offset_units >= 1.0 {
                    continue;
                }
                let visible_left = offset_units.max(0.0);
                let visible_right = cell_right.min(1.0);
                participants.push(WorkspacePresentationParticipant {
                    workspace,
                    workspace_index: workspace_index as usize,
                    offset_units,
                    translation_x: offset_units * logical_output_width,
                    translation_y: 0.0,
                    logical_cell_clip: WorkspaceLogicalClip {
                        x: visible_left * logical_output_width,
                        y: 0.0,
                        width: (visible_right - visible_left) * logical_output_width,
                        height: logical_output_height,
                    },
                });
            }
        }
        debug_assert!(participants.len() <= 2);
        let signature = workspace_sample_signature(
            self.id,
            position,
            self.target_position,
            self.target_workspace,
            &participants,
        );
        WorkspacePresentationSample {
            transition_id: self.id,
            sampled_at,
            position,
            velocity,
            target_position: self.target_position,
            target_workspace: self.target_workspace,
            mathematically_settled,
            participants,
            signature,
        }
    }

    fn sample_motion(self, at: AnimationTime) -> (f64, f64, bool) {
        let elapsed = at.elapsed_seconds(self.started_at);
        let (position, velocity, settled) = self.curve.sample_scalar(
            self.start_position,
            self.target_position,
            self.start_velocity,
            elapsed,
            true,
        );
        let crossed_target = if self.target_position > self.start_position {
            position >= self.target_position
        } else if self.target_position < self.start_position {
            position <= self.target_position
        } else {
            true
        };
        if settled || crossed_target {
            (self.target_position, 0.0, true)
        } else {
            (position, velocity, false)
        }
    }
}

#[derive(Debug, Clone)]
pub(in crate::compositor) struct WorkspacePresentationState {
    transition: Option<WorkspacePresentationTransition>,
    next_transition_id: u64,
}

impl Default for WorkspacePresentationState {
    fn default() -> Self {
        Self {
            transition: None,
            next_transition_id: 1,
        }
    }
}

impl WorkspacePresentationState {
    pub fn activate_for_switch(
        &mut self,
        outcome: WorkspaceSwitchOutcome,
        workspaces: &WorkspaceManager,
        started_at: AnimationTime,
        curve: AnimationCurve,
    ) -> Option<WorkspacePresentationTransitionId> {
        let WorkspaceSwitchOutcome::Changed { previous, current } = outcome else {
            return None;
        };
        let start_position = workspaces.workspace_axis_position(previous)?;
        let target_position = workspaces.workspace_axis_position(current)?;
        let id = WorkspacePresentationTransitionId::new(self.next_transition_id);
        let transition = match self.transition {
            Some(transition) => {
                transition.retarget(id, current, target_position, started_at, curve)
            }
            None => WorkspacePresentationTransition::new(
                id,
                start_position,
                target_position,
                0.0,
                started_at,
                curve,
                current,
            ),
        };
        self.transition = Some(transition);
        self.next_transition_id = self.next_transition_id.saturating_add(1).max(1);
        Some(id)
    }

    pub const fn transition(&self) -> Option<&WorkspacePresentationTransition> {
        self.transition.as_ref()
    }

    #[cfg(test)]
    pub const fn next_transition_id(&self) -> u64 {
        self.next_transition_id
    }

    pub fn sample_at(
        &self,
        workspaces: &WorkspaceManager,
        sampled_at: AnimationTime,
        logical_output_width: f64,
        logical_output_height: f64,
    ) -> Option<WorkspacePresentationSample> {
        self.transition.map(|transition| {
            transition.sample_at(
                workspaces,
                sampled_at,
                logical_output_width,
                logical_output_height,
            )
        })
    }

    pub fn path_contains_workspace(
        &self,
        workspaces: &WorkspaceManager,
        workspace: WorkspaceId,
    ) -> bool {
        let Some(transition) = self.transition else {
            return false;
        };
        let Some(index) = workspaces.workspace_axis_position(workspace) else {
            return false;
        };
        let lower = transition.start_position.min(transition.target_position);
        let upper = transition.start_position.max(transition.target_position);
        index > lower - 1.0 && index < upper + 1.0
    }

    pub fn retire_if_rendered(&mut self, sample: &WorkspacePresentationSample) -> bool {
        let matching_settled_sample = self.transition.is_some_and(|transition| {
            transition.id == sample.transition_id && sample.mathematically_settled
        });
        if matching_settled_sample {
            self.transition = None;
        }
        matching_settled_sample
    }

    pub fn clear(&mut self) -> bool {
        self.transition.take().is_some()
    }
}

impl crate::compositor::CompositorState {
    pub(in crate::compositor) fn settle_workspace_presentation_for_incompatible_transition(
        &mut self,
        reason: WorkspacePresentationFallbackReason,
    ) -> bool {
        if !self.workspace_presentation.clear() {
            return false;
        }
        self.refresh_frame_work_visibility();
        eprintln!(
            "typhon.workspace_slide event=settled reason={}",
            reason.id()
        );
        true
    }

    pub(in crate::compositor) fn retire_workspace_presentation_if_rendered(
        &mut self,
        sample: &WorkspacePresentationSample,
    ) -> bool {
        let retired = self.workspace_presentation.retire_if_rendered(sample);
        if retired {
            self.refresh_frame_work_visibility();
        }
        retired
    }

    pub(in crate::compositor) fn workspace_frame_presentation_scene_at(
        &self,
        sampled_at: AnimationTime,
    ) -> Option<WorkspaceFramePresentationScene> {
        let workspace_sample = self.workspace_presentation.sample_at(
            &self.workspace_manager,
            sampled_at,
            f64::from(self.output_size.width),
            f64::from(self.output_size.height),
        )?;
        let mut surfaces = self
            .renderable_surfaces
            .iter()
            .enumerate()
            .filter(|(_, surface)| {
                self.surface_is_visible_in_workspace_frame(surface.surface_id, &workspace_sample)
            })
            .map(|(position, surface)| (position, surface.clone()))
            .collect::<Vec<_>>();
        surfaces.sort_by_key(|(position, surface)| {
            self.renderable_root_stack_key(
                self.root_surface_id_for_surface(surface.surface_id),
                *position,
            )
        });
        let surfaces = surfaces
            .into_iter()
            .map(|(_, surface)| surface)
            .collect::<Vec<_>>();
        let scene_node_ids = surfaces
            .iter()
            .map(|surface| {
                self.scene_node_id_for_surface(surface.surface_id)
                    .expect("workspace presentation surface has no canonical scene node")
            })
            .collect::<Vec<_>>();
        let presentation_owner_root_surface_ids = surfaces
            .iter()
            .map(|surface| self.presentation_owner_root_for_surface(surface.surface_id))
            .collect::<Vec<_>>();
        let workspace_by_owner_root = presentation_owner_root_surface_ids
            .iter()
            .copied()
            .filter_map(|owner_root| {
                self.regular_workspace_for_presentation_owner(owner_root)
                    .map(|workspace| (owner_root, workspace))
            })
            .collect();
        let surface_origins = crate::compositor::render::surface_origins(&surfaces);
        let mut popup_surface_ids = self
            .popup_surfaces
            .keys()
            .copied()
            .filter(|surface_id| {
                self.popup_nodes.get(surface_id).is_some_and(|node| {
                    node.lifecycle == PopupLifecycle::Alive
                        && node.mapped
                        && self
                            .surface_is_visible_in_workspace_frame(*surface_id, &workspace_sample)
                })
            })
            .collect::<Vec<_>>();
        popup_surface_ids.sort_unstable();

        Some(WorkspaceFramePresentationScene {
            workspace_sample,
            surfaces,
            scene_node_ids,
            presentation_owner_root_surface_ids,
            workspace_by_owner_root,
            surface_origins,
            popup_surface_ids,
        })
    }

    fn regular_workspace_for_presentation_owner(&self, owner_root: u32) -> Option<WorkspaceId> {
        let window_id = self.window_id_for_surface(owner_root)?;
        let owner_id = self
            .workspace_owner_window_id(window_id)
            .unwrap_or(window_id);
        let window = self.window(owner_id)?;
        match window.management.map(|management| management.location())? {
            WorkspaceLocation::Regular(workspace) => Some(workspace),
            WorkspaceLocation::Special(_) => None,
        }
    }

    fn surface_is_visible_in_workspace_frame(
        &self,
        surface_id: u32,
        sample: &WorkspacePresentationSample,
    ) -> bool {
        let presentation_owner = self.presentation_owner_root_for_surface(surface_id);
        if let Some(window_id) = self.window_id_for_surface(presentation_owner) {
            let owner_id = self
                .workspace_owner_window_id(window_id)
                .unwrap_or(window_id);
            if let Some(window) = self.window(owner_id) {
                if window.state.is_minimized() {
                    return false;
                }
                if let Some(WorkspaceLocation::Regular(workspace)) =
                    window.management.map(|management| management.location())
                {
                    return sample
                        .participants
                        .iter()
                        .any(|participant| participant.workspace == workspace);
                }
            }
        }
        self.surface_is_visible_in_active_scene(surface_id)
    }
}

fn workspace_sample_signature(
    transition_id: WorkspacePresentationTransitionId,
    position: f64,
    target_position: f64,
    target_workspace: WorkspaceId,
    participants: &[WorkspacePresentationParticipant],
) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut signature = OFFSET;
    let mut mix = |value: u64| {
        signature ^= value;
        signature = signature.wrapping_mul(PRIME);
    };
    mix(transition_id.get());
    mix(position.to_bits());
    mix(target_position.to_bits());
    mix(u64::from(target_workspace.raw()));
    for participant in participants {
        mix(u64::from(participant.workspace.raw()));
        mix(participant.workspace_index as u64);
        mix(participant.offset_units.to_bits());
        mix(participant.translation_x.to_bits());
        mix(participant.translation_y.to_bits());
        mix(participant.logical_cell_clip.x.to_bits());
        mix(participant.logical_cell_clip.y.to_bits());
        mix(participant.logical_cell_clip.width.to_bits());
        mix(participant.logical_cell_clip.height.to_bits());
    }
    signature
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation_animation::{AnimationTime, SpringSpec};
    use crate::wm::{WorkspaceId, WorkspaceManager, WorkspaceSwitchOutcome};

    fn slide_curve() -> AnimationCurve {
        AnimationCurve::spring(SpringSpec::new(300.0, 38.1051).with_settlement(0.0001, 0.002))
    }

    fn workspace(value: u32) -> WorkspaceId {
        WorkspaceId::new(value).unwrap()
    }

    fn enable_workspace_slide(compositor: &mut crate::compositor::CompositorState) {
        let mut configuration = crate::animation_control::AnimationConfiguration {
            enabled: true,
            ..crate::animation_control::AnimationConfiguration::default()
        };
        configuration.overrides.insert(
            crate::animation_control::AnimationSlot::WorkspaceSwitch,
            crate::animation_control::AnimationEffect::WorkspaceSlide,
        );
        compositor
            .set_animation_configuration(configuration)
            .expect("enable workspace slide");
    }

    fn fixed_transition(
        position: f64,
        target_workspace: WorkspaceId,
    ) -> WorkspacePresentationTransition {
        WorkspacePresentationTransition::new(
            WorkspacePresentationTransitionId::new(1),
            position,
            position,
            0.0,
            AnimationTime::from_nanos(0),
            slide_curve(),
            target_workspace,
        )
    }

    fn sample(position: f64, target_workspace: WorkspaceId) -> WorkspacePresentationSample {
        let manager = WorkspaceManager::new(8).unwrap();
        fixed_transition(position, target_workspace).sample_at(
            &manager,
            AnimationTime::from_nanos(0),
            1920.0,
            1080.0,
        )
    }

    #[test]
    fn adjacent_sample_translates_each_workspace_and_clips_its_cell() {
        let sample = sample(0.25, workspace(2));

        assert_eq!(sample.position, 0.25);
        assert_eq!(sample.participants.len(), 2);
        assert_eq!(sample.participants[0].workspace, workspace(1));
        assert_eq!(sample.participants[0].translation_x, -480.0);
        assert_eq!(sample.participants[0].logical_cell_clip.x, 0.0);
        assert_eq!(sample.participants[0].logical_cell_clip.width, 1440.0);
        assert_eq!(sample.participants[1].workspace, workspace(2));
        assert_eq!(sample.participants[1].translation_x, 1440.0);
        assert_eq!(sample.participants[1].logical_cell_clip.x, 1440.0);
        assert_eq!(sample.participants[1].logical_cell_clip.width, 480.0);
    }

    #[test]
    fn reverse_sample_moves_the_outgoing_workspace_right() {
        let sample = sample(0.75, workspace(1));

        assert_eq!(sample.participants.len(), 2);
        assert_eq!(sample.participants[0].workspace, workspace(1));
        assert_eq!(sample.participants[0].translation_x, -1440.0);
        assert_eq!(sample.participants[1].workspace, workspace(2));
        assert_eq!(sample.participants[1].translation_x, 480.0);
    }

    #[test]
    fn exact_endpoint_contains_only_the_target_workspace() {
        let sample = sample(1.0, workspace(2));

        assert!(sample.mathematically_settled);
        assert_eq!(sample.participants.len(), 1);
        assert_eq!(sample.participants[0].workspace, workspace(2));
        assert_eq!(sample.participants[0].translation_x, 0.0);
        assert_eq!(sample.participants[0].logical_cell_clip.x, 0.0);
        assert_eq!(sample.participants[0].logical_cell_clip.width, 1920.0);
    }

    #[test]
    fn non_adjacent_path_exposes_only_the_adjacent_pair() {
        for (position, expected) in [
            (0.25, [workspace(1), workspace(2)]),
            (1.25, [workspace(2), workspace(3)]),
            (2.25, [workspace(3), workspace(4)]),
        ] {
            let sample = sample(position, workspace(4));
            let participants = sample
                .participants
                .iter()
                .map(|participant| participant.workspace)
                .collect::<Vec<_>>();

            assert_eq!(participants, expected);
            assert!(sample.participants.len() <= 2);
        }
    }

    #[test]
    fn frame_work_path_covers_intermediate_workspaces_but_not_off_path_workspaces() {
        let mut manager = WorkspaceManager::new(8).unwrap();
        let mut presentation = WorkspacePresentationState::default();
        let outcome = manager.activate(workspace(4));
        presentation.activate_for_switch(
            outcome,
            &manager,
            AnimationTime::from_nanos(0),
            slide_curve(),
        );

        for workspace_id in 1..=4 {
            assert!(presentation.path_contains_workspace(&manager, workspace(workspace_id)));
        }
        assert!(!presentation.path_contains_workspace(&manager, workspace(5)));
        assert!(!presentation.path_contains_workspace(&manager, workspace(8)));
    }

    #[test]
    fn frame_work_visibility_keeps_only_the_active_workspace_and_transition_path() {
        let mut compositor = crate::compositor::CompositorState::default();
        let outcome = compositor.workspace_manager.activate(workspace(4));
        compositor.workspace_presentation.activate_for_switch(
            outcome,
            &compositor.workspace_manager,
            AnimationTime::from_nanos(0),
            slide_curve(),
        );

        assert!(compositor.regular_workspace_is_visible_for_frame_work(workspace(1)));
        assert!(compositor.regular_workspace_is_visible_for_frame_work(workspace(3)));
        assert!(compositor.regular_workspace_is_visible_for_frame_work(workspace(4)));
        assert!(!compositor.regular_workspace_is_visible_for_frame_work(workspace(5)));
    }

    #[test]
    fn stationary_shell_surfaces_and_special_workspace_stay_outside_regular_slide_domain() {
        let mut compositor = crate::compositor::CompositorState::default();
        enable_workspace_slide(&mut compositor);
        let target = workspace(2);
        let outgoing_window = crate::compositor::WindowId::from_raw(811).unwrap();
        let target_window = crate::compositor::WindowId::from_raw(812).unwrap();
        let special_window = crate::compositor::WindowId::from_raw(813).unwrap();
        compositor.install_native_frame_test_scene(
            vec![
                super::super::desktop_window_tests::x11_shm_surface(
                    811,
                    320,
                    200,
                    crate::compositor::SurfacePlacement::absolute_root_at(20, 30),
                ),
                super::super::desktop_window_tests::x11_shm_surface(
                    812,
                    320,
                    200,
                    crate::compositor::SurfacePlacement::absolute_root_at(40, 50),
                ),
                super::super::desktop_window_tests::x11_shm_surface(
                    813,
                    320,
                    200,
                    crate::compositor::SurfacePlacement::absolute_root_at(70, 80),
                ),
                super::super::desktop_window_tests::x11_shm_surface(
                    814,
                    320,
                    200,
                    crate::compositor::SurfacePlacement::absolute_root_at(0, 0),
                ),
                super::super::desktop_window_tests::x11_shm_surface(
                    815,
                    320,
                    32,
                    crate::compositor::SurfacePlacement::absolute_root_at(0, 0),
                ),
            ],
            &[
                (811, outgoing_window),
                (812, target_window),
                (813, special_window),
            ],
            None,
        );
        compositor
            .desktop_windows
            .get_mut(&outgoing_window)
            .unwrap()
            .management = Some(crate::wm::WindowManagementState::new(
            WorkspaceLocation::Regular(workspace(1)),
        ));
        compositor
            .desktop_windows
            .get_mut(&target_window)
            .unwrap()
            .management = Some(crate::wm::WindowManagementState::new(
            WorkspaceLocation::Regular(target),
        ));
        let special_id = crate::wm::SpecialWorkspaceId::DEFAULT;
        compositor
            .desktop_windows
            .get_mut(&special_window)
            .unwrap()
            .management = Some(crate::wm::WindowManagementState::new(
            WorkspaceLocation::Special(special_id),
        ));
        compositor
            .workspace_manager
            .toggle_special_workspace(special_id);
        compositor.rebuild_active_scene_view();
        assert!(matches!(
            compositor.activate_workspace(target),
            WorkspaceSwitchOutcome::Changed { current, .. } if current == target
        ));

        let transition = compositor.workspace_presentation.transition().unwrap();
        let sampled_at =
            AnimationTime::from_nanos(transition.started_at.as_nanos().saturating_add(50_000_000));
        let frame_scene = compositor
            .workspace_frame_presentation_scene_at(sampled_at)
            .expect("active workspace slide creates a frame scene");
        let special_root = compositor.presentation_owner_root_for_surface(813);
        let target_root = compositor.presentation_owner_root_for_surface(812);

        let target_participant = frame_scene
            .workspace_sample
            .participants
            .iter()
            .find(|participant| participant.workspace == target)
            .expect("target workspace participates");
        assert_ne!(target_participant.translation_x, 0.0);
        assert_eq!(
            frame_scene.workspace_by_owner_root.get(&target_root),
            Some(&target)
        );
        assert!(
            !frame_scene
                .workspace_by_owner_root
                .contains_key(&special_root)
        );
        assert!(compositor.window_is_visible_in_active_scene(special_window));
        for stationary_id in [813, 814, 815] {
            assert_eq!(
                frame_scene
                    .surfaces
                    .iter()
                    .filter(|surface| surface.surface_id == stationary_id)
                    .count(),
                1,
                "stationary surface {stationary_id} is included exactly once"
            );
        }
        let special_surface = frame_scene
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == 813)
            .unwrap();
        assert_eq!((special_surface.x, special_surface.y), (70, 80));
        let background = frame_scene
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == 814)
            .unwrap();
        assert_eq!((background.x, background.y), (0, 0));
        let panel = frame_scene
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == 815)
            .unwrap();
        assert_eq!((panel.x, panel.y), (0, 0));
    }

    #[test]
    fn frame_presentation_includes_outgoing_workspace_without_changing_semantic_scene() {
        let mut compositor = crate::compositor::CompositorState::default();
        enable_workspace_slide(&mut compositor);
        let target = workspace(2);
        let outgoing_window = crate::compositor::WindowId::from_raw(801).unwrap();
        let target_window = crate::compositor::WindowId::from_raw(802).unwrap();
        compositor.install_native_frame_test_scene(
            vec![
                super::super::desktop_window_tests::x11_shm_surface(
                    801,
                    320,
                    200,
                    crate::compositor::SurfacePlacement::absolute_root_at(20, 30),
                ),
                super::super::desktop_window_tests::x11_shm_surface(
                    802,
                    320,
                    200,
                    crate::compositor::SurfacePlacement::absolute_root_at(40, 50),
                ),
            ],
            &[(801, outgoing_window), (802, target_window)],
            None,
        );
        compositor
            .desktop_windows
            .get_mut(&outgoing_window)
            .expect("outgoing test window")
            .management = Some(crate::wm::WindowManagementState::new(
            WorkspaceLocation::Regular(workspace(1)),
        ));
        compositor
            .desktop_windows
            .get_mut(&target_window)
            .expect("target test window")
            .management = Some(crate::wm::WindowManagementState::new(
            WorkspaceLocation::Regular(target),
        ));
        compositor.rebuild_active_scene_view();

        assert!(matches!(
            compositor.activate_workspace(target),
            WorkspaceSwitchOutcome::Changed { current, .. } if current == target
        ));
        let transition = compositor
            .workspace_presentation
            .transition()
            .expect("workspace slide starts");
        let sampled_at =
            AnimationTime::from_nanos(transition.started_at.as_nanos().saturating_add(50_000_000));
        let frame_scene = compositor
            .workspace_frame_presentation_scene_at(sampled_at)
            .expect("active workspace slide has a frame presentation scene");

        assert_eq!(compositor.active_workspace(), target);
        assert!(!compositor.window_is_visible_in_active_scene(outgoing_window));
        assert!(
            !compositor
                .active_scene_surfaces()
                .iter()
                .any(|surface| surface.surface_id == 801)
        );
        assert!(
            frame_scene
                .surfaces
                .iter()
                .any(|surface| surface.surface_id == 801)
        );
        assert!(
            frame_scene
                .surfaces
                .iter()
                .any(|surface| surface.surface_id == 802)
        );
        let outgoing_root = compositor.presentation_owner_root_for_surface(801);
        let target_root = compositor.presentation_owner_root_for_surface(802);
        assert_eq!(
            frame_scene.workspace_by_owner_root.get(&outgoing_root),
            Some(&workspace(1))
        );
        assert_eq!(
            frame_scene.workspace_by_owner_root.get(&target_root),
            Some(&target)
        );
    }

    #[test]
    fn retarget_preserves_sampled_position_and_velocity() {
        let manager = WorkspaceManager::new(4).unwrap();
        let transition = WorkspacePresentationTransition::new(
            WorkspacePresentationTransitionId::new(1),
            0.0,
            1.0,
            0.0,
            AnimationTime::from_nanos(0),
            slide_curve(),
            workspace(2),
        );
        let at = AnimationTime::from_nanos(25_000_000);
        let sampled = transition.sample_at(&manager, at, 1920.0, 1080.0);
        let retargeted = transition.retarget(
            WorkspacePresentationTransitionId::new(2),
            workspace(3),
            2.0,
            at,
            slide_curve(),
        );

        assert_eq!(retargeted.start_position, sampled.position);
        assert_eq!(retargeted.start_velocity, sampled.velocity);
        assert_eq!(retargeted.target_position, 2.0);
    }

    #[test]
    fn reversal_retarget_preserves_sampled_position_and_velocity() {
        let manager = WorkspaceManager::new(4).unwrap();
        let transition = WorkspacePresentationTransition::new(
            WorkspacePresentationTransitionId::new(1),
            0.0,
            1.0,
            0.0,
            AnimationTime::from_nanos(0),
            slide_curve(),
            workspace(2),
        );
        let at = AnimationTime::from_nanos(25_000_000);
        let sampled = transition.sample_at(&manager, at, 1920.0, 1080.0);
        let reversed = transition.retarget(
            WorkspacePresentationTransitionId::new(2),
            workspace(1),
            0.0,
            at,
            slide_curve(),
        );

        assert_eq!(reversed.start_position, sampled.position);
        assert_eq!(reversed.start_velocity, sampled.velocity);
    }

    #[test]
    fn high_velocity_target_crossing_settles_without_overshoot() {
        let transition = WorkspacePresentationTransition::new(
            WorkspacePresentationTransitionId::new(1),
            0.6,
            1.0,
            1000.0,
            AnimationTime::from_nanos(0),
            slide_curve(),
            workspace(2),
        );
        let manager = WorkspaceManager::new(4).unwrap();
        let sample = transition.sample_at(
            &manager,
            AnimationTime::from_nanos(1_000_000),
            1920.0,
            1080.0,
        );

        assert_eq!(sample.position, 1.0);
        assert_eq!(sample.velocity, 0.0);
        assert!(sample.mathematically_settled);
    }

    #[test]
    fn presentation_signature_changes_with_position() {
        let first = sample(0.25, workspace(2));
        let second = sample(0.5, workspace(2));

        assert_ne!(first.signature, second.signature);
    }

    #[test]
    fn no_change_does_not_allocate_or_retarget_a_transition() {
        let mut manager = WorkspaceManager::new(4).unwrap();
        let mut presentation = WorkspacePresentationState::default();
        let first_outcome = manager.activate(workspace(2));
        presentation.activate_for_switch(
            first_outcome,
            &manager,
            AnimationTime::from_nanos(0),
            slide_curve(),
        );
        let previous_transition = presentation.transition().unwrap().id;
        let next_transition_id = presentation.next_transition_id();
        let outcome = manager.activate(manager.active_workspace());

        assert_eq!(outcome, WorkspaceSwitchOutcome::NoChange);
        assert_eq!(
            presentation.activate_for_switch(
                outcome,
                &manager,
                AnimationTime::from_nanos(10),
                slide_curve(),
            ),
            None
        );
        assert_eq!(presentation.next_transition_id(), next_transition_id);
        assert_eq!(presentation.transition().unwrap().id, previous_transition);
    }

    #[test]
    fn compositor_workspace_activation_keeps_the_target_semantic_and_starts_slide() {
        let mut compositor = crate::compositor::CompositorState::default();
        enable_workspace_slide(&mut compositor);
        let target = workspace(2);

        let outcome = compositor.activate_workspace(target);

        assert_eq!(
            outcome,
            WorkspaceSwitchOutcome::Changed {
                previous: workspace(1),
                current: target,
            }
        );
        assert_eq!(compositor.active_workspace(), target);
        assert!(compositor.workspace_presentation.transition().is_some());
    }

    #[test]
    fn matching_rendered_settled_sample_retires_but_stale_sample_does_not() {
        let mut manager = WorkspaceManager::new(4).unwrap();
        let mut presentation = WorkspacePresentationState::default();
        let start = manager.active_workspace();
        let first_target = workspace(2);
        let first_outcome = manager.activate(first_target);
        presentation.activate_for_switch(
            first_outcome,
            &manager,
            AnimationTime::from_nanos(0),
            slide_curve(),
        );
        let stale_settled_sample = presentation
            .sample_at(
                &manager,
                AnimationTime::from_nanos(10_000_000_000),
                1920.0,
                1080.0,
            )
            .unwrap();

        let second_target = workspace(3);
        let second_outcome = manager.activate(second_target);
        presentation.activate_for_switch(
            second_outcome,
            &manager,
            AnimationTime::from_nanos(20_000_000),
            slide_curve(),
        );
        let current_id = presentation.transition().unwrap().id;

        assert_ne!(current_id, stale_settled_sample.transition_id);
        assert!(!presentation.retire_if_rendered(&stale_settled_sample));
        assert_eq!(presentation.transition().unwrap().id, current_id);

        let settled_sample = presentation
            .sample_at(
                &manager,
                AnimationTime::from_nanos(10_000_000_000),
                1920.0,
                1080.0,
            )
            .unwrap();
        assert!(settled_sample.mathematically_settled);
        assert!(presentation.retire_if_rendered(&settled_sample));
        assert!(presentation.transition().is_none());
        assert_eq!(start, workspace(1));
    }

    #[test]
    fn settled_sample_from_fullscreen_retired_transition_cannot_retire_later_slide() {
        let mut manager = WorkspaceManager::new(4).unwrap();
        let mut presentation = WorkspacePresentationState::default();
        let first_outcome = manager.activate(workspace(2));
        presentation.activate_for_switch(
            first_outcome,
            &manager,
            AnimationTime::from_nanos(0),
            slide_curve(),
        );
        let old_settled_sample = presentation
            .sample_at(
                &manager,
                AnimationTime::from_nanos(10_000_000_000),
                1920.0,
                1080.0,
            )
            .unwrap();
        assert!(presentation.clear());

        let later_outcome = manager.activate(workspace(3));
        presentation.activate_for_switch(
            later_outcome,
            &manager,
            AnimationTime::from_nanos(20_000_000),
            slide_curve(),
        );
        let later_id = presentation.transition().unwrap().id;

        assert!(!presentation.retire_if_rendered(&old_settled_sample));
        assert_eq!(presentation.transition().unwrap().id, later_id);
    }

    #[test]
    fn rendered_settled_sample_retires_and_refreshes_frame_work_visibility() {
        let mut compositor = crate::compositor::CompositorState::default();
        let target = workspace(2);
        let outcome = compositor.workspace_manager.activate(target);
        compositor.workspace_presentation.activate_for_switch(
            outcome,
            &compositor.workspace_manager,
            AnimationTime::from_nanos(0),
            slide_curve(),
        );
        let transition = compositor.workspace_presentation.transition().unwrap();
        let settled_sample = transition.sample_at(
            &compositor.workspace_manager,
            AnimationTime::from_nanos(10_000_000_000),
            1920.0,
            1080.0,
        );

        assert!(settled_sample.mathematically_settled);
        assert!(compositor.regular_workspace_is_visible_for_frame_work(workspace(1)));
        assert!(compositor.retire_workspace_presentation_if_rendered(&settled_sample));
        assert!(!compositor.regular_workspace_is_visible_for_frame_work(workspace(1)));
        assert!(compositor.regular_workspace_is_visible_for_frame_work(target));
    }

    #[test]
    fn disabling_global_animations_settles_workspace_slide_immediately() {
        let mut compositor = crate::compositor::CompositorState::default();
        let target = workspace(2);
        let outcome = compositor.workspace_manager.activate(target);
        compositor.workspace_presentation.activate_for_switch(
            outcome,
            &compositor.workspace_manager,
            AnimationTime::from_nanos(0),
            slide_curve(),
        );
        assert!(compositor.workspace_presentation.transition().is_some());
        assert!(compositor.regular_workspace_is_visible_for_frame_work(workspace(1)));

        let configuration = crate::animation_control::AnimationConfiguration {
            enabled: false,
            ..crate::animation_control::AnimationConfiguration::default()
        };
        compositor
            .set_animation_configuration(configuration)
            .expect("disable global animations");

        assert_eq!(compositor.active_workspace(), target);
        assert!(compositor.workspace_presentation.transition().is_none());
        assert!(!compositor.regular_workspace_is_visible_for_frame_work(workspace(1)));
        assert!(compositor.regular_workspace_is_visible_for_frame_work(target));
    }

    #[test]
    fn output_geometry_change_settles_workspace_slide_and_clears_frame_work() {
        let mut compositor = crate::compositor::CompositorState::default();
        let target = workspace(2);
        let outcome = compositor.workspace_manager.activate(target);
        compositor.workspace_presentation.activate_for_switch(
            outcome,
            &compositor.workspace_manager,
            AnimationTime::from_nanos(0),
            slide_curve(),
        );
        assert!(compositor.workspace_presentation.transition().is_some());
        assert!(compositor.regular_workspace_is_visible_for_frame_work(workspace(1)));

        let output_size = compositor.output_size;
        assert!(compositor.set_output_size(output_size.width + 1, output_size.height));

        assert!(compositor.workspace_presentation.transition().is_none());
        assert!(!compositor.regular_workspace_is_visible_for_frame_work(workspace(1)));
        assert!(compositor.regular_workspace_is_visible_for_frame_work(target));
    }
}
