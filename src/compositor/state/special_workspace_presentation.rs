use crate::compositor::CompositorState;
use crate::compositor::state::scene_order::SPECIAL_WORKSPACE_DIM_ALPHA;
use crate::compositor::state::workspace_presentation::sample_slide_motion;
use crate::presentation_animation::{AnimationCurve, AnimationTime, SpringSpec};
use crate::wm::{SpecialWorkspaceId, SpecialWorkspaceToggleOutcome};

/// Vertical travel of a special workspace on open and close, as a fraction of
/// the output height. The windows rest at their own position when shown.
pub const SPECIAL_WORKSPACE_SLIDE_OFFSET_FRACTION: f64 = 0.06;

/// The regular workspace-slide spring, settled for a 0..1 progress instead of
/// workspace positions: 0.002 is a fraction of a pixel of the slide and of
/// opacity, so the motion no longer spends ~90 invisible frames in its tail.
pub(in crate::compositor) fn special_workspace_curve(speed: f64) -> AnimationCurve {
    crate::animation_control::scale_curve(
        AnimationCurve::spring(SpringSpec::new(300.0, 38.1051).with_settlement(0.002, 0.05)),
        speed,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialWorkspaceMotionKind {
    /// Presence of one special workspace: 1 while shown, 0 once departed.
    Presence(SpecialWorkspaceId),
    /// Strength of the dim drawn behind the shown special workspace.
    Dim,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SpecialWorkspaceMotion {
    id: u64,
    kind: SpecialWorkspaceMotionKind,
    start_progress: f64,
    target_progress: f64,
    start_velocity: f64,
    started_at: AnimationTime,
    curve: AnimationCurve,
}

impl SpecialWorkspaceMotion {
    fn sample(self, at: AnimationTime) -> (f64, f64, bool) {
        sample_slide_motion(
            self.start_progress,
            self.target_progress,
            self.start_velocity,
            self.started_at,
            self.curve,
            at,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpecialWorkspaceTrackSample {
    pub transition_id: u64,
    pub kind: SpecialWorkspaceMotionKind,
    pub progress: f64,
    pub target_progress: f64,
    pub mathematically_settled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpecialWorkspacePresentationSample {
    pub tracks: Vec<SpecialWorkspaceTrackSample>,
    pub signature: u64,
}

impl SpecialWorkspacePresentationSample {
    /// Sampled presence of `special`, or `None` when it has no presence track.
    pub fn presence_of(&self, special: SpecialWorkspaceId) -> Option<f64> {
        self.tracks.iter().find_map(|track| match track.kind {
            SpecialWorkspaceMotionKind::Presence(id) if id == special => Some(track.progress),
            _ => None,
        })
    }

    /// True while `special` is retained for its out-animation, i.e. its
    /// presence track heads to zero. Its windows stay in the frame scene.
    pub fn is_departing(&self, special: SpecialWorkspaceId) -> bool {
        self.tracks.iter().any(|track| {
            track.kind == SpecialWorkspaceMotionKind::Presence(special)
                && track.target_progress == 0.0
        })
    }
}

/// Special-workspace open, close, and switch motions. Empty when no special
/// workspace animation is in flight, which keeps the frame path unchanged.
#[derive(Debug, Default)]
pub(in crate::compositor) struct SpecialWorkspacePresentationState {
    motions: Vec<SpecialWorkspaceMotion>,
    next_motion_id: u64,
}

impl SpecialWorkspacePresentationState {
    pub fn is_active(&self) -> bool {
        !self.motions.is_empty()
    }

    /// True while `special` heads to zero presence; its windows are retained.
    pub fn departs(&self, special: SpecialWorkspaceId) -> bool {
        self.motions.iter().any(|motion| {
            motion.kind == SpecialWorkspaceMotionKind::Presence(special)
                && motion.target_progress == 0.0
        })
    }

    pub fn clear(&mut self) -> bool {
        let had_motions = !self.motions.is_empty();
        self.motions.clear();
        had_motions
    }

    /// Starts or retargets one motion. A retarget begins from the sampled
    /// position and velocity of the motion it replaces, so it reverses
    /// smoothly. `fallback_start` is used when no motion exists yet.
    fn begin(
        &mut self,
        kind: SpecialWorkspaceMotionKind,
        target_progress: f64,
        fallback_start: f64,
        started_at: AnimationTime,
        curve: AnimationCurve,
    ) {
        let (start_progress, start_velocity) =
            match self.motions.iter().position(|motion| motion.kind == kind) {
                Some(index) => {
                    let existing = self.motions.remove(index);
                    if existing.target_progress == target_progress {
                        self.motions.insert(index, existing);
                        return;
                    }
                    let (position, velocity, _) = existing.sample(started_at);
                    (position, velocity)
                }
                None => (fallback_start, 0.0),
            };
        let id = self.next_motion_id;
        self.next_motion_id = self.next_motion_id.saturating_add(1);
        self.motions.push(SpecialWorkspaceMotion {
            id,
            kind,
            start_progress,
            target_progress,
            start_velocity,
            started_at,
            curve,
        });
    }

    pub fn begin_for_outcome(
        &mut self,
        outcome: SpecialWorkspaceToggleOutcome,
        started_at: AnimationTime,
        curve: AnimationCurve,
    ) {
        match outcome {
            SpecialWorkspaceToggleOutcome::Opened { id } => {
                self.begin(
                    SpecialWorkspaceMotionKind::Presence(id),
                    1.0,
                    0.0,
                    started_at,
                    curve,
                );
                self.begin(SpecialWorkspaceMotionKind::Dim, 1.0, 0.0, started_at, curve);
            }
            SpecialWorkspaceToggleOutcome::Closed { id } => {
                self.begin(
                    SpecialWorkspaceMotionKind::Presence(id),
                    0.0,
                    1.0,
                    started_at,
                    curve,
                );
                self.begin(SpecialWorkspaceMotionKind::Dim, 0.0, 1.0, started_at, curve);
            }
            SpecialWorkspaceToggleOutcome::Switched { previous, current } => {
                // Both are special workspaces, so the dim stays at full progress.
                self.begin(
                    SpecialWorkspaceMotionKind::Presence(previous),
                    0.0,
                    1.0,
                    started_at,
                    curve,
                );
                self.begin(
                    SpecialWorkspaceMotionKind::Presence(current),
                    1.0,
                    0.0,
                    started_at,
                    curve,
                );
            }
            SpecialWorkspaceToggleOutcome::UnknownSpecial { .. } => {}
        }
    }

    pub fn sample_at(&self, at: AnimationTime) -> Option<SpecialWorkspacePresentationSample> {
        if self.motions.is_empty() {
            return None;
        }
        let tracks = self
            .motions
            .iter()
            .map(|motion| {
                let (progress, _, settled) = motion.sample(at);
                SpecialWorkspaceTrackSample {
                    transition_id: motion.id,
                    kind: motion.kind,
                    progress,
                    target_progress: motion.target_progress,
                    mathematically_settled: settled,
                }
            })
            .collect::<Vec<_>>();
        let signature = special_workspace_sample_signature(&tracks);
        Some(SpecialWorkspacePresentationSample { tracks, signature })
    }

    /// Drops every motion whose settled track was rendered. A motion that was
    /// retargeted after sampling has a new id and stays until its own settle.
    pub fn retire_if_rendered(&mut self, sample: &SpecialWorkspacePresentationSample) -> bool {
        let before = self.motions.len();
        self.motions.retain(|motion| {
            !sample
                .tracks
                .iter()
                .any(|track| track.transition_id == motion.id && track.mathematically_settled)
        });
        self.motions.len() != before
    }

    fn dim_motion_progress(&self, at: AnimationTime) -> Option<f64> {
        self.motions
            .iter()
            .find(|motion| motion.kind == SpecialWorkspaceMotionKind::Dim)
            .map(|motion| motion.sample(at).0.clamp(0.0, 1.0))
    }

    pub fn has_dim_motion(&self) -> bool {
        self.motions
            .iter()
            .any(|motion| motion.kind == SpecialWorkspaceMotionKind::Dim)
    }
}

fn special_workspace_sample_signature(tracks: &[SpecialWorkspaceTrackSample]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut signature = OFFSET;
    let mut mix = |value: u64| {
        signature ^= value;
        signature = signature.wrapping_mul(PRIME);
    };
    for track in tracks {
        mix(track.transition_id);
        match track.kind {
            SpecialWorkspaceMotionKind::Presence(id) => {
                mix(1);
                mix(u64::from(id.get()));
            }
            SpecialWorkspaceMotionKind::Dim => mix(2),
        }
        mix(track.progress.to_bits());
        mix(track.target_progress.to_bits());
        mix(u64::from(track.mathematically_settled));
    }
    signature
}

impl CompositorState {
    /// Same admission policy as the regular workspace slide: the animation
    /// system must be enabled and the workspace switch effect must be a slide.
    pub(in crate::compositor) fn special_workspace_slide_enabled(&self) -> bool {
        self.workspace_slide_effect_enabled()
    }

    /// Starts the open, close, or switch motion for a toggle. With animations
    /// disabled, any running motion is dropped and the change is instant.
    pub(in crate::compositor) fn begin_special_workspace_presentation(
        &mut self,
        outcome: SpecialWorkspaceToggleOutcome,
        sampled_at: AnimationTime,
    ) {
        if !self.special_workspace_slide_enabled() {
            self.special_workspace_presentation.clear();
            return;
        }
        let curve = special_workspace_curve(self.animation_control.configuration().speed);
        self.special_workspace_presentation
            .begin_for_outcome(outcome, sampled_at, curve);
    }

    pub(in crate::compositor) fn special_workspace_presentation_active(&self) -> bool {
        self.special_workspace_presentation.is_active()
    }

    pub(in crate::compositor) fn special_workspace_presentation_sample_at(
        &self,
        sampled_at: AnimationTime,
    ) -> Option<SpecialWorkspacePresentationSample> {
        self.special_workspace_presentation.sample_at(sampled_at)
    }

    /// True while a special workspace is shown or its dim is still animating.
    /// Used by the scanout gate, which has no sample time.
    pub(in crate::compositor) fn special_workspace_dim_active(&self) -> bool {
        self.workspace_manager.visible_special_workspace().is_some()
            || self.special_workspace_presentation.has_dim_motion()
    }

    /// Dim opacity at `at`: `SPECIAL_WORKSPACE_DIM_FULL_ALPHA` scaled by the
    /// dim progress. Without a dim motion it is the static visible-state value.
    pub(in crate::compositor) fn special_workspace_dim_alpha_at(&self, at: AnimationTime) -> f32 {
        let progress = match self.special_workspace_presentation.dim_motion_progress(at) {
            Some(progress) => progress,
            None if self.workspace_manager.visible_special_workspace().is_some() => 1.0,
            None => 0.0,
        };
        if progress >= 1.0 {
            return SPECIAL_WORKSPACE_DIM_ALPHA;
        }
        (f64::from(SPECIAL_WORKSPACE_DIM_ALPHA) * progress) as f32
    }

    pub(in crate::compositor) fn retire_special_workspace_presentation_if_rendered(
        &mut self,
        sample: &SpecialWorkspacePresentationSample,
    ) -> bool {
        let retired = self
            .special_workspace_presentation
            .retire_if_rendered(sample);
        if retired {
            self.refresh_frame_work_visibility();
        }
        retired
    }

    /// A departing special workspace still needs frame callbacks while its
    /// out-animation runs, so its windows keep ticking.
    pub(in crate::compositor) fn special_workspace_departing(
        &self,
        special: SpecialWorkspaceId,
    ) -> bool {
        self.special_workspace_presentation.departs(special)
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use crate::presentation_animation::SpringSpec;

    fn test_curve() -> AnimationCurve {
        AnimationCurve::spring(SpringSpec::new(300.0, 38.1051).with_settlement(0.0001, 0.002))
    }

    fn special(value: u32) -> SpecialWorkspaceId {
        SpecialWorkspaceId::new(value).expect("non-zero special workspace")
    }

    fn at_ms(milliseconds: u64) -> AnimationTime {
        AnimationTime::from_nanos(milliseconds * 1_000_000)
    }

    fn presence(state: &SpecialWorkspacePresentationState, id: SpecialWorkspaceId, ms: u64) -> f64 {
        state
            .sample_at(at_ms(ms))
            .and_then(|sample| sample.presence_of(id))
            .expect("presence track")
    }

    #[test]
    fn open_rises_from_zero_and_settles_at_one() {
        let mut state = SpecialWorkspacePresentationState::default();
        state.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Opened { id: special(1) },
            at_ms(0),
            test_curve(),
        );
        assert_eq!(presence(&state, special(1), 0), 0.0);
        let mid = presence(&state, special(1), 40);
        assert!(mid > 0.0 && mid < 1.0, "mid-open progress {mid}");
        assert_eq!(presence(&state, special(1), 5_000), 1.0);
    }

    #[test]
    fn close_falls_from_one_to_zero_and_retains_windows_until_settled() {
        let mut state = SpecialWorkspacePresentationState::default();
        state.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Closed { id: special(1) },
            at_ms(0),
            test_curve(),
        );
        assert_eq!(presence(&state, special(1), 0), 1.0);
        let sample = state.sample_at(at_ms(40)).expect("sample");
        assert!(sample.is_departing(special(1)));
        assert_eq!(presence(&state, special(1), 5_000), 0.0);
    }

    #[test]
    fn retarget_mid_animation_reverses_from_sampled_progress() {
        let mut state = SpecialWorkspacePresentationState::default();
        state.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Opened { id: special(1) },
            at_ms(0),
            test_curve(),
        );
        let sampled = presence(&state, special(1), 60);
        assert!(sampled > 0.0 && sampled < 1.0);
        state.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Closed { id: special(1) },
            at_ms(60),
            test_curve(),
        );
        // The reversal starts exactly where the open was sampled.
        let restart = presence(&state, special(1), 60);
        assert!((restart - sampled).abs() < 1e-9, "{restart} vs {sampled}");
        assert!(
            state
                .sample_at(at_ms(60))
                .expect("sample")
                .is_departing(special(1))
        );
        assert_eq!(presence(&state, special(1), 5_000), 0.0);
    }

    #[test]
    fn dim_progress_follows_open_and_close() {
        let mut state = SpecialWorkspacePresentationState::default();
        state.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Opened { id: special(1) },
            at_ms(0),
            test_curve(),
        );
        assert_eq!(state.dim_motion_progress(at_ms(0)), Some(0.0));
        let mid = state.dim_motion_progress(at_ms(40)).expect("dim motion");
        assert!(mid > 0.0 && mid < 1.0);
        assert_eq!(state.dim_motion_progress(at_ms(5_000)), Some(1.0));

        state.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Closed { id: special(1) },
            at_ms(5_000),
            test_curve(),
        );
        assert_eq!(state.dim_motion_progress(at_ms(5_000)), Some(1.0));
        assert_eq!(state.dim_motion_progress(at_ms(10_000)), Some(0.0));
    }

    #[test]
    fn switch_animates_both_specials_and_keeps_dim_out_of_motion() {
        let mut state = SpecialWorkspacePresentationState::default();
        state.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Switched {
                previous: special(1),
                current: special(2),
            },
            at_ms(0),
            test_curve(),
        );
        assert!(!state.has_dim_motion());
        assert_eq!(presence(&state, special(1), 0), 1.0);
        assert_eq!(presence(&state, special(2), 0), 0.0);
        let sample = state.sample_at(at_ms(40)).expect("sample");
        assert!(sample.is_departing(special(1)));
        assert!(!sample.is_departing(special(2)));
        assert!(presence(&state, special(2), 40) > 0.0);
        assert_eq!(presence(&state, special(1), 5_000), 0.0);
        assert_eq!(presence(&state, special(2), 5_000), 1.0);
    }

    #[test]
    fn retirement_drops_only_settled_rendered_tracks() {
        let mut state = SpecialWorkspacePresentationState::default();
        state.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Opened { id: special(1) },
            at_ms(0),
            test_curve(),
        );
        let running = state.sample_at(at_ms(40)).expect("sample");
        assert!(!state.retire_if_rendered(&running));
        assert!(state.is_active());

        let settled = state.sample_at(at_ms(5_000)).expect("sample");
        assert!(state.retire_if_rendered(&settled));
        assert!(!state.is_active());
        assert!(state.sample_at(at_ms(5_000)).is_none());
    }

    #[test]
    fn retargeted_track_is_not_retired_by_a_stale_settled_sample() {
        let mut state = SpecialWorkspacePresentationState::default();
        state.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Opened { id: special(1) },
            at_ms(0),
            test_curve(),
        );
        let stale = state.sample_at(at_ms(5_000)).expect("sample");
        state.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Closed { id: special(1) },
            at_ms(5_000),
            test_curve(),
        );
        assert!(!state.retire_if_rendered(&stale));
        assert!(state.is_active());
    }

    #[test]
    fn unknown_special_starts_nothing() {
        let mut state = SpecialWorkspacePresentationState::default();
        state.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::UnknownSpecial { id: special(3) },
            at_ms(0),
            test_curve(),
        );
        assert!(!state.is_active());
        assert!(state.sample_at(at_ms(0)).is_none());
    }

    fn enable_special_slide(state: &mut CompositorState) {
        let mut configuration = crate::animation_control::AnimationConfiguration {
            enabled: true,
            ..crate::animation_control::AnimationConfiguration::default()
        };
        configuration.overrides.insert(
            crate::animation_control::AnimationSlot::WorkspaceSwitch,
            crate::animation_control::AnimationEffect::WorkspaceSlide,
        );
        state
            .set_animation_configuration(configuration)
            .expect("enable workspace slide");
    }

    fn special_window(
        state: &mut CompositorState,
        root_surface_id: u32,
        special_id: SpecialWorkspaceId,
    ) -> crate::wm::WindowId {
        let id = state.allocate_window_id().expect("window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(id, root_surface_id))
            .expect("window");
        state.window_mut(id).expect("window").management =
            Some(crate::wm::WindowManagementState::new(
                crate::wm::WorkspaceLocation::Special(special_id),
            ));
        state.append_renderable_surface(
            crate::compositor::state::tiled_layout_tests::test_renderable_surface(
                root_surface_id,
                64,
                64,
            ),
        );
        state.rebuild_active_scene_view();
        id
    }

    #[test]
    fn closing_special_window_stays_in_frame_scene_but_leaves_active_scene() {
        let mut state = CompositorState::new(None);
        enable_special_slide(&mut state);
        let special_id = special(1);
        let window = special_window(&mut state, 900, special_id);
        assert!(
            state
                .workspace_manager
                .set_visible_special_workspace(Some(special_id))
        );
        state.rebuild_active_scene_view();
        assert!(state.window_is_visible_in_active_scene(window));

        assert_eq!(
            state.toggle_special_workspace(special_id),
            SpecialWorkspaceToggleOutcome::Closed { id: special_id }
        );
        assert!(state.special_workspace_presentation_active());
        assert!(!state.window_is_visible_in_active_scene(window));
        assert_ne!(state.focused_window_id, Some(window));

        let sampled_at = AnimationTime::monotonic_now().expect("monotonic time");
        let frame = state
            .workspace_frame_presentation_scene_at(sampled_at)
            .expect("frame scene while the special departs");
        assert!(
            frame
                .surfaces
                .iter()
                .any(|surface| surface.surface_id == 900),
            "the departing special window is retained in the frame scene"
        );
        assert!(
            frame
                .special_sample
                .as_ref()
                .is_some_and(|sample| sample.is_departing(special_id))
        );
        assert!(frame.workspace_sample.is_none());
    }

    #[test]
    fn animations_disabled_toggle_is_instant_and_disabling_drops_running_motion() {
        let mut state = CompositorState::new(None);
        let special_id = special(1);
        let window = special_window(&mut state, 901, special_id);
        let mut disabled = crate::animation_control::AnimationConfiguration::default();
        disabled.enabled = false;
        state
            .set_animation_configuration(disabled.clone())
            .expect("disable animations");
        assert_eq!(
            state.toggle_special_workspace(special_id),
            SpecialWorkspaceToggleOutcome::Opened { id: special_id }
        );
        assert!(!state.special_workspace_presentation_active());
        assert!(state.window_is_visible_in_active_scene(window));
        assert_eq!(
            state.special_workspace_dim_alpha_at(at_ms(0)),
            SPECIAL_WORKSPACE_DIM_ALPHA
        );

        enable_special_slide(&mut state);
        state.special_workspace_presentation.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Closed { id: special_id },
            at_ms(0),
            test_curve(),
        );
        assert!(state.special_workspace_presentation_active());
        state
            .set_animation_configuration(disabled)
            .expect("disable animations");
        assert!(!state.special_workspace_presentation_active());
    }

    #[test]
    fn dim_alpha_rises_on_open_and_falls_on_close() {
        let mut state = CompositorState::new(None);
        let special_id = special(1);
        let curve = special_workspace_curve(1.0);
        assert_eq!(state.special_workspace_dim_alpha_at(at_ms(0)), 0.0);

        assert!(
            state
                .workspace_manager
                .set_visible_special_workspace(Some(special_id))
        );
        state.special_workspace_presentation.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Opened { id: special_id },
            at_ms(0),
            curve,
        );
        assert_eq!(state.special_workspace_dim_alpha_at(at_ms(0)), 0.0);
        let mid_open = state.special_workspace_dim_alpha_at(at_ms(40));
        assert!(mid_open > 0.0 && mid_open < SPECIAL_WORKSPACE_DIM_ALPHA);
        assert_eq!(
            state.special_workspace_dim_alpha_at(at_ms(5_000)),
            SPECIAL_WORKSPACE_DIM_ALPHA
        );

        assert!(state.workspace_manager.set_visible_special_workspace(None));
        state.special_workspace_presentation.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Closed { id: special_id },
            at_ms(5_000),
            curve,
        );
        assert_eq!(
            state.special_workspace_dim_alpha_at(at_ms(5_000)),
            SPECIAL_WORKSPACE_DIM_ALPHA,
            "the dim stays visible at the start of the close"
        );
        let mid_close = state.special_workspace_dim_alpha_at(at_ms(5_040));
        assert!(mid_close > 0.0 && mid_close < SPECIAL_WORKSPACE_DIM_ALPHA);
        assert_eq!(state.special_workspace_dim_alpha_at(at_ms(10_000)), 0.0);
    }

    #[test]
    fn dim_anchor_keeps_departing_special_window_above_the_dim() {
        use crate::compositor::SpecialWorkspaceDim;
        use crate::compositor::state::tiled_layout_tests::test_renderable_surface;

        let mut state = CompositorState::new(None);
        enable_special_slide(&mut state);
        let special_id = special(1);
        let regular = state.allocate_window_id().expect("window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(regular, 451))
            .expect("window");
        state.window_mut(regular).expect("window").management =
            Some(crate::wm::WindowManagementState::new(
                crate::wm::WorkspaceLocation::Regular(state.active_workspace()),
            ));
        state.append_renderable_surface(test_renderable_surface(451, 64, 64));
        let _ = special_window(&mut state, 452, special_id);
        let surfaces = [
            test_renderable_surface(451, 64, 64),
            test_renderable_surface(452, 64, 64),
        ];

        state.special_workspace_presentation.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Closed { id: special_id },
            at_ms(0),
            test_curve(),
        );
        assert_eq!(
            state.special_workspace_dim(at_ms(0), &surfaces, &[451, 452]),
            SpecialWorkspaceDim::new(SPECIAL_WORKSPACE_DIM_ALPHA, Some(452)),
            "the dim sits between the regular window and the departing special window"
        );
    }

    #[test]
    fn keep_alive_holds_until_the_settled_track_is_rendered_and_retired() {
        let mut state = CompositorState::new(None);
        let special_id = special(1);
        state.special_workspace_presentation.begin_for_outcome(
            SpecialWorkspaceToggleOutcome::Opened { id: special_id },
            at_ms(0),
            test_curve(),
        );
        assert!(state.special_workspace_presentation_active());

        let running = state
            .special_workspace_presentation_sample_at(at_ms(40))
            .expect("running sample");
        assert!(!state.retire_special_workspace_presentation_if_rendered(&running));
        assert!(state.special_workspace_presentation_active());

        let settled = state
            .special_workspace_presentation_sample_at(at_ms(5_000))
            .expect("settled sample");
        assert!(state.retire_special_workspace_presentation_if_rendered(&settled));
        assert!(!state.special_workspace_presentation_active());
        assert!(
            state
                .special_workspace_presentation_sample_at(at_ms(5_000))
                .is_none()
        );
    }
}
