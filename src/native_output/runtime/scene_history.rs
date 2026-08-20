use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeFrameSceneSnapshot {
    pub(crate) frame_id: u64,
    pub(crate) render_generation: u64,
    pub(crate) scene: NativeSceneSnapshot,
    pub(crate) cursor_damage: NativeCursorDamageBounds,
}

impl NativeFrameSceneSnapshot {
    pub(crate) fn from_resolved_frame_scene(
        frame_id: u64,
        resolved: &ResolvedNativeFrameScene<'_>,
        cursor_damage: NativeCursorDamageBounds,
    ) -> Self {
        Self {
            frame_id,
            render_generation: resolved.render_generation,
            scene: resolved.snapshot(),
            cursor_damage,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct NativeSceneHistory {
    presented: Option<NativeFrameSceneSnapshot>,
    ready: Option<NativeFrameSceneSnapshot>,
    submitted: VecDeque<(u64, NativeFrameSceneSnapshot)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedNativePresentationTransition {
    pub(crate) token: u64,
    pub(crate) previous_frame_id: Option<u64>,
    pub(crate) current_frame_id: u64,
    pub(crate) damage: OutputDamage,
}

impl NativeSceneHistory {
    const MAX_SUBMITTED_SCENES: usize = 3;

    pub(crate) fn new(presented: NativeFrameSceneSnapshot) -> Self {
        Self {
            presented: Some(presented),
            ready: None,
            submitted: VecDeque::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn presented_scene(&self) -> &NativeSceneSnapshot {
        &self
            .presented
            .as_ref()
            .expect("caller must establish a presented scene before using this accessor")
            .scene
    }

    pub(crate) fn presented_scene_if_any(&self) -> Option<&NativeSceneSnapshot> {
        self.presented.as_ref().map(|snapshot| &snapshot.scene)
    }

    pub(crate) fn presented_frame_id(&self) -> Option<u64> {
        self.presented.as_ref().map(|snapshot| snapshot.frame_id)
    }

    pub(crate) fn presented_cursor_damage(&self) -> NativeCursorDamageBounds {
        let Some(cursor_damage) = self
            .presented
            .as_ref()
            .map(|snapshot| snapshot.cursor_damage)
        else {
            return NativeCursorDamageBounds::default();
        };
        NativeCursorDamageBounds {
            previous_client: cursor_damage.client,
            client: cursor_damage.client,
            previous_software: cursor_damage.software,
            software: cursor_damage.software,
        }
    }

    pub(crate) fn cursor_damage(
        &self,
        (client, software): (
            Option<NativeClientCursorDamageState>,
            Option<NativeDamageRect>,
        ),
    ) -> NativeCursorDamageBounds {
        let presented = self.presented_cursor_damage();
        NativeCursorDamageBounds {
            previous_client: presented.client,
            client,
            previous_software: presented.software,
            software,
        }
    }

    pub(crate) fn invalidate_presented_damage_history(&mut self) {
        self.presented = None;
        self.ready = None;
        self.submitted.clear();
    }

    pub(crate) fn replace_ready(&mut self, snapshot: NativeFrameSceneSnapshot) {
        self.ready = Some(snapshot);
    }

    pub(crate) fn discard_ready(&mut self) {
        self.ready = None;
    }

    pub(crate) fn queue_submission(&mut self, token: u64) -> bool {
        if self.submitted.len() >= Self::MAX_SUBMITTED_SCENES {
            return false;
        }
        let Some(snapshot) = self.ready.take() else {
            return false;
        };
        self.submitted.push_back((token, snapshot));
        true
    }

    pub(crate) fn queue_submission_or_error(&mut self, token: u64) -> NativeResult<()> {
        self.queue_submission(token).then_some(()).ok_or_else(|| {
            io::Error::other("native submission has no rendered scene snapshot").into()
        })
    }

    pub(crate) fn prepare_pageflip_transition(
        &self,
        token: u64,
        output_width: u32,
        output_height: u32,
    ) -> Option<PreparedNativePresentationTransition> {
        let (_, current) = self
            .submitted
            .iter()
            .find(|(submitted_token, _)| *submitted_token == token)?;
        let previous_frame_id = self.presented.as_ref().map(|snapshot| snapshot.frame_id);
        if previous_frame_id.is_some_and(|frame_id| current.frame_id <= frame_id) {
            return None;
        }
        let damage = match self.presented.as_ref() {
            Some(previous) => native_output_damage_for_scene_snapshots(
                output_width,
                output_height,
                &previous.scene,
                &current.scene,
                NativeCursorDamageBounds {
                    previous_client: previous.cursor_damage.client,
                    client: current.cursor_damage.client,
                    previous_software: previous.cursor_damage.software,
                    software: current.cursor_damage.software,
                },
            )
            .as_renderer_damage(output_width, output_height),
            None => OutputDamage::Full,
        };
        Some(PreparedNativePresentationTransition {
            token,
            previous_frame_id,
            current_frame_id: current.frame_id,
            damage,
        })
    }

    pub(crate) fn promote_immediate(&mut self) -> bool {
        let Some(snapshot) = self.ready.take() else {
            return false;
        };
        self.presented = Some(snapshot);
        true
    }

    pub(crate) fn promote_immediate_or_error(&mut self) -> NativeResult<()> {
        self.promote_immediate().then_some(()).ok_or_else(|| {
            io::Error::other("immediate presentation has no rendered scene snapshot").into()
        })
    }

    pub(crate) fn promote_pageflip(&mut self, token: u64) -> bool {
        let Some(index) = self
            .submitted
            .iter()
            .position(|(submitted_token, _)| *submitted_token == token)
        else {
            return false;
        };
        let (_, snapshot) = self
            .submitted
            .remove(index)
            .expect("position was returned from submitted scene history");
        if self
            .presented
            .as_ref()
            .is_some_and(|presented| snapshot.frame_id <= presented.frame_id)
        {
            return false;
        }
        self.presented = Some(snapshot);
        true
    }

    pub(crate) fn discard_submission(&mut self, token: u64) -> bool {
        let Some(index) = self
            .submitted
            .iter()
            .position(|(submitted_token, _)| *submitted_token == token)
        else {
            return false;
        };
        self.submitted.remove(index).is_some()
    }

    pub(crate) fn discard_unpresented(&mut self) {
        self.ready = None;
        self.submitted.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(frame_id: u64) -> NativeFrameSceneSnapshot {
        NativeFrameSceneSnapshot {
            frame_id,
            render_generation: frame_id,
            scene: NativeSceneSnapshot::default(),
            cursor_damage: NativeCursorDamageBounds::default(),
        }
    }

    #[test]
    fn rendered_snapshot_advances_presented_history_only_on_matching_pageflip() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(1)
        );
        assert!(!history.promote_pageflip(77));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(1)
        );
        assert!(history.queue_submission(77));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(1)
        );
        assert!(history.promote_pageflip(77));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(2)
        );
    }

    #[test]
    fn pageflip_transition_uses_the_actual_presented_predecessor() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert!(history.queue_submission(20));
        history.replace_ready(snapshot(3));
        assert!(history.queue_submission(30));

        let b_transition = history
            .prepare_pageflip_transition(20, 100, 80)
            .expect("B transition must be prepared");
        assert_eq!(b_transition.previous_frame_id, Some(1));
        assert_eq!(b_transition.current_frame_id, 2);
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(1)
        );
        assert!(history.promote_pageflip(20));

        let c_transition = history
            .prepare_pageflip_transition(30, 100, 80)
            .expect("C transition must be prepared");
        assert_eq!(c_transition.previous_frame_id, Some(2));
        assert_eq!(c_transition.current_frame_id, 3);
    }

