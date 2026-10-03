//! Typed helpers for the XDND wire surface shared by the Wayland source and
//! incoming XWayland target bridge.

use crate::xwayland::{WaylandDndAction, XwaylandDndAction};
use x11rb::protocol::xproto::{self, Atom, ClientMessageData, Window};

use super::super::atoms::{XwmAtomName, XwmAtoms};

pub(crate) const XDND_VERSION_SHIFT: u32 = 24;
pub(crate) const XDND_MORE_TYPES: u32 = 1;
pub(crate) const XDND_STATUS_ACCEPTED: u32 = 1;
pub(crate) const XDND_FINISHED_ACCEPTED: u32 = 1;

/// Decoded fields from an incoming five-word XDND Position message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DecodedXdndPosition {
    pub(crate) source: Window,
    pub(crate) state: u32,
    pub(crate) root_x: i16,
    pub(crate) root_y: i16,
    pub(crate) timestamp: u32,
    pub(crate) action_atom: Atom,
}

pub(crate) fn decode_xdnd_position(data: [u32; 5]) -> DecodedXdndPosition {
    let (root_x, root_y) = crate::xwayland::unpack_root_coordinates(data[2]);
    DecodedXdndPosition {
        source: data[0],
        state: data[1],
        root_x: root_x as i16,
        root_y: root_y as i16,
        timestamp: data[3],
        action_atom: data[4],
    }
}

/// Pack root coordinates as signed 16-bit values. XDND's fields carry the
/// root-space X11 coordinates, so fractional compositor values are rounded
/// to the nearest pixel and values outside the protocol range are clamped.
pub(crate) fn pack_root_coordinates(x: f64, y: f64) -> Option<u32> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let x = x.round().clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
    let y = y.round().clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
    Some((u32::from(x as u16) << 16) | u32::from(y as u16))
}

pub(crate) const fn requested_action(actions: &[XwaylandDndAction]) -> Option<XwaylandDndAction> {
    let mut index = 0;
    let mut copy = false;
    let mut move_action = false;
    let mut ask = false;
    while index < actions.len() {
        match actions[index] {
            XwaylandDndAction::Copy => copy = true,
            XwaylandDndAction::Move => move_action = true,
            XwaylandDndAction::Ask => ask = true,
            XwaylandDndAction::Link | XwaylandDndAction::Private => {}
        }
        index += 1;
    }
    if copy {
        Some(XwaylandDndAction::Copy)
    } else if move_action {
        Some(XwaylandDndAction::Move)
    } else if ask {
        Some(XwaylandDndAction::Ask)
    } else {
        None
    }
}

pub(crate) fn wayland_action_atom(atoms: &XwmAtoms, action: XwaylandDndAction) -> Option<Atom> {
    match action {
        XwaylandDndAction::Copy => Some(atoms.get(XwmAtomName::XdndActionCopy)),
        XwaylandDndAction::Move => Some(atoms.get(XwmAtomName::XdndActionMove)),
        XwaylandDndAction::Ask => Some(atoms.get(XwmAtomName::XdndActionAsk)),
        XwaylandDndAction::Link | XwaylandDndAction::Private => None,
    }
}

pub(crate) fn action_from_atom(atoms: &XwmAtoms, atom: Atom) -> Option<XwaylandDndAction> {
    if atom == atoms.get(XwmAtomName::XdndActionCopy) {
        Some(XwaylandDndAction::Copy)
    } else if atom == atoms.get(XwmAtomName::XdndActionMove) {
        Some(XwaylandDndAction::Move)
    } else if atom == atoms.get(XwmAtomName::XdndActionAsk) {
        Some(XwaylandDndAction::Ask)
    } else if atom == atoms.get(XwmAtomName::XdndActionLink) {
        Some(XwaylandDndAction::Link)
    } else if atom == atoms.get(XwmAtomName::XdndActionPrivate) {
        Some(XwaylandDndAction::Private)
    } else {
        None
    }
}

