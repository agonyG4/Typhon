use std::num::NonZeroU64;

use x11rb::protocol::{Event, xproto};

use super::{normalize, tests::test_fixture};

#[test]
fn root_xdnd_proxy_property_notify_is_consumed_before_window_normalization() {
    let generation = crate::xwayland::XwaylandGeneration::new(
        NonZeroU64::new(120).expect("nonzero XWayland generation"),
    );
    let (mut xwm, _peer) = test_fixture(generation);
    super::super::data_bridge::dnd_incoming::initialize_target_proxy(&mut xwm)
        .expect("initialize private DND target proxy");
    let proxy = super::super::data_bridge::dnd_incoming::target_proxy(&xwm)
        .expect("private DND target proxy");
    xwm.data_bridge.dnd_incoming.root_proxy_authority =
        super::super::data_bridge::dnd_incoming::RootProxyAuthority::Owned { generation, proxy };
    let event = xproto::PropertyNotifyEvent {
        response_type: xproto::PROPERTY_NOTIFY_EVENT,
        sequence: 0,
        window: xwm.root,
        atom: xwm.atoms.get(super::super::atoms::XwmAtomName::XdndProxy),
        time: 1,
        state: xproto::Property::NEW_VALUE,
    };

    normalize(&mut xwm, Event::PropertyNotify(event))
        .expect("route root proxy notification through DND authority check");

    assert!(matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        super::super::data_bridge::dnd_incoming::RootProxyAuthority::Verifying {
            generation: owner,
            proxy: verifying_proxy,
            ..
        } if owner == generation && verifying_proxy == proxy
    ));
    assert!(
        !xwm.windows
            .contains(crate::xwayland::X11WindowHandle::new(generation, xwm.root))
    );
}
