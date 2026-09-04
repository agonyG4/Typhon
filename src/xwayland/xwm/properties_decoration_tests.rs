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
        Vec::new()
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
