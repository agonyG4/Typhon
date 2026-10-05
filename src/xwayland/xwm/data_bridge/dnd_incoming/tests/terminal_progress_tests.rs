use super::*;

#[test]
fn starting_exact_post_drop_transfer_renews_terminal_progress_lease() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, mut server_sequence) =
        accepted_drop(Action::Copy);
    let initial_deadline = match xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase {
        IncomingDndWirePhase::AwaitingWaylandFinish {
            deadline_ns,
            hard_deadline_ns,
            ..
        } => {
            assert_eq!(hard_deadline_ns, 600_000_000_041);
            deadline_ns
        }
        phase => panic!("expected dropped phase, got {phase:?}"),
    };
    assert_eq!(initial_deadline, 60_000_000_041);

    let (_reader, transfer_id, _, _) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        50_000_000_000,
    );
    assert_eq!(transfer_id.offer_id(), offer_id);
    let renewed_deadline = match xwm.data_bridge.dnd.incoming_session().unwrap().wire_phase {
        IncomingDndWirePhase::AwaitingWaylandFinish { deadline_ns, .. } => deadline_ns,
        phase => panic!("expected dropped phase, got {phase:?}"),
    };
    assert_eq!(renewed_deadline, 110_000_000_000);
}

#[test]
fn validated_selection_notify_and_direct_property_reply_renew_terminal_lease() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, mut server_sequence) =
        accepted_drop(Action::Copy);
    let (mut reader, transfer_id, requestor, property) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        50_000_000_000,
    );

    assert!(deliver_selection_notify(
        &mut xwm,
        requestor,
        mime_atom,
        property,
        timestamp,
        51_000_000_000,
    ));
    assert_eq!(terminal_deadlines(&xwm).0, 111_000_000_000);

    let sequence = transfer_pending_sequence(&xwm, transfer_id).unwrap();
    peer.write_all(&get_property_reply(
        sequence,
        mime_atom,
        8,
        b"direct payload",
    ))
    .unwrap();
    super::poll_replies(&mut xwm, 32, 52_000_000_000).unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 112_000_000_000);
    let mut payload = [0; 14];
    reader.read_exact(&mut payload).unwrap();
    assert_eq!(&payload, b"direct payload");
}

#[test]
fn validated_multi_read_property_continuation_renews_terminal_lease() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, mut server_sequence) =
        accepted_drop(Action::Copy);
    let (mut reader, transfer_id, requestor, property) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        50_000_000_000,
    );
    assert!(deliver_selection_notify(
        &mut xwm,
        requestor,
        mime_atom,
        property,
        timestamp,
        51_000_000_000,
    ));
    let first_sequence = transfer_pending_sequence(&xwm, transfer_id).unwrap();
    peer.write_all(&get_property_reply_with_bytes_after(
        first_sequence,
        mime_atom,
        8,
        b"abcd",
        4,
    ))
    .unwrap();
    super::poll_replies(&mut xwm, 32, 52_000_000_000).unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 112_000_000_000);
    assert_eq!(
        xwm.data_bridge.dnd_incoming.transfers[&transfer_id].offset_units,
        1
    );
    let continuation_sequence = transfer_pending_sequence(&xwm, transfer_id).unwrap();
    assert_ne!(continuation_sequence, first_sequence);

    peer.write_all(&get_property_reply(
        continuation_sequence,
        mime_atom,
        8,
        b"efgh",
    ))
    .unwrap();
    super::poll_replies(&mut xwm, 32, 53_000_000_000).unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 113_000_000_000);
    let mut payload = [0; 8];
    reader.read_exact(&mut payload).unwrap();
    assert_eq!(&payload, b"abcdefgh");
}