pub(crate) fn decode_status_action(
    accepted_bit: bool,
    action_atom: Atom,
    requested_action: Option<XwaylandDndAction>,
    source_actions: &[XwaylandDndAction],
    atoms: &XwmAtoms,
) -> (bool, Option<XwaylandDndAction>) {
    let action = accepted_bit
        .then(|| action_from_atom(atoms, action_atom))
        .flatten()
        .filter(|action| action.to_wayland_action().is_some() && source_actions.contains(action))
        .filter(|action| {
            matches!(
                (requested_action, *action),
                (Some(XwaylandDndAction::Copy), XwaylandDndAction::Copy)
                    | (
                        Some(XwaylandDndAction::Move),
                        XwaylandDndAction::Move | XwaylandDndAction::Copy
                    )
                    | (
                        Some(XwaylandDndAction::Ask),
                        XwaylandDndAction::Ask | XwaylandDndAction::Copy
                    )
            )
        });
    let accepted = accepted_bit && action.is_some();
    (accepted, accepted.then_some(action).flatten())
}

pub(crate) struct XdndDropFields {
    pub(crate) actual_target: u32,
    pub(crate) source_proxy: u32,
    pub(crate) timestamp: u32,
    pub(crate) drop_atom: Atom,
}

pub(crate) fn encode_xdnd_drop(fields: XdndDropFields) -> xproto::ClientMessageEvent {
    xproto::ClientMessageEvent {
        response_type: xproto::CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window: fields.actual_target,
        type_: fields.drop_atom,
        data: ClientMessageData::from([fields.source_proxy, 0, fields.timestamp, 0, 0]),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct XdndFinishedResult {
    pub(crate) accepted: bool,
    pub(crate) action: Option<XwaylandDndAction>,
}

pub(crate) fn decode_xdnd_finished(
    data: [u32; 5],
    version: u8,
    accepted_status_action: XwaylandDndAction,
    source_actions: &[XwaylandDndAction],
    atoms: &XwmAtoms,
) -> XdndFinishedResult {
    if version < 5 {
        let action_is_concrete = matches!(
            accepted_status_action,
            XwaylandDndAction::Copy | XwaylandDndAction::Move
        );
        let accepted = action_is_concrete && source_actions.contains(&accepted_status_action);
        return XdndFinishedResult {
            accepted,
            action: accepted.then_some(accepted_status_action),
        };
    }

    if data[1] & XDND_FINISHED_ACCEPTED == 0 {
        return XdndFinishedResult {
            accepted: false,
            action: None,
        };
    }
    let action = action_from_atom(atoms, data[2])
        .filter(|action| matches!(action, XwaylandDndAction::Copy | XwaylandDndAction::Move))
        .filter(|action| source_actions.contains(action));
    let action_matches_status = match accepted_status_action {
        XwaylandDndAction::Copy | XwaylandDndAction::Move => action == Some(accepted_status_action),
        XwaylandDndAction::Ask => action.is_some(),
        XwaylandDndAction::Link | XwaylandDndAction::Private => false,
    };
    let action = action.filter(|_| action_matches_status);
    XdndFinishedResult {
        accepted: action.is_some(),
        action,
    }
}

pub(crate) fn validated_proxy(
    actual: u32,
    advertised_proxy: Option<u32>,
    proxy_self_reference: Option<u32>,
) -> u32 {
    match advertised_proxy {
        Some(proxy) if proxy != 0 && proxy_self_reference == Some(proxy) => proxy,
        _ => actual,
    }
}

pub(crate) fn selection_targets(
    targets_atom: u32,
    timestamp_atom: u32,
    multiple_atom: u32,
    mime_atoms: &[u32],
) -> Vec<u32> {
    let mut targets = Vec::with_capacity(3 + mime_atoms.len());
    targets.extend([targets_atom, timestamp_atom, multiple_atom]);
    targets.extend_from_slice(mime_atoms);
    targets
}

pub(crate) struct EnterPositionFields<'a> {
    pub(crate) actual_target: u32,
    pub(crate) source_proxy: u32,
    pub(crate) version: u8,
    pub(crate) mime_atoms: &'a [u32],
    pub(crate) coordinates: u32,
    pub(crate) timestamp: u32,
    pub(crate) action_atom: u32,
    pub(crate) enter_atom: u32,
    pub(crate) position_atom: u32,
}

pub(crate) fn encode_enter_and_initial_position(
    fields: EnterPositionFields<'_>,
) -> [xproto::ClientMessageEvent; 2] {
    let EnterPositionFields {
        actual_target,
        source_proxy,
        version,
        mime_atoms,
        coordinates,
        timestamp,
        action_atom,
        enter_atom,
        position_atom,
    } = fields;
    let mut types = [0; 3];
    for (slot, atom) in types.iter_mut().zip(mime_atoms.iter().take(3)) {
        *slot = *atom;
    }
    let more_types = if mime_atoms.len() > 3 {
        XDND_MORE_TYPES
    } else {
        0
    };
    [
        xproto::ClientMessageEvent {
            response_type: xproto::CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window: actual_target,
            type_: enter_atom,
            data: ClientMessageData::from([
                source_proxy,
                (u32::from(version) << XDND_VERSION_SHIFT) | more_types,
                types[0],
                types[1],
                types[2],
            ]),
        },
        xproto::ClientMessageEvent {
            response_type: xproto::CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window: actual_target,
            type_: position_atom,
            data: ClientMessageData::from([source_proxy, 0, coordinates, timestamp, action_atom]),
        },
    ]
}

pub(crate) fn wayland_actions(actions: &[XwaylandDndAction]) -> Vec<WaylandDndAction> {
    let mut result = Vec::with_capacity(3);
    for action in [
        WaylandDndAction::Copy,
        WaylandDndAction::Move,
        WaylandDndAction::Ask,
    ] {
        if actions
            .iter()
            .any(|candidate| candidate.to_wayland_action() == Some(action))
        {
            result.push(action);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn action_atoms() -> XwmAtoms {
        XwmAtoms::from_values(HashMap::from([
            (XwmAtomName::XdndActionCopy, 11),
            (XwmAtomName::XdndActionMove, 12),
            (XwmAtomName::XdndActionLink, 13),
            (XwmAtomName::XdndActionAsk, 14),
            (XwmAtomName::XdndActionPrivate, 15),
        ]))
    }

    #[test]
    fn root_coordinates_round_and_clamp_in_signed_x11_space() {
        assert_eq!(pack_root_coordinates(1.4, -2.6), Some(0x0001_fffd));
        assert_eq!(pack_root_coordinates(-40000.0, 40000.0), Some(0x8000_7fff));
        assert_eq!(pack_root_coordinates(f64::NAN, 0.0), None);
    }

    #[test]
    fn xdnd_position_decoder_uses_each_protocol_field_exactly() {
        let atoms = action_atoms();
        let source = 0x71a5_2400;
        let state = 0x05a3;
        let coordinates = pack_root_coordinates(-123.0, 456.0).unwrap();
        let timestamp = 0x8091_a2b3;
        let action_atom = atoms.get(XwmAtomName::XdndActionMove);
        let fields = [source, state, coordinates, timestamp, action_atom];
        for left in 0..fields.len() {
            assert!(
                fields[left + 1..]
                    .iter()
                    .all(|right| *right != fields[left])
            );
        }

        assert_eq!(
            decode_xdnd_position(fields),
            DecodedXdndPosition {
                source,
                state,
                root_x: -123,
                root_y: 456,
                timestamp,
                action_atom,
            }
        );
    }

    #[test]
    fn xdnd_position_modifier_state_does_not_change_other_fields() {
        let atoms = action_atoms();
        let position = [
            0x71a5_2400,
            0x05a3,
            pack_root_coordinates(-123.0, -456.0).unwrap(),
            0x8091_a2b3,
            atoms.get(XwmAtomName::XdndActionMove),
        ];
        let first = decode_xdnd_position(position);
        let mut changed_state = position;
        changed_state[1] = 0x0137;
        let second = decode_xdnd_position(changed_state);

        assert_eq!(first.state, 0x05a3);
        assert_eq!(second.state, 0x0137);
        assert_eq!((first.root_x, first.root_y), (-123, -456));
        assert_eq!((second.root_x, second.root_y), (-123, -456));
        assert_eq!(first.timestamp, second.timestamp);
        assert_eq!(first.action_atom, second.action_atom);
    }

    #[test]
    fn xdnd_position_action_is_decoded_only_from_data_four() {
        let atoms = action_atoms();
        let known_action = decode_xdnd_position([
            0x71a5_2400,
            0x05a3,
            pack_root_coordinates(-123.0, 456.0).unwrap(),
            0x8091_a2b3,
            atoms.get(XwmAtomName::XdndActionMove),
        ]);
        assert_eq!(
            action_from_atom(&atoms, known_action.action_atom),
            Some(XwaylandDndAction::Move)
        );

        let unknown_action = decode_xdnd_position([
            0x71a5_2400,
            0x05a3,
            pack_root_coordinates(-123.0, 456.0).unwrap(),
            atoms.get(XwmAtomName::XdndActionCopy),
            0xfeed_cafe,
        ]);
        assert_eq!(
            action_from_atom(&atoms, unknown_action.action_atom),
            None,
            "a known action atom in timestamp data[3] cannot override unknown data[4]"
        );
    }

    #[test]
    fn xdnd_position_decoder_preserves_signed_coordinate_boundaries() {
        let atoms = action_atoms();
        let negative = decode_xdnd_position([
            1,
            0,
            pack_root_coordinates(i16::MIN.into(), (-1_i16).into()).unwrap(),
            2,
            atoms.get(XwmAtomName::XdndActionCopy),
        ]);
        assert_eq!((negative.root_x, negative.root_y), (i16::MIN, -1));

        let positive = decode_xdnd_position([
            1,
            0,
            pack_root_coordinates(i16::MAX.into(), i16::MAX.into()).unwrap(),
            2,
            atoms.get(XwmAtomName::XdndActionCopy),
        ]);
        assert_eq!((positive.root_x, positive.root_y), (i16::MAX, i16::MAX));
    }

    #[test]
    fn requested_action_uses_only_the_deterministic_wayland_domain() {
        assert_eq!(
            requested_action(&[XwaylandDndAction::Move]),
            Some(XwaylandDndAction::Move)
        );
        assert_eq!(
            requested_action(&[XwaylandDndAction::Ask]),
            Some(XwaylandDndAction::Ask)
        );
        assert_eq!(
            requested_action(&[XwaylandDndAction::Ask, XwaylandDndAction::Move]),
            Some(XwaylandDndAction::Move)
        );
        assert_eq!(
            requested_action(&[XwaylandDndAction::Ask, XwaylandDndAction::Copy]),
            Some(XwaylandDndAction::Copy)
        );
        assert_eq!(
            requested_action(&[XwaylandDndAction::Link, XwaylandDndAction::Private]),
            None
        );
    }

    #[test]
    fn enter_is_followed_by_initial_position_and_inline_types_are_bounded() {
        let messages = encode_enter_and_initial_position(EnterPositionFields {
            actual_target: 20,
            source_proxy: 21,
            version: 5,
            mime_atoms: &[31, 32, 33, 34, 35],
            coordinates: 0x0001_fffd,
            timestamp: 40,
            action_atom: 11,
            enter_atom: 51,
            position_atom: 52,
        });
        assert_eq!(messages[0].type_, 51);
        assert_eq!(messages[1].type_, 52);
        assert_eq!(messages[0].window, 20);
        assert_eq!(messages[1].window, 20);
        let enter = messages[0].data.as_data32();
        assert_eq!(enter[0], 21);
        assert_eq!(enter[1], (5 << XDND_VERSION_SHIFT) | XDND_MORE_TYPES);
        assert_eq!(&enter[2..], &[31, 32, 33]);
        assert_eq!(messages[1].data.as_data32(), [21, 0, 0x0001_fffd, 40, 11]);

        for (types, more) in [(&[1][..], 0), (&[1, 2, 3][..], 0), (&[1, 2, 3, 4][..], 1)] {
            let messages = encode_enter_and_initial_position(EnterPositionFields {
                actual_target: 20,
                source_proxy: 21,
                version: 5,
                mime_atoms: types,
                coordinates: 0,
                timestamp: 40,
                action_atom: 11,
                enter_atom: 51,
                position_atom: 52,
            });
            assert_eq!(messages[0].data.as_data32()[1] & XDND_MORE_TYPES, more);
        }
    }

    #[test]
    fn incoming_position_decoder_matches_the_c2_encoder_wire_layout() {
        let atoms = action_atoms();
        let coordinates = pack_root_coordinates(-123.0, 456.0).unwrap();
        let messages = encode_enter_and_initial_position(EnterPositionFields {
            actual_target: 0x31,
            source_proxy: 0x42,
            version: 5,
            mime_atoms: &[0x53],
            coordinates,
            timestamp: 0x8091_a2b3,
            action_atom: atoms.get(XwmAtomName::XdndActionMove),
            enter_atom: 0x64,
            position_atom: 0x75,
        });

        let decoded = decode_xdnd_position(messages[1].data.as_data32());
        assert_eq!(decoded.source, 0x42);
        assert_eq!(decoded.state, 0);
        assert_eq!((decoded.root_x, decoded.root_y), (-123, 456));
        assert_eq!(decoded.timestamp, 0x8091_a2b3);
        assert_eq!(decoded.action_atom, atoms.get(XwmAtomName::XdndActionMove));
    }

    #[test]
    fn wayland_status_actions_are_typed_and_unsupported_values_reject() {
        let atoms = action_atoms();
        let allowed = [
            XwaylandDndAction::Copy,
            XwaylandDndAction::Move,
            XwaylandDndAction::Ask,
        ];
        for (wire_atom, expected) in [
            (11, XwaylandDndAction::Copy),
            (12, XwaylandDndAction::Move),
            (14, XwaylandDndAction::Ask),
        ] {
            assert_eq!(
                decode_status_action(true, wire_atom, Some(expected), &allowed, &atoms),
                (true, Some(expected))
            );
        }
        assert_eq!(
            decode_status_action(false, 11, Some(XwaylandDndAction::Copy), &allowed, &atoms),
            (false, None)
        );
        assert_eq!(
            decode_status_action(true, 999, Some(XwaylandDndAction::Copy), &allowed, &atoms),
            (false, None)
        );
        assert_eq!(
            decode_status_action(true, 13, Some(XwaylandDndAction::Copy), &allowed, &atoms),
            (false, None)
        );
        assert_eq!(
            decode_status_action(true, 15, Some(XwaylandDndAction::Copy), &allowed, &atoms),
            (false, None)
        );
        assert_eq!(
            wayland_action_atom(&atoms, XwaylandDndAction::Copy),
            Some(11)
        );
        assert_eq!(
            wayland_action_atom(&atoms, XwaylandDndAction::Move),
            Some(12)
        );
        assert_eq!(
            wayland_action_atom(&atoms, XwaylandDndAction::Ask),
            Some(14)
        );
        assert_eq!(wayland_action_atom(&atoms, XwaylandDndAction::Link), None);
        assert_eq!(
            wayland_action_atom(&atoms, XwaylandDndAction::Private),
            None
        );
    }

    #[test]
    fn invalid_proxy_self_reference_falls_back_to_actual_target() {
        assert_eq!(validated_proxy(20, Some(21), Some(21)), 21);
        assert_eq!(validated_proxy(20, Some(21), Some(22)), 20);
        assert_eq!(validated_proxy(20, Some(20), Some(21)), 20);
        assert_eq!(validated_proxy(20, None, None), 20);
    }

    #[test]
    fn selection_targets_include_only_implemented_specials_and_offered_mimes() {
        let targets = selection_targets(1, 2, 3, &[30, 31]);
        assert_eq!(targets, [1, 2, 3, 30, 31]);
        assert!(!targets.contains(&99)); // DELETE is deliberately unsupported.
    }

    #[test]
    fn xdnd_drop_uses_actual_target_source_proxy_and_x_server_timestamp() {
        let event = encode_xdnd_drop(XdndDropFields {
            actual_target: 20,
            source_proxy: 21,
            timestamp: 40,
            drop_atom: 51,
        });
        assert_eq!(event.window, 20);
        assert_eq!(event.type_, 51);
        assert_eq!(event.data.as_data32(), [21, 0, 40, 0, 0]);
    }

    #[test]
    fn status_action_must_match_requested_action_or_protocol_copy_fallback() {
        let atoms = action_atoms();
        let source_actions = [XwaylandDndAction::Copy, XwaylandDndAction::Move];
        assert_eq!(
            decode_status_action(
                true,
                11,
                Some(XwaylandDndAction::Copy),
                &source_actions,
                &atoms
            ),
            (true, Some(XwaylandDndAction::Copy))
        );
        assert_eq!(
            decode_status_action(
                true,
                12,
                Some(XwaylandDndAction::Copy),
                &source_actions,
                &atoms
            ),
            (false, None)
        );
        assert_eq!(
            decode_status_action(
                true,
                11,
                Some(XwaylandDndAction::Move),
                &source_actions,
                &atoms
            ),
            (true, Some(XwaylandDndAction::Copy))
        );
        assert_eq!(
            decode_status_action(
                true,
                12,
                Some(XwaylandDndAction::Ask),
                &source_actions,
                &atoms
            ),
            (false, None)
        );
    }

    #[test]
    fn xdnd_finished_decodes_v5_result_and_ignores_v5_fields_before_version_five() {
        let atoms = action_atoms();
        let source_actions = [XwaylandDndAction::Copy, XwaylandDndAction::Move];
        assert_eq!(
            decode_xdnd_finished(
                [20, XDND_FINISHED_ACCEPTED, 12, 0, 0],
                5,
                XwaylandDndAction::Move,
                &source_actions,
                &atoms,
            ),
            XdndFinishedResult {
                accepted: true,
                action: Some(XwaylandDndAction::Move),
            }
        );
        assert_eq!(
            decode_xdnd_finished(
                [20, 0, 12, 0, 0],
                5,
                XwaylandDndAction::Move,
                &source_actions,
                &atoms,
            ),
            XdndFinishedResult {
                accepted: false,
                action: None,
            }
        );
        assert_eq!(
            decode_xdnd_finished(
                [20, 0, 15, 0, 0],
                4,
                XwaylandDndAction::Copy,
                &source_actions,
                &atoms,
            ),
            XdndFinishedResult {
                accepted: true,
                action: Some(XwaylandDndAction::Copy),
            }
        );
        assert_eq!(
            decode_xdnd_finished(
                [20, XDND_FINISHED_ACCEPTED, 15, 0, 0],
                5,
                XwaylandDndAction::Ask,
                &source_actions,
                &atoms,
            ),
            XdndFinishedResult {
                accepted: false,
                action: None,
            }
        );
    }
}
