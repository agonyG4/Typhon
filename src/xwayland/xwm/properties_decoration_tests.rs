use std::num::NonZeroU64;

use x11rb::protocol::xproto;

use super::{
    ParsedProperty, PropertyKind, X11PropertySnapshot, X11WindowType, X11WindowTypes, apply_parsed,
    commit_property, fallback_for, parse_gtk_frame_extents, parse_motif_hints,
};
use crate::compositor::DesktopWindowKind;
use crate::xwayland::XwaylandGeneration;
use crate::xwayland::xwm::window::X11WindowRegistry;
use crate::xwayland::xwm::{
    X11DecorationHints, X11FrameExtents, X11MetadataDelta, X11MotifDecorationHint,
    X11WindowSnapshot,
};

fn test_handle() -> super::X11WindowHandle {
    super::X11WindowHandle::new(
        XwaylandGeneration::new(NonZeroU64::new(1).expect("nonzero")),
        42,
    )
}

#[test]
fn window_type_refresh_does_not_replace_motif_decoration_preference() {
    let handle = test_handle();
    let mut properties = X11PropertySnapshot::default();

    apply_parsed(
        &mut properties,
        handle,
        PropertyKind::MotifWmHints,
        ParsedProperty::MotifDecorationHint(X11MotifDecorationHint::Undecorated),
    );
    apply_parsed(
        &mut properties,
        handle,
        PropertyKind::NetWmWindowType,
        ParsedProperty::WindowTypes(X11WindowTypes::new(vec![X11WindowType::Normal])),
    );

    assert_eq!(
        properties.decoration_hints.motif,
        X11MotifDecorationHint::Undecorated
    );
}

#[test]
fn motif_and_window_type_refresh_order_is_commutative() {
    let handle = test_handle();
    let mut motif_then_type = X11PropertySnapshot::default();
    apply_parsed(
        &mut motif_then_type,
        handle,
        PropertyKind::MotifWmHints,
        ParsedProperty::MotifDecorationHint(X11MotifDecorationHint::Undecorated),
    );
    apply_parsed(
        &mut motif_then_type,
        handle,
        PropertyKind::NetWmWindowType,
        ParsedProperty::WindowTypes(X11WindowTypes::new(vec![X11WindowType::Dialog])),
    );

    let mut type_then_motif = X11PropertySnapshot::default();
    apply_parsed(
        &mut type_then_motif,
        handle,
        PropertyKind::NetWmWindowType,
        ParsedProperty::WindowTypes(X11WindowTypes::new(vec![X11WindowType::Dialog])),
    );
    apply_parsed(
        &mut type_then_motif,
        handle,
        PropertyKind::MotifWmHints,
        ParsedProperty::MotifDecorationHint(X11MotifDecorationHint::Undecorated),
    );

    assert_eq!(motif_then_type, type_then_motif);
}

fn property_reply(
    values: &[u32],
    type_: u32,
    format: u8,
    bytes_after: u32,
) -> xproto::GetPropertyReply {
    let value = if format == 32 {
        values
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect()
    } else {
        values.iter().map(|value| *value as u8).collect()
    };
    xproto::GetPropertyReply {
        format,
        sequence: 0,
        length: values.len() as u32,
        type_,
        bytes_after,
        value_len: values.len() as u32,
        value,
    }
}

#[test]
fn motif_hints_have_explicit_decoration_semantics() {
    assert_eq!(
        parse_motif_hints(&property_reply(&[2, 0, 0], 1, 32, 0)),
        Some(ParsedProperty::MotifDecorationHint(
            X11MotifDecorationHint::Undecorated
        ))
    );
    assert_eq!(
        parse_motif_hints(&property_reply(&[2, 0, 1], 1, 32, 0)),
        Some(ParsedProperty::MotifDecorationHint(
            X11MotifDecorationHint::Decorated
        ))
    );
    assert_eq!(
        parse_motif_hints(&property_reply(&[0], 1, 32, 0)),
        Some(ParsedProperty::MotifDecorationHint(
            X11MotifDecorationHint::Unspecified
        ))
    );
    assert_eq!(
        fallback_for(PropertyKind::MotifWmHints),
        ParsedProperty::MotifDecorationHint(X11MotifDecorationHint::Unspecified)
    );
    assert_eq!(parse_motif_hints(&property_reply(&[2, 0], 1, 32, 0)), None);
}