#[test]
fn only_positive_write_results_renew_the_terminal_lease() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, mut server_sequence) =
        accepted_drop(Action::Copy);
    let (_reader, transfer_id, _, _) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        50_000_000_000,
    );
    assert_eq!(terminal_deadlines(&xwm).0, 110_000_000_000);

    for (now_ns, kind) in [
        (51_000_000_000, std::io::ErrorKind::WouldBlock),
        (52_000_000_000, std::io::ErrorKind::Interrupted),
    ] {
        let error = super::super::super::terminal::note_terminal_transfer_write_result(
            &mut xwm,
            transfer_id,
            now_ns,
            Err(std::io::Error::from(kind)),
        )
        .unwrap_err();
        assert_eq!(error.kind(), kind);
        assert_eq!(terminal_deadlines(&xwm).0, 110_000_000_000);
    }

    super::super::super::terminal::note_terminal_transfer_write_result(
        &mut xwm,
        transfer_id,
        53_000_000_000,
        Ok(0),
    )
    .unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 110_000_000_000);

    super::super::super::terminal::note_terminal_transfer_write_result(
        &mut xwm,
        transfer_id,
        55_000_000_000,
        Ok(1),
    )
    .unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 115_000_000_000);
}

#[test]
fn valid_incr_progress_and_empty_terminator_keep_canonical_finish_available() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, mut server_sequence) =
        accepted_drop(Action::Copy);
    let (mut reader, transfer_id, requestor, property) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        50_000_000_000,
    );
    assert!(deliver_selection_notify(
        &mut xwm,
        requestor,
        mime_atom,
        property,
        timestamp,
        51_000_000_000,
    ));
    let incr = xwm.atoms.get(XwmAtomName::Incr);
    let marker_sequence = transfer_pending_sequence(&xwm, transfer_id).unwrap();
    peer.write_all(&get_property_reply(
        marker_sequence,
        incr,
        32,
        &16_u32.to_ne_bytes(),
    ))
    .unwrap();
    super::poll_replies(&mut xwm, 32, 52_000_000_000).unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 112_000_000_000);
    assert_eq!(
        xwm.data_bridge.dnd_incoming.transfers[&transfer_id].phase,
        IncomingTransferPhase::WaitingForIncrValue
    );

    let property_event = xproto::PropertyNotifyEvent {
        response_type: xproto::PROPERTY_NOTIFY_EVENT,
        sequence: 0,
        window: requestor,
        atom: property,
        time: 1,
        state: xproto::Property::NEW_VALUE,
    };
    assert!(
        super::super::super::property_notify(&mut xwm, property_event, 53_000_000_000).unwrap()
    );
    assert_eq!(terminal_deadlines(&xwm).0, 112_000_000_000);
    let chunk_sequence = transfer_pending_sequence(&xwm, transfer_id).unwrap();
    peer.write_all(&get_property_reply(chunk_sequence, mime_atom, 8, b"chunk"))
        .unwrap();
    super::poll_replies(&mut xwm, 32, 54_000_000_000).unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 114_000_000_000);
    let mut chunk = [0; 5];
    reader.read_exact(&mut chunk).unwrap();
    assert_eq!(&chunk, b"chunk");

    assert!(
        super::super::super::property_notify(&mut xwm, property_event, 55_000_000_000).unwrap()
    );
    assert_eq!(terminal_deadlines(&xwm).0, 114_000_000_000);
    let terminator_sequence = transfer_pending_sequence(&xwm, transfer_id).unwrap();
    peer.write_all(&get_property_reply(terminator_sequence, mime_atom, 8, &[]))
        .unwrap();
    super::poll_replies(&mut xwm, 32, 56_000_000_000).unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 116_000_000_000);
    assert!(!transfer_is_active(&xwm, transfer_id));
    assert!(xwm.data_bridge.dnd.incoming_session().is_some());

    super::super::super::apply_transition(
        &mut xwm,
        crate::xwayland::XwaylandDndTransition::SourceFinished {
            offer_id,
            accepted: true,
            action: Some(Action::Copy),
        },
        57_000_000_000,
    )
    .unwrap();
    let finished = finished_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(finished.len(), 1);
    assert_eq!(
        finished[0].1[1..3],
        [1, xwm.atoms.get(XwmAtomName::XdndActionCopy)]
    );
}

