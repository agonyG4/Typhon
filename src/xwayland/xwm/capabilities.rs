use x11rb::{connection::Connection, protocol};

use super::{XwmStartupError, atoms::XwmAtomName};

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

        if !composite {
            return Err(XwmStartupError::MissingRequiredExtension(
                protocol::composite::X11_EXTENSION_NAME,
            ));
        }

        Ok(Self {
            composite,
            xfixes,
            shape,
            randr,
            sync,
        })
    }

    pub(crate) const fn required_contract_available(self) -> bool {
        self.composite
    }

    pub(crate) const fn supports_atom(self, atom: XwmAtomName) -> bool {
        match atom {
            XwmAtomName::NetWmSyncRequest | XwmAtomName::NetWmSyncRequestCounter => self.sync,
            _ => true,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities(xfixes: bool, shape: bool, randr: bool, sync: bool) -> XwmCapabilities {
        XwmCapabilities {
            composite: true,
            xfixes,
            shape,
            randr,
            sync,
        }
    }

    #[test]
    fn optional_xfixes_missing_does_not_invalidate_composite_startup() {
        assert!(capabilities(false, true, true, true).required_contract_available());
    }

    #[test]
    fn optional_shape_missing_does_not_invalidate_composite_startup() {
        assert!(capabilities(true, false, true, true).required_contract_available());
    }

    #[test]
    fn optional_randr_missing_does_not_invalidate_composite_startup() {
        assert!(capabilities(true, true, false, true).required_contract_available());
    }

    #[test]
    fn optional_sync_missing_does_not_invalidate_composite_startup() {
        assert!(capabilities(true, true, true, false).required_contract_available());
        assert!(
            !capabilities(true, true, true, false).supports_atom(XwmAtomName::NetWmSyncRequest)
        );
    }

    #[test]
    fn all_optional_extensions_missing_keeps_composite_startup_contract() {
        assert!(capabilities(false, false, false, false).required_contract_available());
    }
}
