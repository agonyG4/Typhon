use super::*;

pub(crate) fn defer_cursor_after_busy(
    arbitration: &mut NativeCursorOutputArbitration,
    scheduler: &mut NativeFrameScheduler,
    now_ns: u64,
    perf: NativePerfLogger,
    reason: &'static str,
) {
    arbitration.defer_after_busy(now_ns, scheduler.next_refresh_deadline_ns(now_ns));
    scheduler.note_immediate_completion();
    perf.log("native.cursor", || {
        vec![
            NativePerfField::str("event", "deferred"),
            NativePerfField::str("reason", reason),
        ]
    });
}

pub(crate) fn log_client_cursor_path(
    perf: NativePerfLogger,
    path: NativeClientCursorPath,
    hardware_eligible: bool,
    direct_active: bool,
    client_cursor: Option<oblivion_one::compositor::ClientCursorRenderState<'_>>,
) {
    perf.log("native.cursor", || {
        let mut fields = vec![
            NativePerfField::str("event", "client_cursor_path"),
            NativePerfField::str(
                "path",
                match path {
                    NativeClientCursorPath::Hidden => "hidden",
                    NativeClientCursorPath::Hardware => "hardware",
                    NativeClientCursorPath::Software => "software",
                },
            ),
            NativePerfField::bool("hardware_eligible", hardware_eligible),
            NativePerfField::bool("direct_active", direct_active),
        ];
        if let Some(client) = client_cursor {
            let key = NativeCursorImageKey::for_surface(
                client.surface,
                client.hotspot_x,
                client.hotspot_y,
            );
            fields.extend([
                NativePerfField::u64("surface_id", u64::from(key.surface_id)),
                NativePerfField::u64("buffer_id", key.buffer_id),
                NativePerfField::u64("commit_sequence", key.commit_sequence),
                NativePerfField::str(
                    "buffer_source",
                    format!("{:?}", client.surface.buffer_source()),
                ),
                NativePerfField::str("buffer_size", format!("{}x{}", key.width, key.height)),
                NativePerfField::str("hotspot", format!("{},{}", key.hotspot_x, key.hotspot_y)),
            ]);
        }
        fields
    });
}