#[test]
fn selection_none_malformed_property_and_stale_reply_do_not_renew_terminal_lease() {
    let (mut none_xwm, mut none_peer, offer_id, mime_atom, timestamp, mut server_sequence) =
        accepted_drop(Action::Copy);
    let (_reader, transfer_id, requestor, _property) = begin_fake_selection_transfer_at(
        &mut none_xwm,
        &mut none_peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        50_000_000_000,
    );
    let selection_atom = none_xwm.atoms.get(XwmAtomName::XdndSelection);
    let none_event = selection_notify_event(
        requestor,
        selection_atom,
        mime_atom,
        u32::from(AtomEnum::NONE),
        timestamp,
    );
    super::super::super::selection_notify(&mut none_xwm, none_event, 55_000_000_000).unwrap();
    assert_eq!(terminal_deadlines(&none_xwm).0, 110_000_000_000);
    assert!(!transfer_is_active(&none_xwm, transfer_id));

    let (mut malformed_xwm, mut malformed_peer, offer_id, mime_atom, timestamp, mut sequence) =
        accepted_drop(Action::Copy);
    let (_reader, transfer_id, requestor, property) = begin_fake_selection_transfer_at(
        &mut malformed_xwm,
        &mut malformed_peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut sequence,
        50_000_000_000,
    );
    assert!(deliver_selection_notify(
        &mut malformed_xwm,
        requestor,
        mime_atom,
        property,
        timestamp,
        51_000_000_000,
    ));
    let property_sequence = transfer_pending_sequence(&malformed_xwm, transfer_id).unwrap();
    malformed_peer
        .write_all(&get_property_reply(
            property_sequence,
            u32::from(AtomEnum::NONE),
            8,
            b"malformed",
        ))
        .unwrap();
    super::poll_replies(&mut malformed_xwm, 32, 52_000_000_000).unwrap();
    assert_eq!(terminal_deadlines(&malformed_xwm).0, 111_000_000_000);
    assert!(!transfer_is_active(&malformed_xwm, transfer_id));

    let (mut stale_xwm, mut stale_peer, offer_id, mime_atom, timestamp, mut sequence) =
        accepted_drop(Action::Copy);
    let (_reader, transfer_id, requestor, property) = begin_fake_selection_transfer_at(
        &mut stale_xwm,
        &mut stale_peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut sequence,
        50_000_000_000,
    );
    assert!(deliver_selection_notify(
        &mut stale_xwm,
        requestor,
        mime_atom,
        property,
        timestamp,
        51_000_000_000,
    ));
    let property_sequence = transfer_pending_sequence(&stale_xwm, transfer_id).unwrap();
    stale_xwm
        .data_bridge
        .dnd_incoming
        .transfers
        .get_mut(&transfer_id)
        .unwrap()
        .pending_reply = Some(u64::from(property_sequence) + 1);
    stale_peer
        .write_all(&get_property_reply(
            property_sequence,
            mime_atom,
            8,
            b"stale reply identity",
        ))
        .unwrap();
    super::poll_replies(&mut stale_xwm, 32, 52_000_000_000).unwrap();
    assert_eq!(terminal_deadlines(&stale_xwm).0, 111_000_000_000);
}

#[test]
fn stale_offer_generation_and_requestor_cannot_renew_terminal_lease() {
    for identity in ["offer", "generation", "requestor"] {
        let (mut xwm, mut peer, offer_id, mime_atom, timestamp, mut server_sequence) =
            accepted_drop(Action::Copy);
        let (_reader, _transfer_id, requestor, property) = begin_fake_selection_transfer_at(
            &mut xwm,
            &mut peer,
            offer_id,
            mime_atom,
            timestamp,
            &mut server_sequence,
            50_000_000_000,
        );
        let transfer_id = xwm.data_bridge.dnd_incoming.requestors[&requestor];
        let event_requestor = if identity == "requestor" {
            requestor.wrapping_add(1)
        } else {
            requestor
        };
        if identity == "offer" {
            let foreign_offer = XwaylandDndOfferId::new(
                xwm.generation,
                std::num::NonZeroU64::new(offer_id.serial() + 1).unwrap(),
            );
            xwm.data_bridge
                .dnd_incoming
                .transfers
                .get_mut(&transfer_id)
                .unwrap()
                .offer_id = foreign_offer;
        } else if identity == "generation" {
            let stale_generation = crate::xwayland::XwaylandGeneration::new(
                std::num::NonZeroU64::new(xwm.generation.get() + 1).unwrap(),
            );
            xwm.data_bridge
                .dnd_incoming
                .transfers
                .get_mut(&transfer_id)
                .unwrap()
                .generation = stale_generation;
        }
        assert!(deliver_selection_notify(
            &mut xwm,
            event_requestor,
            mime_atom,
            property,
            timestamp,
            55_000_000_000,
        ));
        assert_eq!(
            terminal_deadlines(&xwm).0,
            110_000_000_000,
            "{identity} identity cannot move the current session deadline"
        );
    }
}

