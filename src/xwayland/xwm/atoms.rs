use std::collections::HashMap;

use x11rb::{
    connection::RequestConnection,
    protocol::xproto::{Atom, ConnectionExt as XprotoConnectionExt},
};

use super::XwmStartupError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum XwmAtomName {
    WlSurfaceSerial,
    Utf8String,
    WmProtocols,
    WmDeleteWindow,
    WmTakeFocus,
    WmHints,
    WmClass,
    WmName,
    WmTransientFor,
    WmNormalHints,
    NetWmName,
    NetWmPid,
    NetWmWindowType,
    NetWmWindowTypeNormal,
    NetWmWindowTypeDialog,
    NetWmWindowTypeUtility,
    NetWmWindowTypeMenu,
    NetWmWindowTypePopupMenu,
    NetWmWindowTypeDropdownMenu,
    NetWmWindowTypeTooltip,
    NetWmWindowTypeNotification,
    NetWmState,
    NetWmStateFullscreen,
    NetWmStateMaximizedVert,
    NetWmStateMaximizedHorz,
    NetWmStateHidden,
    NetActiveWindow,
    NetCloseWindow,
    NetMoveResizeWindow,
    NetWmMoveresize,
    NetSupported,
    NetSupportingWmCheck,
    NetClientList,
    NetClientListStacking,
    NetWmSyncRequest,
    NetWmSyncRequestCounter,
    XwaylandAllowCommits,
    MotifWmHints,
    NetWmNameTyphon,
}

impl XwmAtomName {
    pub(crate) const ALL: &'static [Self] = &[
        Self::WlSurfaceSerial,
        Self::Utf8String,
        Self::WmProtocols,
        Self::WmDeleteWindow,
        Self::WmTakeFocus,
        Self::WmHints,
        Self::WmClass,
        Self::WmName,
        Self::WmTransientFor,
        Self::WmNormalHints,
        Self::NetWmName,
        Self::NetWmPid,
        Self::NetWmWindowType,
        Self::NetWmWindowTypeNormal,
        Self::NetWmWindowTypeDialog,
        Self::NetWmWindowTypeUtility,
        Self::NetWmWindowTypeMenu,
        Self::NetWmWindowTypePopupMenu,
        Self::NetWmWindowTypeDropdownMenu,
        Self::NetWmWindowTypeTooltip,
        Self::NetWmWindowTypeNotification,
        Self::NetWmState,
        Self::NetWmStateFullscreen,
        Self::NetWmStateMaximizedVert,
        Self::NetWmStateMaximizedHorz,
        Self::NetWmStateHidden,
        Self::NetActiveWindow,
        Self::NetCloseWindow,
        Self::NetMoveResizeWindow,
        Self::NetWmMoveresize,
        Self::NetSupported,
        Self::NetSupportingWmCheck,
        Self::NetClientList,
        Self::NetClientListStacking,
        Self::NetWmSyncRequest,
        Self::NetWmSyncRequestCounter,
        Self::XwaylandAllowCommits,
        Self::MotifWmHints,
        Self::NetWmNameTyphon,
    ];

    pub(crate) const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::WlSurfaceSerial => b"WL_SURFACE_SERIAL",
            Self::Utf8String => b"UTF8_STRING",
            Self::WmProtocols => b"WM_PROTOCOLS",
            Self::WmDeleteWindow => b"WM_DELETE_WINDOW",
            Self::WmTakeFocus => b"WM_TAKE_FOCUS",
            Self::WmHints => b"WM_HINTS",
            Self::WmClass => b"WM_CLASS",
            Self::WmName => b"WM_NAME",
            Self::WmTransientFor => b"WM_TRANSIENT_FOR",
            Self::WmNormalHints => b"WM_NORMAL_HINTS",
            Self::NetWmName => b"_NET_WM_NAME",
            Self::NetWmPid => b"_NET_WM_PID",
            Self::NetWmWindowType => b"_NET_WM_WINDOW_TYPE",
            Self::NetWmWindowTypeNormal => b"_NET_WM_WINDOW_TYPE_NORMAL",
            Self::NetWmWindowTypeDialog => b"_NET_WM_WINDOW_TYPE_DIALOG",
            Self::NetWmWindowTypeUtility => b"_NET_WM_WINDOW_TYPE_UTILITY",
            Self::NetWmWindowTypeMenu => b"_NET_WM_WINDOW_TYPE_MENU",
            Self::NetWmWindowTypePopupMenu => b"_NET_WM_WINDOW_TYPE_POPUP_MENU",
            Self::NetWmWindowTypeDropdownMenu => b"_NET_WM_WINDOW_TYPE_DROPDOWN_MENU",
            Self::NetWmWindowTypeTooltip => b"_NET_WM_WINDOW_TYPE_TOOLTIP",
            Self::NetWmWindowTypeNotification => b"_NET_WM_WINDOW_TYPE_NOTIFICATION",
            Self::NetWmState => b"_NET_WM_STATE",
            Self::NetWmStateFullscreen => b"_NET_WM_STATE_FULLSCREEN",
            Self::NetWmStateMaximizedVert => b"_NET_WM_STATE_MAXIMIZED_VERT",
            Self::NetWmStateMaximizedHorz => b"_NET_WM_STATE_MAXIMIZED_HORZ",
            Self::NetWmStateHidden => b"_NET_WM_STATE_HIDDEN",
            Self::NetActiveWindow => b"_NET_ACTIVE_WINDOW",
            Self::NetCloseWindow => b"_NET_CLOSE_WINDOW",
            Self::NetMoveResizeWindow => b"_NET_MOVERESIZE_WINDOW",
            Self::NetWmMoveresize => b"_NET_WM_MOVERESIZE",
            Self::NetSupported => b"_NET_SUPPORTED",
            Self::NetSupportingWmCheck => b"_NET_SUPPORTING_WM_CHECK",
            Self::NetClientList => b"_NET_CLIENT_LIST",
            Self::NetClientListStacking => b"_NET_CLIENT_LIST_STACKING",
            Self::NetWmSyncRequest => b"_NET_WM_SYNC_REQUEST",
            Self::NetWmSyncRequestCounter => b"_NET_WM_SYNC_REQUEST_COUNTER",
            Self::XwaylandAllowCommits => b"_XWAYLAND_ALLOW_COMMITS",
            Self::MotifWmHints => b"_MOTIF_WM_HINTS",
            Self::NetWmNameTyphon => b"Typhon",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct XwmAtoms {
    values: HashMap<XwmAtomName, Atom>,
}

