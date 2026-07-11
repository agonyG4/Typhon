use super::*;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeIoOperation {
    SeatDispatch,
    SeatDisableAcknowledged,
    InputSuspend,
    InputResume,
    RawInputDiscard,
    RawInputAction,
    ExplicitSyncPark,
    ExplicitSyncRearm,
    ExplicitSyncNotifier,
    PageflipQuarantine,
    PageflipRetire,
    PageflipDrain,
    PageflipSubmit,
    ScanoutPresent,
    DrmSourceUnregister,
    DrmSourceRegister,
    KmsRecovery,
    KmsRestore,
    AtomicCommit,
    LegacyCommit,
    HardwareCursorDrm,
    SchedulerRearm,
    WaylandProgress,
    DestructorDisarm,
}

#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct NativeIoRecorder {
    operations: Vec<NativeIoOperation>,
}

#[cfg(test)]
impl NativeIoRecorder {
    pub(crate) fn record(&mut self, operation: NativeIoOperation) {
        self.operations.push(operation);
    }

    #[cfg(test)]
    pub(crate) fn operations(&self) -> &[NativeIoOperation] {
        &self.operations
    }
}

pub(crate) trait NativeSessionIo {
    fn observe(&mut self, _operation: NativeIoOperation) {}

    fn suspend_input(&mut self) -> NativeResult<()>;
    fn park_explicit_sync(&mut self) -> NativeResult<()>;
    fn quarantine_pageflip(&mut self) -> NativeResult<()>;
    fn unregister_drm_source(&mut self) -> NativeResult<()>;
    fn disable_hardware_cursor(&mut self) -> NativeResult<()>;
    fn recover_kms_pipeline(&mut self) -> NativeResult<()>;
    fn retire_quarantined_pageflip(&mut self) -> NativeResult<()>;
    fn rearm_explicit_sync(&mut self) -> NativeResult<()>;
    fn recover_hardware_cursor(&mut self) -> NativeResult<()>;
    fn register_drm_source(&mut self) -> NativeResult<()>;
    fn rearm_scheduler(&mut self) -> NativeResult<()>;
    fn resume_input(&mut self) -> NativeResult<()>;

    fn progress_wayland(&mut self) -> NativeResult<()>;
    fn discard_input(&mut self) -> NativeResult<()>;
    fn disarm_drm_destructors(&mut self);
}

pub(crate) fn quiesce_and_acknowledge<I: NativeSessionIo>(
    io: &mut I,
    acknowledge: impl FnOnce(&mut I) -> NativeResult<()>,
) -> NativeResult<()> {
    io.observe(NativeIoOperation::InputSuspend);
    io.suspend_input()?;
    io.observe(NativeIoOperation::ExplicitSyncPark);
    io.park_explicit_sync()?;
    io.observe(NativeIoOperation::PageflipQuarantine);
    io.quarantine_pageflip()?;
    io.observe(NativeIoOperation::DrmSourceUnregister);
    io.unregister_drm_source()?;
    io.observe(NativeIoOperation::HardwareCursorDrm);
    io.disable_hardware_cursor()?;
    acknowledge(io)
}

pub(crate) fn recover_native_output(io: &mut impl NativeSessionIo) -> NativeResult<()> {
    io.observe(NativeIoOperation::KmsRecovery);
    io.recover_kms_pipeline()?;
    io.observe(NativeIoOperation::PageflipRetire);
    io.retire_quarantined_pageflip()?;
    io.observe(NativeIoOperation::ExplicitSyncRearm);
    io.observe(NativeIoOperation::ExplicitSyncNotifier);
    io.rearm_explicit_sync()?;
    io.observe(NativeIoOperation::HardwareCursorDrm);
    io.recover_hardware_cursor()?;
    io.observe(NativeIoOperation::DrmSourceRegister);
    io.register_drm_source()?;
    io.observe(NativeIoOperation::SchedulerRearm);
    io.rearm_scheduler()?;
    io.observe(NativeIoOperation::InputResume);
    io.resume_input()
}