#[test]
fn sink_readiness_and_would_block_do_not_renew_but_positive_resume_does() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, mut server_sequence) =
        accepted_drop(Action::Copy);
    let (mut reader, transfer_id, requestor, property) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        50_000_000_000,
    );
    let generation = xwm.generation;
    assert!(deliver_selection_notify(
        &mut xwm,
        requestor,
        mime_atom,
        property,
        timestamp,
        51_000_000_000,
    ));
    xwm.data_bridge
        .dnd_incoming
        .transfers
        .get_mut(&transfer_id)
        .unwrap()
        .reactor_token = Some(0xC4);
    super::handle_sink_ready(
        &mut xwm,
        transfer_id,
        generation,
        0xC4,
        libc::EPOLLOUT as u32,
        52_000_000_000,
    )
    .unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 111_000_000_000);

    let sink_fd = xwm.data_bridge.dnd_incoming.transfers[&transfer_id]
        .sink
        .as_ref()
        .unwrap()
        .as_raw_fd();
    let send_buffer: libc::c_int = 4096;
    // SAFETY: setsockopt reads this live integer for the duration of the call.
    let set_buffer = unsafe {
        libc::setsockopt(
            sink_fd,
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            (&send_buffer as *const libc::c_int).cast(),
            std::mem::size_of_val(&send_buffer) as libc::socklen_t,
        )
    };
    assert_eq!(set_buffer, 0);
    let payload = vec![b'x'; 64 * 1024];
    let sequence = transfer_pending_sequence(&xwm, transfer_id).unwrap();
    peer.write_all(&get_property_reply(sequence, mime_atom, 8, &payload))
        .unwrap();
    super::poll_replies(&mut xwm, 32, 53_000_000_000).unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 113_000_000_000);
    assert!(xwm.data_bridge.dnd_incoming.transfers[&transfer_id].sink_writable_interest);

    super::handle_sink_ready(
        &mut xwm,
        transfer_id,
        generation,
        0xC4,
        libc::EPOLLOUT as u32,
        54_000_000_000,
    )
    .unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 113_000_000_000);
    let mut drained = [0; 16 * 1024];
    let drained_bytes = reader.read(&mut drained).unwrap();
    assert!(drained_bytes > 0);
    super::handle_sink_ready(
        &mut xwm,
        transfer_id,
        generation,
        0xC4,
        libc::EPOLLOUT as u32,
        55_000_000_000,
    )
    .unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 115_000_000_000);
}

