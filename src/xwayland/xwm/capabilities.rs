use x11rb::{connection::Connection, protocol};

use super::XwmStartupError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct XwmCapabilities {
    pub(crate) composite: bool,
    pub(crate) xfixes: bool,
    pub(crate) shape: bool,
    pub(crate) randr: bool,
    pub(crate) sync: bool,
}

impl XwmCapabilities {
    pub(crate) fn discover<C: Connection>(connection: &C) -> Result<Self, XwmStartupError> {
        let composite = has_extension(connection, protocol::composite::X11_EXTENSION_NAME)?;
        let xfixes = has_extension(connection, protocol::xfixes::X11_EXTENSION_NAME)?;
        let shape = has_extension(connection, protocol::shape::X11_EXTENSION_NAME)?;
        let randr = has_extension(connection, protocol::randr::X11_EXTENSION_NAME)?;
        let sync = has_extension(connection, protocol::sync::X11_EXTENSION_NAME)?;

        for (present, name) in [
            (composite, protocol::composite::X11_EXTENSION_NAME),
            (xfixes, protocol::xfixes::X11_EXTENSION_NAME),
            (shape, protocol::shape::X11_EXTENSION_NAME),
            (randr, protocol::randr::X11_EXTENSION_NAME),
            (sync, protocol::sync::X11_EXTENSION_NAME),
        ] {
            if !present {
                return Err(XwmStartupError::MissingRequiredExtension(name));
            }
        }

        Ok(Self {
            composite,
            xfixes,
            shape,
            randr,
            sync,
        })
    }
}

fn has_extension<C: Connection>(
    connection: &C,
    name: &'static str,
) -> Result<bool, XwmStartupError> {
    connection
        .extension_information(name)
        .map(|information| information.is_some())
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))
}