pub(crate) fn service_suspended_sources(
    io: &mut impl NativeSessionIo,
    readiness: NativeSuspendedReadiness,
) -> NativeResult<()> {
    if readiness.wayland {
        io.observe(NativeIoOperation::WaylandProgress);
        io.progress_wayland()?;
    }
    if readiness.input {
        io.observe(NativeIoOperation::RawInputDiscard);
        io.discard_input()?;
    }
    // DRM and explicit-sync sources are unregistered, timers are disarmed, and
    // redraw/cursor work remains queued in compositor state until recovery.
    let _parked = readiness.drm
        || readiness.timer
        || readiness.explicit_sync
        || readiness.redraw
        || readiness.cursor;
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NativeSuspendedReadiness {
    pub(crate) wayland: bool,
    pub(crate) input: bool,
    pub(crate) drm: bool,
    pub(crate) timer: bool,
    pub(crate) explicit_sync: bool,
    pub(crate) redraw: bool,
    pub(crate) cursor: bool,
}

pub(crate) fn teardown_without_drm_io(io: &mut impl NativeSessionIo) {
    io.observe(NativeIoOperation::DestructorDisarm);
    io.disarm_drm_destructors();
}

impl NativeSessionIo for NativeRuntime {
    #[cfg(test)]
    fn observe(&mut self, operation: NativeIoOperation) {
        self.native_io_recorder.record(operation);
    }

    fn suspend_input(&mut self) -> NativeResult<()> {
        self.input_devices.suspend_for_session();
        self.input_state.clear_pressed_state_for_session_switch();
        Ok(())
    }

    fn park_explicit_sync(&mut self) -> NativeResult<()> {
        self.parked_acquire_watches.extend(
            self.acquire_watches
                .park_for_session_suspend(&mut self.event_loop)?,
        );
        Ok(())
    }

    fn quarantine_pageflip(&mut self) -> NativeResult<()> {
        self.frame_scheduler.abandon_for_session_suspend();
        self.scanout.suspend_page_flip();
        Ok(())
    }

    fn unregister_drm_source(&mut self) -> NativeResult<()> {
        if let Some(token) = self.drm_reactor_token.take() {
            self.event_loop.unregister(token)?;
        }
        Ok(())
    }

    fn disable_hardware_cursor(&mut self) -> NativeResult<()> {
        if let Some(mut cursor) = self.hardware_cursor.take() {
            cursor.disable()?;
        }
        Ok(())
    }

    fn recover_kms_pipeline(&mut self) -> NativeResult<()> {
        let recovery = self.scanout.prepare_session_recovery()?;
        let framebuffer = recovery.framebuffer_id();
        self.kms_backend.recover(framebuffer)?;
        self.pending_session_recovery = Some(recovery);
        self.perf.log("native.session_recovery", || {
            vec![
                NativePerfField::str("completion", "synchronous_modeset"),
                NativePerfField::str("kms_backend", self.kms_backend.effective_kind().as_str()),
                NativePerfField::u64("framebuffer", u64::from(framebuffer.get())),
            ]
        });
        Ok(())
    }

    fn retire_quarantined_pageflip(&mut self) -> NativeResult<()> {
        let recovery = self.pending_session_recovery.as_ref().ok_or_else(|| {
            io::Error::other("session recovery completion has no prepared framebuffer")
        })?;
        self.scanout.complete_session_recovery(*recovery)?;
        self.pending_session_recovery = None;
        Ok(())
    }

    fn rearm_explicit_sync(&mut self) -> NativeResult<()> {
        self.drm_file_generation = allocate_native_drm_file_generation();
        self.scanout
            .rebind_session_generation(self.drm_file_generation);
        self.acquire_watches
            .set_drm_file_generation(self.drm_file_generation);
        self.rearm_parked_acquire_watches()
    }

    fn recover_hardware_cursor(&mut self) -> NativeResult<()> {
        if self.cursor_preference == NativeCursorPreference::Software {
            return Ok(());
        }
        match NativeHardwareCursor::create(self.kms.file(), self.target.crtc_id) {
            Ok(mut cursor) => {
                let (x, y) = self.input_state.cursor_position();
                cursor.enable().and_then(|()| cursor.move_to(x, y))?;
                self.hardware_cursor = Some(cursor);
                self.cursor_render_mode = NativeCursorRenderMode::Hardware;
            }
            Err(error) if self.cursor_preference == NativeCursorPreference::Hardware => {
                return Err(error.into());
            }
            Err(error) => {
                eprintln!("native cursor: session recovery fell back to software: {error}");
                self.cursor_render_mode = NativeCursorRenderMode::Software;
            }
        }
        Ok(())
    }

    fn register_drm_source(&mut self) -> NativeResult<()> {
        self.drm_reactor_token = Some(
            self.event_loop
                .register(self.kms.file().as_raw_fd(), NativeEventSource::Drm)?,
        );
        Ok(())
    }

    fn rearm_scheduler(&mut self) -> NativeResult<()> {
        self.event_loop.arm_deadline(earliest_native_deadline(
            self.frame_scheduler.next_deadline_ns(),
            self.acquire_watches.next_fallback_deadline_ns(),
        ))?;
        Ok(())
    }

    fn resume_input(&mut self) -> NativeResult<()> {
        self.input_devices.resume_after_session()?;
        Ok(())
    }

    fn progress_wayland(&mut self) -> NativeResult<()> {
        let _ = self.server.tick()?;
        Ok(())
    }

    fn discard_input(&mut self) -> NativeResult<()> {
        self.input_devices.discard_suspended_events();
        Ok(())
    }

    fn disarm_drm_destructors(&mut self) {
        self.scanout.disarm_drm_cleanup();
        self.kms_backend.disarm_drm_io();
        if let Some(cursor) = self.hardware_cursor.as_mut() {
            cursor.disarm_drm_cleanup();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, io, rc::Rc};

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Operation {
        SeatDispatch,
        InputSuspend,
        ExplicitSyncPark,
        PageflipQuarantine,
        DrmUnregister,
        CursorDisable,
        DisableAck,
        KmsRecovery,
        PageflipRetire,
        ExplicitSyncRearm,
        CursorRecover,
        DrmRegister,
        SchedulerRearm,
        InputResume,
        WaylandProgress,
        InputDiscard,
        DestructorDisarm,
        KmsRestore,
        AtomicCommit,
        LegacyCommit,
        PageflipDrain,
        PageflipSubmit,
        ScanoutPresent,
        ExplicitSyncNotifier,
        FramebufferRemove,
        RawInputAction,
    }

    #[derive(Default)]
    struct Recorder {
        operations: Vec<Operation>,
        native_io: NativeIoRecorder,
        recovery_fails: bool,
    }

    struct DestructionRecorder {
        armed: bool,
        forbidden: Rc<RefCell<[usize; 11]>>,
    }

    impl Drop for DestructionRecorder {
        fn drop(&mut self) {
            if self.armed {
                let mut counters = self.forbidden.borrow_mut();
                for counter in counters.iter_mut() {
                    *counter += 1;
                }
            }
        }
    }

    impl NativeSessionIo for DestructionRecorder {
        fn suspend_input(&mut self) -> NativeResult<()> {
            Ok(())
        }
        fn park_explicit_sync(&mut self) -> NativeResult<()> {
            Ok(())
        }
        fn quarantine_pageflip(&mut self) -> NativeResult<()> {
            Ok(())
        }
        fn unregister_drm_source(&mut self) -> NativeResult<()> {
            Ok(())
        }
        fn disable_hardware_cursor(&mut self) -> NativeResult<()> {
            Ok(())
        }
        fn recover_kms_pipeline(&mut self) -> NativeResult<()> {
            Ok(())
        }
        fn retire_quarantined_pageflip(&mut self) -> NativeResult<()> {
            Ok(())
        }
        fn rearm_explicit_sync(&mut self) -> NativeResult<()> {
            Ok(())
        }
        fn recover_hardware_cursor(&mut self) -> NativeResult<()> {
            Ok(())
        }
        fn register_drm_source(&mut self) -> NativeResult<()> {
            Ok(())
        }
        fn rearm_scheduler(&mut self) -> NativeResult<()> {
            Ok(())
        }
        fn resume_input(&mut self) -> NativeResult<()> {
            Ok(())
        }
        fn progress_wayland(&mut self) -> NativeResult<()> {
            Ok(())
        }
        fn discard_input(&mut self) -> NativeResult<()> {
            Ok(())
        }
        fn disarm_drm_destructors(&mut self) {
            self.armed = false;
        }
    }

    impl Recorder {
        fn push(&mut self, operation: Operation) {
            self.operations.push(operation);
        }

        fn forbidden_counts_after_ack(&self) -> [usize; 11] {
            let ack = self
                .operations
                .iter()
                .position(|operation| *operation == Operation::DisableAck)
                .expect("disable acknowledgment recorded");
            let forbidden = [
                Operation::KmsRecovery,
                Operation::KmsRestore,
                Operation::AtomicCommit,
                Operation::LegacyCommit,
                Operation::PageflipDrain,
                Operation::PageflipSubmit,
                Operation::ScanoutPresent,
                Operation::ExplicitSyncNotifier,
                Operation::FramebufferRemove,
                Operation::CursorDisable,
                Operation::CursorRecover,
            ];
            let mut counts = [0; 11];
            for operation in &self.operations[ack + 1..] {
                if let Some(index) = forbidden.iter().position(|item| item == operation) {
                    counts[index] += 1;
                }
            }
            counts
        }
    }

    impl NativeSessionIo for Recorder {
        fn observe(&mut self, operation: NativeIoOperation) {
            self.native_io.record(operation);
        }

        fn suspend_input(&mut self) -> NativeResult<()> {
            self.push(Operation::InputSuspend);
            Ok(())
        }
        fn park_explicit_sync(&mut self) -> NativeResult<()> {
            self.push(Operation::ExplicitSyncPark);
            Ok(())
        }
        fn quarantine_pageflip(&mut self) -> NativeResult<()> {
            self.push(Operation::PageflipQuarantine);
            Ok(())
        }
        fn unregister_drm_source(&mut self) -> NativeResult<()> {
            self.push(Operation::DrmUnregister);
            Ok(())
        }
        fn disable_hardware_cursor(&mut self) -> NativeResult<()> {
            self.push(Operation::CursorDisable);
            Ok(())
        }
        fn recover_kms_pipeline(&mut self) -> NativeResult<()> {
            self.push(Operation::KmsRecovery);
            if self.recovery_fails {
                Err(io::Error::other("injected recovery failure").into())
            } else {
                Ok(())
            }
        }
        fn retire_quarantined_pageflip(&mut self) -> NativeResult<()> {
            self.push(Operation::PageflipRetire);
            Ok(())
        }
        fn rearm_explicit_sync(&mut self) -> NativeResult<()> {
            self.push(Operation::ExplicitSyncRearm);
            Ok(())
        }
        fn recover_hardware_cursor(&mut self) -> NativeResult<()> {
            self.push(Operation::CursorRecover);
            Ok(())
        }
        fn register_drm_source(&mut self) -> NativeResult<()> {
            self.push(Operation::DrmRegister);
            Ok(())
        }
        fn rearm_scheduler(&mut self) -> NativeResult<()> {
            self.push(Operation::SchedulerRearm);
            Ok(())
        }
        fn resume_input(&mut self) -> NativeResult<()> {
            self.push(Operation::InputResume);
            Ok(())
        }
        fn progress_wayland(&mut self) -> NativeResult<()> {
            self.push(Operation::WaylandProgress);
            Ok(())
        }
        fn discard_input(&mut self) -> NativeResult<()> {
            self.push(Operation::InputDiscard);
            Ok(())
        }
        fn disarm_drm_destructors(&mut self) {
            self.push(Operation::DestructorDisarm);
        }
    }

    fn quiesce_without_seat(recorder: &mut Recorder) {
        recorder.push(Operation::SeatDispatch);
        quiesce_and_acknowledge(recorder, |io| {
            io.observe(NativeIoOperation::SeatDisableAcknowledged);
            io.push(Operation::DisableAck);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn disable_orders_quiesce_before_acknowledgment() {
        let mut recorder = Recorder::default();
        quiesce_without_seat(&mut recorder);

        assert_eq!(
            recorder.operations,
            vec![
                Operation::SeatDispatch,
                Operation::InputSuspend,
                Operation::ExplicitSyncPark,
                Operation::PageflipQuarantine,
                Operation::DrmUnregister,
                Operation::CursorDisable,
                Operation::DisableAck,
            ]
        );
    }

    #[test]
    fn suspended_wakeups_and_destruction_perform_zero_native_output_io() {
        let mut recorder = Recorder::default();
        quiesce_without_seat(&mut recorder);
        service_suspended_sources(
            &mut recorder,
            NativeSuspendedReadiness {
                wayland: true,
                input: true,
                drm: true,
                timer: true,
                explicit_sync: true,
                redraw: true,
                cursor: true,
            },
        )
        .unwrap();
        teardown_without_drm_io(&mut recorder);

        assert_eq!(recorder.forbidden_counts_after_ack(), [0; 11]);
        assert!(recorder.operations.contains(&Operation::WaylandProgress));
        assert!(recorder.operations.contains(&Operation::InputDiscard));
        assert_eq!(
            recorder.operations.last(),
            Some(&Operation::DestructorDisarm)
        );
        assert!(!recorder.operations.contains(&Operation::RawInputAction));

        let ack = recorder
            .native_io
            .operations()
            .iter()
            .position(|operation| *operation == NativeIoOperation::SeatDisableAcknowledged)
            .expect("native I/O recorder must observe disable acknowledgment");
        assert!(
            recorder.native_io.operations()[ack + 1..]
                .iter()
                .all(|operation| {
                    !matches!(
                        operation,
                        NativeIoOperation::KmsRecovery
                            | NativeIoOperation::KmsRestore
                            | NativeIoOperation::AtomicCommit
                            | NativeIoOperation::LegacyCommit
                            | NativeIoOperation::PageflipDrain
                            | NativeIoOperation::PageflipSubmit
                            | NativeIoOperation::ScanoutPresent
                            | NativeIoOperation::ExplicitSyncNotifier
                            | NativeIoOperation::HardwareCursorDrm
                    )
                })
        );
    }

    #[test]
    fn suspended_shutdown_disarms_all_forbidden_destructor_io() {
        let forbidden = Rc::new(RefCell::new([0; 11]));
        let mut resources = DestructionRecorder {
            armed: true,
            forbidden: Rc::clone(&forbidden),
        };

        teardown_without_drm_io(&mut resources);
        drop(resources);

        assert_eq!(*forbidden.borrow(), [0; 11]);
    }

    #[test]
    fn synchronous_recovery_precedes_input_resume() {
        let mut recorder = Recorder::default();
        recover_native_output(&mut recorder).unwrap();

        let recovery = recorder
            .operations
            .iter()
            .position(|op| *op == Operation::KmsRecovery)
            .unwrap();
        let resume = recorder
            .operations
            .iter()
            .position(|op| *op == Operation::InputResume)
            .unwrap();
        assert!(recovery < resume);
        assert!(!recorder.operations.contains(&Operation::PageflipQuarantine));
        assert!(!recorder.operations.contains(&Operation::PageflipSubmit));
    }

    #[test]
    fn recovery_failure_never_resumes_input_or_scheduler() {
        let mut recorder = Recorder {
            recovery_fails: true,
            ..Default::default()
        };
        assert!(recover_native_output(&mut recorder).is_err());

        assert!(!recorder.operations.contains(&Operation::InputResume));
        assert!(!recorder.operations.contains(&Operation::SchedulerRearm));
    }

    #[test]
    fn lifecycle_becomes_active_only_after_recorded_recovery_and_input_resume() {
        let mut lifecycle = NativeSessionLifecycle::default();
        let mut recorder = Recorder::default();
        lifecycle.begin_for_event(NativeSeatEvent::Disabled);
        quiesce_without_seat(&mut recorder);
        lifecycle.finish_suspend();
        lifecycle.begin_for_event(NativeSeatEvent::Enabled);

        recover_native_output(&mut recorder).unwrap();
        assert!(!lifecycle.permits_output());
        lifecycle.finish_resume();

        assert!(lifecycle.permits_output());
        assert_eq!(recorder.operations.last(), Some(&Operation::InputResume));
    }

    #[test]
    fn lifecycle_recovery_failure_remains_non_active_with_input_suspended() {
        let mut lifecycle = NativeSessionLifecycle::default();
        lifecycle.begin_for_event(NativeSeatEvent::Disabled);
        lifecycle.finish_suspend();
        lifecycle.begin_for_event(NativeSeatEvent::Enabled);
        let mut recorder = Recorder {
            recovery_fails: true,
            ..Default::default()
        };

        assert!(recover_native_output(&mut recorder).is_err());
        lifecycle.fail_resume();

        assert!(!lifecycle.permits_output());
        assert!(!recorder.operations.contains(&Operation::InputResume));
    }

    #[test]
    fn runtime_owned_lifecycle_is_independent_of_managed_input_backend_kind() {
        for (input_preference, input_kind) in [
            (
                NativeInputBackendPreference::LibseatLibinputUdev,
                NativeInputBackendKind::LibseatLibinputUdev,
            ),
            (
                NativeInputBackendPreference::DirectLibinputUdev,
                NativeInputBackendKind::DirectLibinputUdev,
            ),
            (
                NativeInputBackendPreference::RawEvdev,
                NativeInputBackendKind::RawEvdev,
            ),
        ] {
            let input_plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
                preference: input_preference,
                libseat_available: true,
                libinput_available: true,
                raw_evdev_available: true,
            });
            assert_eq!(input_plan.primary, input_kind);
            let drm_plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
                preference: NativeDrmBackendPreference::Auto,
                seat_available: true,
            });
            assert_eq!(drm_plan.primary, NativeDrmBackendKind::Libseat);
            assert!(drm_plan.fallbacks.is_empty());

            let mut recorder = Recorder::default();
            quiesce_without_seat(&mut recorder);
            assert!(
                recorder.operations.contains(&Operation::DisableAck),
                "managed lifecycle was not consumed for {}",
                input_kind.as_str()
            );
        }
    }

    #[test]
    fn native_io_recorder_preserves_cross_subsystem_operation_order() {
        let mut recorder = NativeIoRecorder::default();
        recorder.record(NativeIoOperation::SeatDispatch);
        recorder.record(NativeIoOperation::InputSuspend);
        recorder.record(NativeIoOperation::KmsRecovery);
        recorder.record(NativeIoOperation::InputResume);

        assert_eq!(
            recorder.operations(),
            &[
                NativeIoOperation::SeatDispatch,
                NativeIoOperation::InputSuspend,
                NativeIoOperation::KmsRecovery,
                NativeIoOperation::InputResume,
            ]
        );
    }
}