#[test]
fn x_side_drain_after_wayland_sink_loss_does_not_renew_terminal_lease() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, mut server_sequence) =
        accepted_drop(Action::Copy);
    let (reader, transfer_id, requestor, property) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        50_000_000_000,
    );
    assert!(deliver_selection_notify(
        &mut xwm,
        requestor,
        mime_atom,
        property,
        timestamp,
        51_000_000_000,
    ));
    let incr = xwm.atoms.get(XwmAtomName::Incr);
    let marker_sequence = transfer_pending_sequence(&xwm, transfer_id).unwrap();
    peer.write_all(&get_property_reply(
        marker_sequence,
        incr,
        32,
        &32_u32.to_ne_bytes(),
    ))
    .unwrap();
    super::poll_replies(&mut xwm, 32, 52_000_000_000).unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 112_000_000_000);

    let property_event = xproto::PropertyNotifyEvent {
        response_type: xproto::PROPERTY_NOTIFY_EVENT,
        sequence: 0,
        window: requestor,
        atom: property,
        time: 1,
        state: xproto::Property::NEW_VALUE,
    };
    super::super::super::property_notify(&mut xwm, property_event, 53_000_000_000).unwrap();
    reader.shutdown(std::net::Shutdown::Read).unwrap();
    let chunk_sequence = transfer_pending_sequence(&xwm, transfer_id).unwrap();
    peer.write_all(&get_property_reply(
        chunk_sequence,
        mime_atom,
        8,
        b"sink gone",
    ))
    .unwrap();
    super::poll_replies(&mut xwm, 32, 54_000_000_000).unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 114_000_000_000);
    assert!(
        xwm.data_bridge.dnd_incoming.transfers[&transfer_id]
            .sink
            .is_none()
    );

    super::super::super::property_notify(&mut xwm, property_event, 55_000_000_000).unwrap();
    let drain_sequence = transfer_pending_sequence(&xwm, transfer_id).unwrap();
    peer.write_all(&get_property_reply(
        drain_sequence,
        mime_atom,
        8,
        b"drain only",
    ))
    .unwrap();
    super::poll_replies(&mut xwm, 32, 56_000_000_000).unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, 114_000_000_000);
}

#[test]
fn committed_transfer_progress_survives_root_authority_loss() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, mut server_sequence) =
        accepted_drop(Action::Copy);
    lose_root_proxy(&mut xwm, &mut peer, 50_000_000_000, 0x777);
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Lost { .. }
    ));
    let (_reader, transfer_id, _, _) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        55_000_000_000,
    );
    assert_eq!(transfer_id.offer_id(), offer_id);
    assert_eq!(terminal_deadlines(&xwm).0, 115_000_000_000);
}

#[test]
fn root_authority_loss_still_rejects_new_enter_admission() {
    let (mut xwm, mut peer, _, _, _, _, _) = fake_incoming_hover();
    lose_root_proxy(&mut xwm, &mut peer, 50_000_000_000, 0x777);
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    let enter = client_message(
        &xwm,
        xwm.root,
        XwmAtomName::XdndEnter,
        [0x994, 5 << 24, 0x554, 0, 0],
    );
    assert!(super::super::super::client_message(&mut xwm, enter, 55_000_000_000).unwrap());
    assert!(xwm.data_bridge.dnd.incoming_session().is_none());
    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Lost { .. }
    ));
}

#[test]
fn transfer_progress_after_canonical_cancel_keeps_the_one_second_fallback() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, mut server_sequence) =
        accepted_drop(Action::Copy);
    let (_reader, _transfer_id, requestor, property) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        50_000_000_000,
    );
    assert!(deliver_selection_notify(
        &mut xwm,
        requestor,
        mime_atom,
        property,
        timestamp,
        55_000_000_000,
    ));
    let progress_deadline = terminal_deadlines(&xwm).0;
    assert_eq!(progress_deadline, 115_000_000_000);
    super::super::super::expire_deadlines(&mut xwm, progress_deadline).unwrap();
    assert_eq!(
        xwm.data_bridge.dnd_incoming.take_events(),
        [XwaylandDndIncomingEvent::CancelAfterDrop { offer_id }]
    );
    let fallback = terminal_deadlines(&xwm).0;
    assert_eq!(
        fallback,
        progress_deadline + INCOMING_DND_DROP_ACK_TIMEOUT_NS
    );
    assert!(terminal_deadlines(&xwm).2);

    let (_late_reader, late_transfer, late_requestor, late_property) =
        begin_fake_selection_transfer_at(
            &mut xwm,
            &mut peer,
            offer_id,
            mime_atom,
            timestamp,
            &mut server_sequence,
            progress_deadline + 100_000_000,
        );
    assert!(deliver_selection_notify(
        &mut xwm,
        late_requestor,
        mime_atom,
        late_property,
        timestamp,
        progress_deadline + 200_000_000,
    ));
    let reply_sequence = transfer_pending_sequence(&xwm, late_transfer).unwrap();
    peer.write_all(&get_property_reply(
        reply_sequence,
        mime_atom,
        8,
        b"late payload",
    ))
    .unwrap();
    super::poll_replies(&mut xwm, 32, progress_deadline + 300_000_000).unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, fallback);
    super::super::super::resolve_cancel_after_drop(&mut xwm, offer_id, false).unwrap();
    assert!(finished_messages(&read_peer(&mut peer), &xwm).is_empty());
}

