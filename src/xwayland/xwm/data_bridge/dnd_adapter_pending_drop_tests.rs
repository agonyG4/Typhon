use super::*;

#[test]
fn late_final_status_cannot_change_a_frozen_non_ask_drop_action() {
    for (generation_value, session_value, frozen_action, requested_action, status_action) in [
        (
            801,
            9801,
            XwaylandDndAction::Copy,
            XwaylandDndAction::Move,
            XwaylandDndAction::Move,
        ),
        (
            802,
            9802,
            XwaylandDndAction::Move,
            XwaylandDndAction::Move,
            XwaylandDndAction::Copy,
        ),
    ] {
        let generation = XwaylandGeneration::new(NonZeroU64::new(generation_value).unwrap());
        let (mut xwm, mut peer) = super::super::super::super::test_fixture_for_tests(generation);
        let id = identity(generation, session_value);
        let target = X11WindowHandle::new(generation, generation_value as u32 + 0x10);
        let recipient = target.xid() + 1;
        let source_proxy = target.xid() + 2;
        install_waiting_position(
            &mut xwm,
            id,
            target,
            recipient,
            source_proxy,
            requested_action,
        );
        xwm.flush().unwrap();
        let _position = read_peer(&mut peer);
        apply_transitions(&mut xwm, [drop_transition(id, target, frozen_action)], 100).unwrap();
        let pending = xwm.data_bridge.dnd.active_session().unwrap();
        assert_eq!(pending.pending_drop_action, Some(frozen_action));
        assert_eq!(pending.authorized_drop_action, None);

        inject_status_message(
            &mut xwm,
            &mut peer,
            source_proxy,
            target,
            true,
            status_action,
        );
        xwm.flush().unwrap();
        let messages = wire_client_messages(&read_peer(&mut peer));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].destination, recipient);
        assert_eq!(messages[0].window, target.xid());
        assert_eq!(messages[0].type_atom, xwm.atoms.get(XwmAtomName::XdndLeave));
        assert_eq!(
            xwm.data_bridge.dnd.take_feedback(),
            [
                DndFeedback::Status(DndStatusFeedback {
                    id,
                    target,
                    accepted: true,
                    action: Some(status_action),
                }),
                DndFeedback::Terminal(DndTerminalFeedback {
                    id,
                    target,
                    accepted: false,
                    action: None,
                }),
            ]
        );
        assert_eq!(
            xwm.data_bridge.dnd.active_session().unwrap().progress,
            DndWireProgress::TerminalConsumed
        );
    }
}

#[test]
fn late_matching_copy_or_move_status_authorizes_drop() {
    for (generation_value, session_value, action) in [
        (803, 9803, XwaylandDndAction::Copy),
        (804, 9804, XwaylandDndAction::Move),
    ] {
        let generation = XwaylandGeneration::new(NonZeroU64::new(generation_value).unwrap());
        let (mut xwm, mut peer) = super::super::super::super::test_fixture_for_tests(generation);
        let id = identity(generation, session_value);
        let target = X11WindowHandle::new(generation, generation_value as u32 + 0x10);
        let recipient = target.xid() + 1;
        let source_proxy = target.xid() + 2;
        install_waiting_position(&mut xwm, id, target, recipient, source_proxy, action);
        xwm.flush().unwrap();
        let _position = read_peer(&mut peer);
        apply_transitions(&mut xwm, [drop_transition(id, target, action)], 100).unwrap();
        inject_status_message(&mut xwm, &mut peer, source_proxy, target, true, action);
        xwm.flush().unwrap();

        let messages = wire_client_messages(&read_peer(&mut peer));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].destination, recipient);
        assert_eq!(messages[0].window, target.xid());
        assert_eq!(messages[0].type_atom, xwm.atoms.get(XwmAtomName::XdndDrop));
        let session = xwm.data_bridge.dnd.active_session().unwrap();
        assert_eq!(session.pending_drop_action, Some(action));
        assert_eq!(session.authorized_drop_action, Some(action));
        assert_eq!(session.progress, DndWireProgress::AwaitingFinished);
        assert_eq!(
            xwm.data_bridge.dnd.take_feedback(),
            [DndFeedback::Status(DndStatusFeedback {
                id,
                target,
                accepted: true,
                action: Some(action),
            })]
        );

        let stale_action = if action == XwaylandDndAction::Copy {
            XwaylandDndAction::Move
        } else {
            XwaylandDndAction::Copy
        };
        inject_status_message(
            &mut xwm,
            &mut peer,
            source_proxy,
            target,
            true,
            stale_action,
        );
        xwm.flush().unwrap();
        assert!(wire_client_messages(&read_peer(&mut peer)).is_empty());
        assert!(xwm.data_bridge.dnd.take_feedback().is_empty());
        let session = xwm.data_bridge.dnd.active_session().unwrap();
        assert_eq!(session.pending_drop_action, Some(action));
        assert_eq!(session.authorized_drop_action, Some(action));
    }
}

