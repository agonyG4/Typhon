use super::*;
use crate::{
    native_output::input::NativeInputApplication,
    system_action_transport::{NativeSystemActionTransport, SystemActionSubmitResult},
};

impl NativeRuntime {
    pub(super) fn service_system_action_transport(
        &mut self,
        cycle: &NativeCycleState,
        now_ns: u64,
    ) {
        let previous = self.system_action_transport.snapshot();
        self.system_action_transport.service(
            &mut self.event_loop,
            &cycle.wakeup.system_action_events,
            now_ns,
        );
        let capabilities_changed =
            apply_capability_update(&mut self.system_action_transport, &mut self.input_state);
        let current = self.system_action_transport.snapshot();
        if capabilities_changed
            || previous.peer_connected != current.peer_connected
            || previous.protocol_ready != current.protocol_ready
        {
            log_transport_snapshot(
                self.perf,
                current,
                self.input_state.system_action_capability_count(),
            );
        }
    }

    pub(super) fn quiesce_system_action_transport(&mut self) {
        self.system_action_transport.shutdown(&mut self.event_loop);
        if apply_capability_update(&mut self.system_action_transport, &mut self.input_state) {
            log_transport_snapshot(
                self.perf,
                self.system_action_transport.snapshot(),
                self.input_state.system_action_capability_count(),
            );
        }
        self.input_state.set_system_action_capabilities(
            crate::system_action::AstreaSystemActionCapabilities::EMPTY,
        );
    }
}

pub(super) fn submit_input_application_system_actions(
    transport: &mut NativeSystemActionTransport,
    event_loop: &mut NativeEventLoop,
    input_state: &mut NativeInputState,
    application: &NativeInputApplication,
    perf: NativePerfLogger,
) {
    if application.system_action_overflow || application.system_actions.overflowed() {
        eprintln!(
            "native system-action input batch exceeded its fixed capacity; the batch was discarded"
        );
        return;
    }
    for action in application.system_actions.iter() {
        if transport.submit(event_loop, action) == SystemActionSubmitResult::Disconnected {
            if apply_capability_update(transport, input_state) {
                log_transport_snapshot(
                    perf,
                    transport.snapshot(),
                    input_state.system_action_capability_count(),
                );
            }
            break;
        }
    }
    if apply_capability_update(transport, input_state) {
        log_transport_snapshot(
            perf,
            transport.snapshot(),
            input_state.system_action_capability_count(),
        );
    }
}

fn apply_capability_update(
    transport: &mut NativeSystemActionTransport,
    input_state: &mut NativeInputState,
) -> bool {
    if let Some(capabilities) = transport.take_capability_update() {
        input_state.set_system_action_capabilities(capabilities)
    } else {
        false
    }
}

fn log_transport_snapshot(
    perf: NativePerfLogger,
    snapshot: crate::system_action_transport::SystemActionTransportSnapshot,
    accepted_capability_count: u32,
) {
    perf.log("native.system_action.transport", || {
        vec![
            NativePerfField::bool("peer_connected", snapshot.peer_connected),
            NativePerfField::bool("protocol_ready", snapshot.protocol_ready),
            NativePerfField::u64(
                "accepted_capabilities",
                u64::from(accepted_capability_count),
            ),
            NativePerfField::usize("outbound_queue_depth", snapshot.outbound_queue_depth),
            NativePerfField::u64("listener_accepts", snapshot.telemetry.listener_accepts),
            NativePerfField::u64("peer_rejections", snapshot.telemetry.peer_rejections),
            NativePerfField::u64(
                "handshake_successes",
                snapshot.telemetry.handshake_successes,
            ),
            NativePerfField::u64("handshake_failures", snapshot.telemetry.handshake_failures),
            NativePerfField::u64("peer_disconnects", snapshot.telemetry.peer_disconnects),
            NativePerfField::u64("capability_updates", snapshot.telemetry.capability_updates),
            NativePerfField::u64("actions_sent", snapshot.telemetry.actions_sent),
            NativePerfField::u64(
                "action_occurrences_sent",
                snapshot.telemetry.action_occurrences_sent,
            ),
            NativePerfField::u64(
                "action_records_coalesced",
                snapshot.telemetry.action_records_coalesced,
            ),
            NativePerfField::u64("send_would_block", snapshot.telemetry.send_would_block),
            NativePerfField::u64("queue_overflows", snapshot.telemetry.queue_overflows),
            NativePerfField::u64("protocol_errors", snapshot.telemetry.protocol_errors),
        ]
    });
}