#[test]
fn processed_transfer_progress_precedes_same_cycle_terminal_expiration() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, _target_proxy, mut server_sequence) =
        fake_incoming_hover();
    let (_reader, _transfer_id, requestor, property) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        20,
    );
    assert!(accept_position(&mut xwm, &mut peer, offer_id, Action::Copy));
    submit_drop_and_ack(&mut xwm, offer_id, DROP_TIME, 40);
    let original_deadline = terminal_deadlines(&xwm).0;
    assert_eq!(original_deadline, 60_000_000_041);

    // The native cycle drains ready XWayland work before evaluating a timer
    // that became due in the same cycle.
    assert!(deliver_selection_notify(
        &mut xwm,
        requestor,
        mime_atom,
        property,
        timestamp,
        original_deadline,
    ));
    let renewed = terminal_deadlines(&xwm).0;
    assert_eq!(
        renewed,
        original_deadline + INCOMING_DND_TERMINAL_TIMEOUT_NS
    );
    super::super::super::expire_deadlines(&mut xwm, original_deadline).unwrap();
    assert_eq!(terminal_deadlines(&xwm).0, renewed);
    assert!(!terminal_deadlines(&xwm).2);
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());
}

#[test]
fn repeated_exact_progress_stops_at_hard_cap_and_fails_once_after_fallback() {
    let (mut xwm, mut peer, offer_id, mime_atom, timestamp, mut server_sequence) =
        accepted_drop(Action::Copy);
    let hard_deadline = terminal_deadlines(&xwm).1;
    assert_eq!(hard_deadline, 600_000_000_041);
    let mut progress_at = 50_000_000_000;
    while progress_at <= 572_000_000_000 {
        super::super::super::expire_deadlines(&mut xwm, progress_at).unwrap();
        let (_reader, transfer_id, _, _) = begin_fake_selection_transfer_at(
            &mut xwm,
            &mut peer,
            offer_id,
            mime_atom,
            timestamp,
            &mut server_sequence,
            progress_at,
        );
        assert_eq!(transfer_id.offer_id(), offer_id);
        assert_eq!(
            terminal_deadlines(&xwm).0,
            progress_at
                .saturating_add(INCOMING_DND_TERMINAL_TIMEOUT_NS)
                .min(hard_deadline)
        );
        progress_at += 29_000_000_000;
    }
    assert_eq!(terminal_deadlines(&xwm).0, hard_deadline);

    let (_reader, transfer_id, _, _) = begin_fake_selection_transfer_at(
        &mut xwm,
        &mut peer,
        offer_id,
        mime_atom,
        timestamp,
        &mut server_sequence,
        hard_deadline,
    );
    assert_eq!(transfer_id.offer_id(), offer_id);
    assert_eq!(terminal_deadlines(&xwm).0, hard_deadline);
    super::super::super::expire_deadlines(&mut xwm, hard_deadline).unwrap();
    assert_eq!(
        xwm.data_bridge.dnd_incoming.take_events(),
        [XwaylandDndIncomingEvent::CancelAfterDrop { offer_id }]
    );
    let fallback = terminal_deadlines(&xwm).0;
    assert_eq!(fallback, hard_deadline + INCOMING_DND_DROP_ACK_TIMEOUT_NS);
    assert_eq!(terminal_deadlines(&xwm).1, hard_deadline);
    super::super::super::expire_deadlines(&mut xwm, hard_deadline).unwrap();
    assert!(xwm.data_bridge.dnd_incoming.take_events().is_empty());

    super::super::super::expire_deadlines(&mut xwm, fallback).unwrap();
    let finished = finished_messages(&read_peer(&mut peer), &xwm);
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].1, [xwm.root, 0, 0, 0, 0]);
    super::super::super::expire_deadlines(&mut xwm, fallback + 1).unwrap();
    assert!(finished_messages(&read_peer(&mut peer), &xwm).is_empty());
}
