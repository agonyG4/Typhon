use std::{os::fd::AsRawFd, os::unix::net::UnixStream};

use x11rb::{
    connection::Connection,
    protocol::composite::ConnectionExt as CompositeConnectionExt,
    protocol::xproto::ConnectionExt as XprotoConnectionExt,
    protocol::{composite, xproto},
    rust_connection::{DefaultStream, RustConnection},
    wrapper::ConnectionExt,
};

use super::{
    Xwm, XwmStartupError, atoms::XwmAtoms, capabilities::XwmCapabilities, window::X11WindowRegistry,
};
use crate::xwayland::XwaylandGeneration;

pub(crate) fn connect(
    generation: XwaylandGeneration,
    stream: UnixStream,
) -> Result<Xwm, XwmStartupError> {
    let (stream, _) = DefaultStream::from_unix_stream(stream)
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    let raw_fd = stream.as_raw_fd();
    let connection =
        RustConnection::connect_to_stream(stream, 0).map_err(XwmStartupError::Connection)?;
    let screen_number = 0;
    let root = connection
        .setup()
        .roots
        .get(screen_number)
        .ok_or(XwmStartupError::InvalidScreen)?
        .root;
    let capabilities = XwmCapabilities::discover(&connection)?;
    let atoms = XwmAtoms::intern(&connection)?;
    let supporting_wm_check = setup_root(&connection, root, &atoms, &capabilities)?;
    connection
        .flush()
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;

    Ok(Xwm {
        generation,
        connection,
        screen_number,
        root,
        atoms,
        capabilities,
        windows: X11WindowRegistry::default(),
        outgoing_events: Default::default(),
        association: super::association::SurfaceAssociationJoin::default(),
        buffer_ready_surfaces: Default::default(),
        supporting_wm_check,
        raw_fd,
    })
}

fn setup_root(
    connection: &RustConnection<DefaultStream>,
    root: u32,
    atoms: &XwmAtoms,
    capabilities: &XwmCapabilities,
) -> Result<u32, XwmStartupError> {
    if !capabilities.composite
        || !capabilities.xfixes
        || !capabilities.shape
        || !capabilities.randr
        || !capabilities.sync
    {
        return Err(XwmStartupError::Protocol(
            "XWM capability record is incomplete".to_owned(),
        ));
    }

    let event_mask = xproto::EventMask::SUBSTRUCTURE_REDIRECT
        | xproto::EventMask::SUBSTRUCTURE_NOTIFY
        | xproto::EventMask::PROPERTY_CHANGE
        | xproto::EventMask::FOCUS_CHANGE;
    connection
        .change_window_attributes(
            root,
            &xproto::ChangeWindowAttributesAux::new().event_mask(event_mask),
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?
        .check()
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    connection
        .composite_redirect_subwindows(root, composite::Redirect::MANUAL)
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?
        .check()
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;

    let screen = connection
        .setup()
        .roots
        .first()
        .ok_or(XwmStartupError::InvalidScreen)?;
    let supporting_wm_check = connection
        .generate_id()
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    connection
        .create_window(
            screen.root_depth,
            supporting_wm_check,
            root,
            0,
            0,
            1,
            1,
            0,
            xproto::WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &xproto::CreateWindowAux::new(),
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?
        .check()
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;

    let supporting_atom = atoms.get(super::atoms::XwmAtomName::NetSupportingWmCheck);
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            root,
            supporting_atom,
            xproto::AtomEnum::WINDOW,
            &[supporting_wm_check],
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            supporting_wm_check,
            supporting_atom,
            xproto::AtomEnum::WINDOW,
            &[supporting_wm_check],
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    connection
        .change_property8(
            xproto::PropMode::REPLACE,
            supporting_wm_check,
            atoms.get(super::atoms::XwmAtomName::NetWmName),
            atoms.get(super::atoms::XwmAtomName::Utf8String),
            b"Typhon",
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;

    let supported = XwmAtoms::advertised_names()
        .iter()
        .map(|name| atoms.get(*name))
        .collect::<Vec<_>>();
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            root,
            atoms.get(super::atoms::XwmAtomName::NetSupported),
            xproto::AtomEnum::ATOM,
            &supported,
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            root,
            atoms.get(super::atoms::XwmAtomName::NetActiveWindow),
            xproto::AtomEnum::WINDOW,
            &[],
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            root,
            atoms.get(super::atoms::XwmAtomName::NetClientList),
            xproto::AtomEnum::WINDOW,
            &[],
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            root,
            atoms.get(super::atoms::XwmAtomName::NetClientListStacking),
            xproto::AtomEnum::WINDOW,
            &[],
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    Ok(supporting_wm_check)
}