#[test]
fn late_ask_status_can_authorize_final_copy_or_move() {
    for (generation_value, session_value, status_action, performed_action) in [
        (805, 9805, XwaylandDndAction::Ask, XwaylandDndAction::Copy),
        (806, 9806, XwaylandDndAction::Ask, XwaylandDndAction::Move),
        (807, 9807, XwaylandDndAction::Copy, XwaylandDndAction::Copy),
    ] {
        let generation = XwaylandGeneration::new(NonZeroU64::new(generation_value).unwrap());
        let (mut xwm, mut peer) = super::super::super::super::test_fixture_for_tests(generation);
        let id = identity(generation, session_value);
        let target = X11WindowHandle::new(generation, generation_value as u32 + 0x10);
        let recipient = target.xid() + 1;
        let source_proxy = target.xid() + 2;
        let source_actions = vec![
            XwaylandDndAction::Copy,
            XwaylandDndAction::Move,
            XwaylandDndAction::Ask,
        ];
        install_waiting_position_with_actions(
            &mut xwm,
            id,
            target,
            recipient,
            source_proxy,
            XwaylandDndAction::Ask,
            source_actions,
        );
        xwm.flush().unwrap();
        let _position = read_peer(&mut peer);
        apply_transitions(
            &mut xwm,
            [drop_transition(id, target, XwaylandDndAction::Ask)],
            100,
        )
        .unwrap();
        inject_status_message(
            &mut xwm,
            &mut peer,
            source_proxy,
            target,
            true,
            status_action,
        );
        xwm.flush().unwrap();
        let drop_messages = wire_client_messages(&read_peer(&mut peer));
        assert_eq!(drop_messages.len(), 1);
        assert_eq!(drop_messages[0].destination, recipient);
        assert_eq!(
            drop_messages[0].type_atom,
            xwm.atoms.get(XwmAtomName::XdndDrop)
        );
        let session = xwm.data_bridge.dnd.active_session().unwrap();
        assert_eq!(session.pending_drop_action, Some(XwaylandDndAction::Ask));
        assert_eq!(session.authorized_drop_action, Some(status_action));
        assert_eq!(
            xwm.data_bridge.dnd.take_feedback(),
            [DndFeedback::Status(DndStatusFeedback {
                id,
                target,
                accepted: true,
                action: Some(status_action),
            })]
        );

        inject_finished_message(
            &mut xwm,
            &mut peer,
            source_proxy,
            target,
            true,
            Some(performed_action),
        );
        assert_eq!(
            xwm.data_bridge.dnd.take_feedback(),
            [DndFeedback::Terminal(DndTerminalFeedback {
                id,
                target,
                accepted: true,
                action: Some(performed_action),
            })]
        );
    }
}

#[test]
fn late_ask_status_requires_v5_before_drop() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(808).unwrap());
    let (mut xwm, mut peer) = super::super::super::super::test_fixture_for_tests(generation);
    let id = identity(generation, 9808);
    let target = X11WindowHandle::new(generation, 0x818);
    let recipient = 0x819;
    let source_proxy = 0x81a;
    install_waiting_position_with_actions(
        &mut xwm,
        id,
        target,
        recipient,
        source_proxy,
        XwaylandDndAction::Ask,
        vec![
            XwaylandDndAction::Copy,
            XwaylandDndAction::Move,
            XwaylandDndAction::Ask,
        ],
    );
    xwm.data_bridge.dnd.active.as_mut().unwrap().target_version =
        Some(XwaylandDndVersion::new(4).unwrap());
    xwm.flush().unwrap();
    let _position = read_peer(&mut peer);
    apply_transitions(
        &mut xwm,
        [drop_transition(id, target, XwaylandDndAction::Ask)],
        100,
    )
    .unwrap();
    inject_status_message(
        &mut xwm,
        &mut peer,
        source_proxy,
        target,
        true,
        XwaylandDndAction::Ask,
    );
    xwm.flush().unwrap();

    let messages = wire_client_messages(&read_peer(&mut peer));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].destination, recipient);
    assert_eq!(messages[0].window, target.xid());
    assert_eq!(messages[0].type_atom, xwm.atoms.get(XwmAtomName::XdndLeave));
    assert_eq!(
        xwm.data_bridge.dnd.take_feedback(),
        [
            DndFeedback::Status(DndStatusFeedback {
                id,
                target,
                accepted: true,
                action: Some(XwaylandDndAction::Ask),
            }),
            DndFeedback::Terminal(DndTerminalFeedback {
                id,
                target,
                accepted: false,
                action: None,
            }),
        ]
    );
}
