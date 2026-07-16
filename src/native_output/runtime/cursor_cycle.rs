use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicCursorVisibilityPolicy {
    HardwareVisible,
    Hidden,
    UnavailableForDirect,
}

impl AtomicCursorVisibilityPolicy {
    pub(crate) const fn direct_compatible(self, input_visible: bool) -> bool {
        !input_visible || matches!(self, Self::HardwareVisible)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicCursorSubmissionState<'a> {
    Visible(&'a AtomicCursorVisualState),
    Hidden,
    UnavailableForDirect,
}

impl<'a> AtomicCursorSubmissionState<'a> {
    pub(crate) const fn kms_state(self) -> Option<&'a AtomicCursorVisualState> {
        match self {
            Self::Visible(cursor) => Some(cursor),
            Self::Hidden | Self::UnavailableForDirect => None,
        }
    }

    pub(crate) const fn hardware_usable(self) -> bool {
        matches!(self, Self::Visible(_))
    }
}

pub(crate) fn effective_atomic_cursor_state<'a>(
    cursor: &'a NativeAtomicCursor,
    render_mode: NativeCursorRenderMode,
    input_visible: bool,
    client_cursor_active: bool,
) -> AtomicCursorSubmissionState<'a> {
    match atomic_cursor_visibility_policy(
        cursor.desired().visible,
        cursor.failure_latched(),
        render_mode,
        input_visible,
        client_cursor_active,
    ) {
        AtomicCursorVisibilityPolicy::HardwareVisible => {
            AtomicCursorSubmissionState::Visible(cursor.desired())
        }
        AtomicCursorVisibilityPolicy::Hidden => AtomicCursorSubmissionState::Hidden,
        AtomicCursorVisibilityPolicy::UnavailableForDirect => {
            AtomicCursorSubmissionState::UnavailableForDirect
        }
    }
}

pub(crate) fn atomic_cursor_visibility_policy(
    desired_visible: bool,
    failure_latched: bool,
    render_mode: NativeCursorRenderMode,
    input_visible: bool,
    client_cursor_active: bool,
) -> AtomicCursorVisibilityPolicy {
    if !input_visible || !desired_visible {
        AtomicCursorVisibilityPolicy::Hidden
    } else if render_mode != NativeCursorRenderMode::Hardware
        || failure_latched
        || client_cursor_active
    {
        AtomicCursorVisibilityPolicy::UnavailableForDirect
    } else {
        AtomicCursorVisibilityPolicy::HardwareVisible
    }
}

pub(crate) fn synchronize_cursor_state(
    atomic_cursor: &mut Option<NativeAtomicCursor>,
    legacy_cursor: &mut Option<NativeLegacyHardwareCursor>,
    input_state: &NativeInputState,
) -> io::Result<()> {
    let (x, y) = input_state.cursor_position();
    if let Some(cursor) = atomic_cursor.as_mut() {
        cursor.set_position(x, y);
        cursor.set_visible(input_state.cursor_visible());
    }
    if let Some(cursor) = legacy_cursor.as_mut() {
        if input_state.cursor_visible() {
            if !cursor.active {
                cursor.enable()?;
            }
            cursor.move_to(x, y)?;
        } else {
            cursor.disable()?;
        }
    }
    Ok(())
}

pub(crate) fn apply_cursor_position(
    atomic_cursor: &mut Option<NativeAtomicCursor>,
    legacy_cursor: &mut Option<NativeLegacyHardwareCursor>,
    position: Option<(i32, i32)>,
    visible: bool,
    cursor_preference: NativeCursorPreference,
    cursor_render_mode: &mut NativeCursorRenderMode,
    perf: NativePerfLogger,
) -> io::Result<()> {
    let Some((x, y)) = position else {
        return Ok(());
    };
    if let Some(cursor) = atomic_cursor.as_mut() {
        cursor.set_position(x, y);
        cursor.set_visible(visible);
    }
    let Some(cursor) = legacy_cursor.as_mut() else {
        return Ok(());
    };
    if let Err(error) = cursor.move_to(x, y) {
        if cursor_preference == NativeCursorPreference::Hardware {
            return Err(error);
        }
        eprintln!("native cursor: hardware cursor move failed: {error}; using software");
        *legacy_cursor = None;
        *cursor_render_mode = NativeCursorRenderMode::Software;
        perf.log("native.cursor", || {
            vec![
                NativePerfField::str("backend", cursor_render_mode.as_str()),
                NativePerfField::str("policy", cursor_preference.as_str()),
                NativePerfField::str("fallback", "move_failed"),
                NativePerfField::str("error", error.to_string()),
            ]
        });
    }
    Ok(())
}

