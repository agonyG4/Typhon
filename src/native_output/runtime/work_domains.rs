use super::{NativeWakeup, NativeWorkClass, NativeWorkDecision};

/// Runtime state that is not encoded in a single reactor wakeup.
///
/// This is deliberately a compact, allocation-free snapshot.  The native
/// loop uses it to keep readiness domains independent: input can be drained
/// without promoting a hardware cursor wake into a scene render, while an
/// explicit-sync or pacing deadline still gets serviced promptly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NativeRuntimeState {
    pub(super) scene_dirty: bool,
    pub(super) visual_work_deadline_due: bool,
    pub(super) cursor_only_due: bool,
    pub(super) explicit_sync_pending: bool,
    pub(super) astrea_publication_due: bool,
    pub(super) commit_timing_planning_due: bool,
    pub(super) pacing_active: bool,
    pub(super) pacing_due: bool,
    pub(super) xwayland_generation_changed: bool,
    pub(super) recovery_required: bool,
    pub(super) shutdown_requested: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NativeWorkDomains {
    pub(super) input: bool,
    pub(super) wayland_protocol: bool,
    pub(super) astrea_publication: bool,
    pub(super) commit_timing_planning: bool,
    pub(super) wayland_dispatch: bool,
    pub(super) scene: bool,
    pub(super) cursor: bool,
    pub(super) presentation: bool,
    pub(super) explicit_sync: bool,
    pub(super) surface_pacing: bool,
    pub(super) xwayland: bool,
    pub(super) control: bool,
    pub(super) children: bool,
    pub(super) session: bool,
    pub(super) shutdown: bool,
}

impl NativeWorkDomains {
    pub(super) const fn should_service_surface_pacing(self) -> bool {
        self.surface_pacing && !self.wayland_dispatch
    }

    pub(super) fn service_surface_pacing_if_due<E>(
        self,
        service: impl FnOnce() -> Result<bool, E>,
    ) -> Result<bool, E> {
        if !self.should_service_surface_pacing() {
            return Ok(false);
        }
        service()
    }

    pub(super) fn from_wakeup(
        wakeup: &NativeWakeup,
        state: &NativeRuntimeState,
    ) -> NativeWorkDecision {
        Self::classify(wakeup, state).decision()
    }

    pub(super) fn classify(wakeup: &NativeWakeup, state: &NativeRuntimeState) -> Self {
        let reasons = wakeup.reasons;
        let input = reasons.input();
        let wayland_protocol = reasons.wayland_listener() || reasons.wayland_clients();
        let control = reasons.control();
        let children = reasons.child_signal();
        let session = reasons.seat();
        let xwayland = reasons.xwayland_listen()
            || reasons.xwayland_display_ready()
            || reasons.xwayland_xwm()
            || reasons.xwayland_stderr()
            || !wakeup.xwayland_events.is_empty()
            || state.xwayland_generation_changed;
        let explicit_sync = reasons.explicit_sync_acquire()
            || !wakeup.explicit_sync_acquire_tokens.is_empty()
            || state.explicit_sync_pending;
        let astrea_publication = state.astrea_publication_due;
        let commit_timing_planning = state.commit_timing_planning_due;
        let cursor = reasons.cursor_io_worker()
            || !wakeup.cursor_io_events.is_empty()
            || state.cursor_only_due;
        let wayland_dispatch = wayland_protocol;
        let surface_pacing = state.pacing_due;
        let scene = state.scene_dirty || state.visual_work_deadline_due || state.recovery_required;
        let presentation = reasons.drm()
            || reasons.kms_commit_worker()
            || reasons.output_render_fence()
            || state.visual_work_deadline_due
            || state.recovery_required;

        Self {
            input,
            wayland_protocol,
            astrea_publication,
            commit_timing_planning,
            wayland_dispatch,
            scene,
            cursor,
            presentation,
            explicit_sync,
            surface_pacing,
            xwayland,
            control,
            children,
            session,
            shutdown: state.shutdown_requested,
        }
    }

