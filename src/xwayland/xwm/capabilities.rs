use super::atoms::XwmAtomName;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct XwmCapabilities {
    pub(crate) composite: bool,
    pub(crate) xfixes: bool,
    pub(crate) shape: bool,
    pub(crate) randr: bool,
    pub(crate) sync: bool,
}

impl XwmCapabilities {
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
