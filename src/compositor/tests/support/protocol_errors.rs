use super::super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::compositor::tests) struct ProtocolErrorObservation {
    pub(in crate::compositor::tests) code: u32,
    pub(in crate::compositor::tests) object_id: u32,
    pub(in crate::compositor::tests) object_interface: String,
    pub(in crate::compositor::tests) message: String,
}

pub(in crate::compositor::tests) fn expect_protocol_error(
    connection: &Connection,
    expected_interface: &str,
    expected_code: u32,
) -> ProtocolErrorObservation {
    let read_result = loop {
        if let Some(guard) = connection.prepare_read() {
            break guard.read();
        }
        if connection
            .backend()
            .dispatch_inner_queue()
            .expect("pending client events must be dispatchable")
            > 0
        {
            continue;
        }
    };
    assert!(
        read_result.is_err(),
        "the invalid request must terminate the Wayland connection while reading the wire error"
    );

    let error = connection
        .protocol_error()
        .expect("the connection should retain the server protocol error");
    assert_eq!(error.object_interface, expected_interface);
    assert_eq!(error.code, expected_code);

    ProtocolErrorObservation {
        code: error.code,
        object_id: error.object_id,
        object_interface: error.object_interface,
        message: error.message,
    }
}

pub(in crate::compositor::tests) fn expect_request_ignored_without_disconnect(
    connection: &Connection,
    request: impl FnOnce(),
) -> usize {
    request();
    connection
        .flush()
        .expect("an intentionally ignored request must still flush");
    connection
        .roundtrip()
        .expect("an intentionally ignored request must not disconnect the client")
}

pub(in crate::compositor::tests) fn expect_roundtrip_alive(connection: &Connection) -> usize {
    connection
        .roundtrip()
        .expect("the unaffected client must remain connected")
}

pub(in crate::compositor::tests) fn expect_object_destroy_order_error(
    connection: &Connection,
    expected_interface: &str,
    expected_code: u32,
) -> ProtocolErrorObservation {
    expect_protocol_error(connection, expected_interface, expected_code)
}

pub(in crate::compositor::tests) fn expect_client_state_scrubbed(
    server: &OwnCompositorServer,
    removed_surface_ids: &[u32],
) {
    for surface_id in removed_surface_ids {
        assert!(!server.state.surface_resources.contains_key(surface_id));
        assert!(!server.state.surface_client_ids.contains_key(surface_id));
        assert!(
            !server
                .state
                .current_surface_buffers
                .contains_key(surface_id)
        );
        assert!(
            !server
                .state
                .renderable_surfaces
                .iter()
                .any(|surface| { surface.surface_id == *surface_id })
        );
    }
}