impl XwmAtoms {
    pub(crate) fn from_values(values: HashMap<XwmAtomName, Atom>) -> Self {
        Self { values }
    }

    pub(crate) fn into_values(self) -> HashMap<XwmAtomName, Atom> {
        self.values
    }

    pub(crate) fn intern<C: RequestConnection>(connection: &C) -> Result<Self, XwmStartupError> {
        let mut values = HashMap::with_capacity(XwmAtomName::ALL.len());
        for name in XwmAtomName::ALL {
            let atom = connection
                .intern_atom(false, name.as_bytes())
                .map_err(|error| XwmStartupError::Protocol(error.to_string()))?
                .reply()
                .map_err(|error| XwmStartupError::Protocol(error.to_string()))?
                .atom;
            values.insert(*name, atom);
        }
        Ok(Self { values })
    }

    pub(crate) fn get(&self, name: XwmAtomName) -> Atom {
        self.values
            .get(&name)
            .copied()
            .expect("all required XWM atoms are interned")
    }

    pub(crate) fn advertised_names() -> &'static [XwmAtomName] {
        &[
            XwmAtomName::NetSupported,
            XwmAtomName::NetSupportingWmCheck,
            XwmAtomName::NetWmName,
            XwmAtomName::NetWmPid,
            XwmAtomName::NetWmWindowType,
            XwmAtomName::NetActiveWindow,
            XwmAtomName::NetClientList,
            XwmAtomName::NetClientListStacking,
            XwmAtomName::NetCloseWindow,
            XwmAtomName::NetWmState,
            XwmAtomName::NetWmStateFullscreen,
            XwmAtomName::NetWmStateMaximizedVert,
            XwmAtomName::NetWmStateMaximizedHorz,
            XwmAtomName::NetWmStateHidden,
            XwmAtomName::NetWmSyncRequest,
        ]
    }
}