    pub(super) const fn decision(self) -> NativeWorkDecision {
        let protocol_work = self.wayland_protocol
            || self.astrea_publication
            || self.commit_timing_planning
            || self.xwayland
            || self.explicit_sync
            || self.surface_pacing
            || self.control
            || self.children
            || self.session;
        NativeWorkDecision::new(
            NativeWorkClass::from_flags(protocol_work, self.cursor, self.scene),
            self.xwayland,
            self.surface_pacing,
            self.explicit_sync,
            self.astrea_publication,
            self.commit_timing_planning,
            self.scene,
            self.control,
            self.children,
            self.session,
            self.shutdown,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oblivion_one::native::event_loop::WakeReasons;

    const INPUT: u32 = 1 << 3;
    const TIMER: u32 = 1 << 4;
    const WAYLAND_CLIENTS: u32 = 1 << 2;
    const EXPLICIT_SYNC_ACQUIRE: u32 = 1 << 5;
    const CHILD_SIGNAL: u32 = 1 << 6;
    const SEAT: u32 = 1 << 7;
    const XWAYLAND_XWM: u32 = 1 << 11;
    const CONTROL: u32 = 1 << 14;

    fn wakeup(bits: u32) -> NativeWakeup {
        NativeWakeup {
            reasons: WakeReasons::from_bits(bits),
            ready_sources: 1,
            blocked_ns: 0,
            timer_lateness_ns: None,
            explicit_sync_acquire_tokens: Vec::new(),
            xwayland_events: Vec::new(),
            control_events: Vec::new(),
            cursor_io_events: Vec::new(),
        }
    }

    fn state() -> NativeRuntimeState {
        NativeRuntimeState {
            pacing_active: false,
            ..NativeRuntimeState::default()
        }
    }

    #[test]
    fn input_with_stable_hardware_cursor_is_a_pure_input_fast_path() {
        let decision = NativeWorkDomains::from_wakeup(&wakeup(INPUT), &state());

        assert_eq!(decision.work_class, NativeWorkClass::NoOutputWork);
        assert!(!decision.service_primary_scene);
        assert!(!decision.service_xwayland);
        assert!(!decision.service_pacing);
    }

    #[test]
    fn input_only_readiness_does_not_request_wayland_read_dispatch() {
        let domains = NativeWorkDomains::classify(&wakeup(INPUT), &state());

        assert!(!domains.wayland_dispatch);
    }

    #[test]
    fn combined_input_and_wayland_readiness_keeps_both_domains_once() {
        let domains = NativeWorkDomains::classify(&wakeup(INPUT | WAYLAND_CLIENTS), &state());

        assert!(domains.input);
        assert!(domains.wayland_dispatch);
        assert!(!domains.should_service_surface_pacing());
    }

    #[test]
    fn one_thousand_independent_input_wakes_do_not_count_as_wayland_ticks() {
        let mut input_cycles = 0;
        let mut wayland_read_dispatches = 0;

        for _ in 0..1_000 {
            let domains = NativeWorkDomains::classify(&wakeup(INPUT), &state());
            input_cycles += u64::from(domains.input);
            wayland_read_dispatches += u64::from(domains.wayland_dispatch);
        }

        assert_eq!(input_cycles, 1_000);
        assert_eq!(wayland_read_dispatches, 0);
    }

    #[test]
    fn input_with_explicit_sync_requires_acquire_service() {
        let decision =
            NativeWorkDomains::from_wakeup(&wakeup(INPUT | EXPLICIT_SYNC_ACQUIRE), &state());

        assert_eq!(decision.work_class, NativeWorkClass::ProtocolOnly);
        assert!(decision.service_explicit_sync_acquire);
    }

    #[test]
    fn input_with_pacing_deadline_requires_pacing_service() {
        let decision = NativeWorkDomains::from_wakeup(
            &wakeup(INPUT | TIMER),
            &NativeRuntimeState {
                pacing_due: true,
                ..state()
            },
        );

        assert_eq!(decision.work_class, NativeWorkClass::ProtocolOnly);
        assert!(decision.service_pacing);
    }

    #[test]
    fn commit_timing_planning_is_protocol_only_and_independent_of_scene_work() {
        let decision = NativeWorkDomains::from_wakeup(
            &wakeup(INPUT),
            &NativeRuntimeState {
                commit_timing_planning_due: true,
                scene_dirty: false,
                ..state()
            },
        );

        assert_eq!(decision.work_class, NativeWorkClass::ProtocolOnly);
        assert!(decision.service_commit_timing_planning);
        assert!(!decision.service_primary_scene);
    }

    #[test]
    fn one_thousand_future_pacing_input_cycles_do_not_service_pacing() {
        let mut pacing_services = 0;
        let state = NativeRuntimeState {
            pacing_active: true,
            pacing_due: false,
            ..state()
        };
        for _ in 0..1_000 {
            let domains = NativeWorkDomains::classify(&wakeup(INPUT), &state);
            domains
                .service_surface_pacing_if_due(|| {
                    pacing_services += 1;
                    Ok::<bool, ()>(false)
                })
                .unwrap();
        }

        assert_eq!(pacing_services, 0);
    }

    #[test]
    fn due_pacing_service_returns_the_visual_handoff() {
        let domains = NativeWorkDomains::classify(
            &wakeup(TIMER),
            &NativeRuntimeState {
                pacing_due: true,
                ..state()
            },
        );

        assert!(
            domains
                .service_surface_pacing_if_due(|| Ok::<bool, ()>(true))
                .unwrap()
        );
    }

    #[test]
    fn active_future_pacing_does_not_promote_an_unrelated_timer() {
        let domains = NativeWorkDomains::classify(
            &wakeup(TIMER),
            &NativeRuntimeState {
                pacing_active: true,
                pacing_due: false,
                ..state()
            },
        );

        assert!(!domains.surface_pacing);
    }

    #[test]
    fn input_does_not_service_an_older_commit_timing_plan() {
        let domains = NativeWorkDomains::classify(
            &wakeup(INPUT),
            &NativeRuntimeState {
                commit_timing_planning_due: false,
                ..state()
            },
        );

        assert!(!domains.commit_timing_planning);
    }

    #[test]
    fn input_with_xwm_readiness_requires_xwayland_service() {
        let decision = NativeWorkDomains::from_wakeup(&wakeup(INPUT | XWAYLAND_XWM), &state());

        assert_eq!(decision.work_class, NativeWorkClass::ProtocolOnly);
        assert!(decision.service_xwayland);
    }

    #[test]
    fn input_with_control_child_and_session_readiness_services_each_domain() {
        let domains =
            NativeWorkDomains::classify(&wakeup(INPUT | CONTROL | CHILD_SIGNAL | SEAT), &state());

        assert!(!domains.wayland_dispatch);
        assert!(domains.control);
        assert!(domains.children);
        assert!(domains.session);
        assert_eq!(domains.decision().work_class, NativeWorkClass::ProtocolOnly);
    }

    #[test]
    fn input_with_cursor_deadline_is_cursor_only() {
        let decision = NativeWorkDomains::from_wakeup(
            &wakeup(INPUT),
            &NativeRuntimeState {
                cursor_only_due: true,
                ..state()
            },
        );

        assert_eq!(decision.work_class, NativeWorkClass::CursorOnly);
        assert!(!decision.service_primary_scene);
    }

    #[test]
    fn input_with_scene_dirty_or_interaction_is_primary_scene() {
        let decision = NativeWorkDomains::from_wakeup(
            &wakeup(INPUT),
            &NativeRuntimeState {
                scene_dirty: true,
                ..state()
            },
        );

        assert_eq!(decision.work_class, NativeWorkClass::PrimaryScene);
        assert!(decision.service_primary_scene);
    }

    #[test]
    fn queued_visual_work_waits_until_its_deadline() {
        let waiting = NativeWorkDomains::from_wakeup(
            &wakeup(INPUT),
            &NativeRuntimeState {
                visual_work_deadline_due: false,
                ..state()
            },
        );
        assert_eq!(waiting.work_class, NativeWorkClass::NoOutputWork);
        assert!(!waiting.service_primary_scene);

        let due = NativeWorkDomains::from_wakeup(
            &wakeup(TIMER),
            &NativeRuntimeState {
                visual_work_deadline_due: true,
                ..state()
            },
        );
        assert_eq!(due.work_class, NativeWorkClass::PrimaryScene);
        assert!(due.service_primary_scene);
    }
}