#[test]
fn gtk_frame_extents_require_exact_cardinal_quadruple() {
    assert_eq!(
        parse_gtk_frame_extents(&property_reply(&[1, 2, 3, 4], 1, 32, 0)),
        Some(ParsedProperty::GtkFrameExtents(Some(X11FrameExtents {
            left: 1,
            right: 2,
            top: 3,
            bottom: 4,
        })))
    );
    assert_eq!(
        parse_gtk_frame_extents(&property_reply(&[1, 2, 3], 1, 32, 0)),
        None
    );
    assert_eq!(
        parse_gtk_frame_extents(&property_reply(&[1, 2, 3, 4, 5], 1, 32, 0)),
        None
    );
    assert_eq!(
        parse_gtk_frame_extents(&property_reply(&[1, 2, 3, 4], 1, 32, 1)),
        None
    );
    assert_eq!(
        fallback_for(PropertyKind::GtkFrameExtents),
        ParsedProperty::GtkFrameExtents(None)
    );
    assert_eq!(PropertyKind::GtkFrameExtents.max_items(), 4);
}

#[test]
fn gtk_frame_extents_complete_parser_rejects_invalid_replies() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(3).expect("nonzero"));
    let (xwm, _peer) = super::super::events::tests::test_fixture(generation);
    let valid_type = u32::from(xproto::AtomEnum::CARDINAL);
    let invalid_type = u32::from(xproto::AtomEnum::STRING);

    for reply in [
        property_reply(&[1, 2, 3, 4], invalid_type, 32, 0),
        property_reply(&[1, 2, 3, 4], valid_type, 8, 0),
        property_reply(&[1, 2, 3], valid_type, 32, 0),
        property_reply(&[1, 2, 3, 4], valid_type, 32, 1),
    ] {
        assert_eq!(
            super::parse(PropertyKind::GtkFrameExtents, &reply, &xwm),
            None
        );
    }
}

#[test]
fn admitted_decoration_hint_refresh_emits_only_changed_metadata() {
    let handle = test_handle();
    let mut registry = X11WindowRegistry::default();
    registry.insert_snapshot(X11WindowSnapshot {
        handle,
        surface_id: 9,
        kind: DesktopWindowKind::Managed,
        window_types: X11WindowTypes::default(),
        decoration_hints: Default::default(),
        override_redirect: false,
        geometry: Default::default(),
        metadata: Default::default(),
        constraints: Default::default(),
        state: Default::default(),
        transient_for: None,
        supports_delete: false,
        supports_take_focus: false,
        accepts_input: Some(true),
        window_role: None,
        startup_id: None,
        user_time: None,
        urgency: false,
        supports_sync_request: false,
        sync_counter: None,
    });
    let record = registry.get_mut(handle).expect("snapshot record");
    apply_parsed(
        &mut record.staging_properties,
        handle,
        PropertyKind::MotifWmHints,
        ParsedProperty::MotifDecorationHint(X11MotifDecorationHint::Undecorated),
    );
    let delta = commit_property(record, PropertyKind::MotifWmHints);
    assert_eq!(
        delta,
        Some(X11MetadataDelta::DecorationHints(X11DecorationHints {
            motif: X11MotifDecorationHint::Undecorated,
            gtk_frame_extents: None,
        }))
    );

    let record = registry.get_mut(handle).expect("snapshot record");
    apply_parsed(
        &mut record.staging_properties,
        handle,
        PropertyKind::MotifWmHints,
        ParsedProperty::MotifDecorationHint(X11MotifDecorationHint::Undecorated),
    );
    assert_eq!(commit_property(record, PropertyKind::MotifWmHints), None);
}

#[test]
fn gtk_frame_extents_refresh_emits_each_dynamic_decoration_delta() {
    let handle = test_handle();
    let mut registry = X11WindowRegistry::default();
    registry.insert_snapshot(X11WindowSnapshot {
        handle,
        surface_id: 9,
        kind: DesktopWindowKind::Managed,
        window_types: X11WindowTypes::default(),
        decoration_hints: Default::default(),
        override_redirect: false,
        geometry: Default::default(),
        metadata: Default::default(),
        constraints: Default::default(),
        state: Default::default(),
        transient_for: None,
        supports_delete: false,
        supports_take_focus: false,
        accepts_input: Some(true),
        window_role: None,
        startup_id: None,
        user_time: None,
        urgency: false,
        supports_sync_request: false,
        sync_counter: None,
    });
    let values = [
        Some(X11FrameExtents {
            left: 1,
            right: 2,
            top: 3,
            bottom: 4,
        }),
        Some(X11FrameExtents {
            left: 4,
            right: 3,
            top: 2,
            bottom: 1,
        }),
        None,
    ];
    let expected = values.map(|gtk_frame_extents| {
        X11MetadataDelta::DecorationHints(X11DecorationHints {
            motif: X11MotifDecorationHint::Unspecified,
            gtk_frame_extents,
        })
    });

    for (gtk_frame_extents, expected_delta) in values.into_iter().zip(expected) {
        let record = registry.get_mut(handle).expect("snapshot record");
        apply_parsed(
            &mut record.staging_properties,
            handle,
            PropertyKind::GtkFrameExtents,
            ParsedProperty::GtkFrameExtents(gtk_frame_extents),
        );
        assert_eq!(
            commit_property(record, PropertyKind::GtkFrameExtents),
            Some(expected_delta)
        );
    }
}