pub(crate) const fn resolve_client_cursor_path(
    active: bool,
    hardware_eligible: bool,
) -> NativeClientCursorPath {
    if !active {
        NativeClientCursorPath::Hidden
    } else if hardware_eligible {
        NativeClientCursorPath::Hardware
    } else {
        NativeClientCursorPath::Software
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeResolvedCursorSource {
    Hidden,
    Theme,
    InteractionOverride,
    Client,
}

pub(crate) const fn resolve_native_cursor_source_with_hidden(
    client_active: bool,
    client_explicitly_hidden: bool,
    interaction_override_active: bool,
    theme_visible: bool,
) -> NativeResolvedCursorSource {
    if interaction_override_active {
        NativeResolvedCursorSource::InteractionOverride
    } else if client_active {
        NativeResolvedCursorSource::Client
    } else if client_explicitly_hidden {
        NativeResolvedCursorSource::Hidden
    } else if theme_visible {
        NativeResolvedCursorSource::Theme
    } else {
        NativeResolvedCursorSource::Hidden
    }
}

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
) -> AtomicCursorSubmissionState<'a> {
    match atomic_cursor_visibility_policy(
        cursor.desired().visible,
        cursor.capability_quarantined(),
        render_mode,
        input_visible,
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
    capability_quarantined: bool,
    render_mode: NativeCursorRenderMode,
    input_visible: bool,
) -> AtomicCursorVisibilityPolicy {
    let selection = select_cursor_delivery_mode(CursorModePolicyInput {
        preference: if render_mode == NativeCursorRenderMode::Hardware {
            CursorPreference::Auto
        } else {
            CursorPreference::Software
        },
        visible: input_visible && desired_visible,
        hardware_status: if capability_quarantined {
            CursorHardwareStatus::Quarantined
        } else {
            CursorHardwareStatus::Proven
        },
        geometry_valid: true,
        software_allowed: true,
    });
    match selection {
        CursorModeSelection::Hidden => AtomicCursorVisibilityPolicy::Hidden,
        CursorModeSelection::Hardware => AtomicCursorVisibilityPolicy::HardwareVisible,
        CursorModeSelection::Software | CursorModeSelection::Rejected => {
            AtomicCursorVisibilityPolicy::UnavailableForDirect
        }
    }
}

pub(crate) fn synchronize_cursor_state(
    atomic_cursor: &mut Option<NativeAtomicCursor>,
    legacy_cursor: &mut Option<NativeLegacyHardwareCursor>,
    input_state: &NativeInputState,
    atomic_visible: bool,
    legacy_visible: bool,
) -> io::Result<()> {
    let (x, y) = input_state.cursor_position();
    if let Some(cursor) = atomic_cursor.as_mut() {
        cursor.set_position(x, y);
        cursor.set_visible(atomic_visible);
    }
    if let Some(cursor) = legacy_cursor.as_mut() {
        if !legacy_visible {
            cursor.disable()?;
        } else if input_state.cursor_visible() {
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

pub(crate) fn synchronize_cursor_state_for_server(
    server: &OwnCompositorServer,
    atomic_cursor: &mut Option<NativeAtomicCursor>,
    legacy_cursor: &mut Option<NativeLegacyHardwareCursor>,
    input_state: &NativeInputState,
) -> io::Result<()> {
    let source = resolve_native_cursor_source_with_hidden(
        server.client_cursor_render_state().is_some(),
        server.client_cursor_explicitly_hidden(),
        server.interaction_cursor_override_active(),
        input_state.cursor_visible(),
    );
    synchronize_cursor_state(
        atomic_cursor,
        legacy_cursor,
        input_state,
        !matches!(source, NativeResolvedCursorSource::Hidden),
        server.client_cursor_render_state().is_none(),
    )
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

pub(crate) fn complete_plane_delta_pageflip(
    atomic_cursor: &mut Option<NativeAtomicCursor>,
    pageflip_token: u64,
    generation: u64,
    perf: NativePerfLogger,
) -> io::Result<bool> {
    let is_plane_delta = atomic_cursor.as_ref().is_some_and(|cursor| {
        !cursor.pending_is_primary()
            && cursor
                .pending_token()
                .is_some_and(|token| token.get() == pageflip_token)
    });
    if !is_plane_delta {
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

pub(crate) fn prepare_primary_cursor_pageflip(
    atomic_cursor: &Option<NativeAtomicCursor>,
    pageflip_token: u64,
    generation: u64,
) -> io::Result<bool> {
    let Some(cursor) = atomic_cursor.as_ref() else {
        return Ok(false);
    };
    if !cursor.pending_is_primary()
        || cursor
            .pending_token()
            .is_none_or(|token| token.get() != pageflip_token)
    {
        return Ok(false);
    }
    cursor.prepare_submission_completion(
        PageFlipToken::new(pageflip_token)
            .ok_or_else(|| io::Error::other("cursor pageflip token is zero"))?,
        generation,
    )?;
    Ok(true)
}

pub(crate) fn commit_primary_cursor_pageflip(
    atomic_cursor: &mut Option<NativeAtomicCursor>,
    pageflip_token: u64,
    generation: u64,
) {
    if let Some(cursor) = atomic_cursor.as_mut()
        && cursor.pending_is_primary()
        && cursor
            .pending_token()
            .is_some_and(|token| token.get() == pageflip_token)
    {
        cursor.commit_submission_completion(
            PageFlipToken::new(pageflip_token).expect("prepared cursor pageflip token is nonzero"),
            generation,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_cursor_source_stays_visible_when_theme_cursor_is_hidden() {
        assert_eq!(
            resolve_native_cursor_source_with_hidden(true, false, false, false),
            NativeResolvedCursorSource::Client
        );
    }

    #[test]
    fn hidden_and_interaction_sources_keep_their_precedence() {
        assert_eq!(
            resolve_native_cursor_source_with_hidden(false, false, true, false),
            NativeResolvedCursorSource::InteractionOverride
        );
        assert_eq!(
            resolve_native_cursor_source_with_hidden(false, true, false, true),
            NativeResolvedCursorSource::Hidden
        );
        assert_eq!(
            resolve_native_cursor_source_with_hidden(false, false, false, false),
            NativeResolvedCursorSource::Hidden
        );
    }

    #[test]
    fn explicit_client_concealment_is_restored_after_interaction_override() {
        assert_eq!(
            resolve_native_cursor_source_with_hidden(false, true, true, true),
            NativeResolvedCursorSource::InteractionOverride
        );
        assert_eq!(
            resolve_native_cursor_source_with_hidden(false, true, false, true),
            NativeResolvedCursorSource::Hidden
        );
        assert_eq!(
            resolve_native_cursor_source_with_hidden(true, false, false, true),
            NativeResolvedCursorSource::Client
        );
    }

    #[test]
    fn software_mode_keeps_atomic_cursor_plane_disabled() {
        assert_eq!(
            atomic_cursor_visibility_policy(true, false, NativeCursorRenderMode::Software, true,),
            AtomicCursorVisibilityPolicy::UnavailableForDirect
        );
    }

    #[test]
    fn capability_quarantine_keeps_plane_disabled_after_input_visibility_sync() {
        assert_eq!(
            atomic_cursor_visibility_policy(true, true, NativeCursorRenderMode::Hardware, true,),
            AtomicCursorVisibilityPolicy::UnavailableForDirect
        );
    }

    #[test]
    fn client_cursor_does_not_disable_atomic_plane() {
        assert_eq!(
            atomic_cursor_visibility_policy(true, false, NativeCursorRenderMode::Hardware, true,),
            AtomicCursorVisibilityPolicy::HardwareVisible
        );
    }

    #[test]
    fn primary_submit_uses_effective_hidden_state_in_software_mode() {
        let policy =
            atomic_cursor_visibility_policy(true, false, NativeCursorRenderMode::Software, true);
        assert!(!matches!(
            policy,
            AtomicCursorVisibilityPolicy::HardwareVisible
        ));
    }

    #[test]
    fn direct_test_uses_effective_hidden_state_when_pointer_hidden() {
        assert_eq!(
            atomic_cursor_visibility_policy(true, false, NativeCursorRenderMode::Software, false,),
            AtomicCursorVisibilityPolicy::Hidden
        );
    }

    #[test]
    fn normal_motion_does_not_clear_capability_quarantine() {
        assert_eq!(
            atomic_cursor_visibility_policy(true, true, NativeCursorRenderMode::Hardware, true,),
            AtomicCursorVisibilityPolicy::UnavailableForDirect
        );
    }

    #[test]
    fn new_drm_generation_can_clear_capability_quarantine() {
        assert_eq!(
            atomic_cursor_visibility_policy(true, false, NativeCursorRenderMode::Hardware, true,),
            AtomicCursorVisibilityPolicy::HardwareVisible
        );
    }
}