    #[test]
    fn rejected_render_ahead_candidate_does_not_change_next_transition_predecessor() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert!(history.queue_submission(20));
        history.replace_ready(snapshot(3));
        assert!(history.queue_submission(30));
        assert!(history.discard_submission(20));

        let transition = history
            .prepare_pageflip_transition(30, 100, 80)
            .expect("C transition must be prepared");
        assert_eq!(transition.previous_frame_id, Some(1));
        assert_eq!(transition.current_frame_id, 3);
    }

    #[test]
    fn delayed_pageflip_uses_submitted_c_not_mutable_ready_d() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert!(history.queue_submission(20));
        assert!(history.promote_pageflip(20));
        history.replace_ready(snapshot(3));
        assert!(history.queue_submission(30));
        history.replace_ready(snapshot(4));

        let transition = history
            .prepare_pageflip_transition(30, 100, 80)
            .expect("C transition must remain available");
        assert_eq!(transition.previous_frame_id, Some(2));
        assert_eq!(transition.current_frame_id, 3);
    }

    #[test]
    fn rejected_c_is_not_inserted_between_b_and_d() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert!(history.queue_submission(20));
        assert!(history.promote_pageflip(20));
        history.replace_ready(snapshot(3));
        assert!(history.queue_submission(30));
        assert!(history.discard_submission(30));
        history.replace_ready(snapshot(4));
        assert!(history.queue_submission(40));

        let transition = history
            .prepare_pageflip_transition(40, 100, 80)
            .expect("D transition must be prepared");
        assert_eq!(transition.previous_frame_id, Some(2));
        assert_eq!(transition.current_frame_id, 4);
    }

    #[test]
    fn replacing_ready_and_discarding_submission_never_promotes_stale_frame() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert!(history.queue_submission(20));
        history.replace_ready(snapshot(3));
        assert!(history.discard_submission(20));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(1)
        );
        assert!(history.promote_immediate());
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(3)
        );
    }

    #[test]
    fn stale_pageflip_token_cannot_regress_newer_presented_scene() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert!(history.queue_submission(20));
        history.replace_ready(snapshot(3));
        assert!(history.queue_submission(30));
        assert!(history.promote_pageflip(30));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(3)
        );
        assert!(!history.promote_pageflip(20));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(3)
        );
    }

    #[test]
    fn multiple_rejected_frames_leave_presented_scene_unchanged_until_d_pageflip() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        for (frame_id, token) in [(2, 20), (3, 30)] {
            history.replace_ready(snapshot(frame_id));
            assert!(history.queue_submission(token));
            assert!(history.discard_submission(token));
            assert_eq!(
                history.presented.as_ref().map(|frame| frame.frame_id),
                Some(1)
            );
        }

        history.replace_ready(snapshot(4));
        assert!(history.queue_submission(40));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(1)
        );
        assert!(history.promote_pageflip(40));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(4)
        );
    }

    #[test]
    fn direct_primary_boundary_has_no_composited_scene_until_return() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert!(history.queue_submission(20));

        history.invalidate_presented_damage_history();

        assert!(history.presented_scene_if_any().is_none());
        assert!(history.ready.is_none());
        assert!(history.submitted.is_empty());
        assert_eq!(
            history.cursor_damage((None, None)),
            NativeCursorDamageBounds::default()
        );

        history.replace_ready(snapshot(3));
        assert!(history.queue_submission(30));
        assert!(history.promote_pageflip(30));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(3)
        );
    }
}