pub(crate) fn complete_cursor_only_pageflip(
    atomic_cursor: &mut Option<NativeAtomicCursor>,
    pageflip_token: u64,
    generation: u64,
    perf: NativePerfLogger,
) -> io::Result<bool> {
    let is_cursor_only = atomic_cursor.as_ref().is_some_and(|cursor| {
        !cursor.pending_is_primary()
            && cursor
                .pending_token()
                .is_some_and(|token| token.get() == pageflip_token)
    });
    if !is_cursor_only {
        return Ok(false);
    }
    let token = PageFlipToken::new(pageflip_token)
        .ok_or_else(|| io::Error::other("cursor pageflip token is zero"))?;
    atomic_cursor
        .as_mut()
        .expect("cursor token implies cursor owner")
        .complete_submission(token, generation)?;
    perf.log("native.cursor", || {
        vec![
            NativePerfField::str("event", "pageflip_complete"),
            NativePerfField::u64("generation", generation),
            NativePerfField::u64("token", pageflip_token),
        ]
    });
    Ok(true)
}

pub(crate) fn schedule_coalesced_cursor_update(
    atomic_cursor: &Option<NativeAtomicCursor>,
    event_loop: &mut NativeEventLoop,
) -> io::Result<()> {
    if atomic_cursor
        .as_ref()
        .is_some_and(NativeAtomicCursor::needs_submission)
    {
        event_loop.arm_deadline(Some(monotonic_now_ns()?))?;
    }
    Ok(())
}

pub(crate) fn complete_primary_cursor_pageflip(
    atomic_cursor: &mut Option<NativeAtomicCursor>,
    pageflip_token: u64,
    generation: u64,
) -> io::Result<()> {
    if let Some(cursor) = atomic_cursor.as_mut()
        && cursor.pending_is_primary()
        && cursor
            .pending_token()
            .is_some_and(|token| token.get() == pageflip_token)
    {
        cursor.complete_submission(
            PageFlipToken::new(pageflip_token)
                .ok_or_else(|| io::Error::other("pageflip token is zero"))?,
            generation,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_mode_keeps_atomic_cursor_plane_disabled() {
        assert_eq!(
            atomic_cursor_visibility_policy(
                true,
                false,
                NativeCursorRenderMode::Software,
                true,
                false,
            ),
            AtomicCursorVisibilityPolicy::UnavailableForDirect
        );
    }

    #[test]
    fn failure_latch_keeps_plane_disabled_after_input_visibility_sync() {
        assert_eq!(
            atomic_cursor_visibility_policy(
                true,
                true,
                NativeCursorRenderMode::Hardware,
                true,
                false,
            ),
            AtomicCursorVisibilityPolicy::UnavailableForDirect
        );
    }

    #[test]
    fn client_cursor_keeps_atomic_plane_disabled() {
        assert_eq!(
            atomic_cursor_visibility_policy(
                true,
                false,
                NativeCursorRenderMode::Hardware,
                true,
                true,
            ),
            AtomicCursorVisibilityPolicy::UnavailableForDirect
        );
    }

    #[test]
    fn primary_submit_uses_effective_hidden_state_in_software_mode() {
        let policy = atomic_cursor_visibility_policy(
            true,
            false,
            NativeCursorRenderMode::Software,
            true,
            false,
        );
        assert!(!matches!(
            policy,
            AtomicCursorVisibilityPolicy::HardwareVisible
        ));
    }

    #[test]
    fn direct_test_uses_effective_hidden_state_when_pointer_hidden() {
        assert_eq!(
            atomic_cursor_visibility_policy(
                true,
                false,
                NativeCursorRenderMode::Software,
                false,
                false,
            ),
            AtomicCursorVisibilityPolicy::Hidden
        );
    }

    #[test]
    fn normal_motion_does_not_clear_failure_latch() {
        assert_eq!(
            atomic_cursor_visibility_policy(
                true,
                true,
                NativeCursorRenderMode::Hardware,
                true,
                false,
            ),
            AtomicCursorVisibilityPolicy::UnavailableForDirect
        );
    }

    #[test]
    fn new_drm_generation_can_clear_failure_latch() {
        assert_eq!(
            atomic_cursor_visibility_policy(
                true,
                false,
                NativeCursorRenderMode::Hardware,
                true,
                false,
            ),
            AtomicCursorVisibilityPolicy::HardwareVisible
        );
    }
}
