use super::*;
use crate::xwayland::{CanonicalDndSessionId, XwaylandDndOffer};

#[derive(Debug, Clone)]
pub(super) struct IdleInhibitorBinding {
    pub(super) inhibitor: zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1,
    pub(super) client_id: ClientId,
    pub(super) target_surface: wl_surface::WlSurface,
}

#[derive(Debug, Clone)]
pub(super) struct DataSourceData {
    pub(super) client_id: ClientId,
}

#[derive(Debug, Clone)]
pub(super) struct DataDeviceData {
    pub(super) client_id: ClientId,
    pub(super) seat_id: ObjectId,
}

#[derive(Debug, Clone)]
pub(super) struct ClipboardDataDevice {
    pub(super) device: wl_data_device::WlDataDevice,
    pub(super) client_id: ClientId,
    pub(super) seat_id: ObjectId,
}

#[derive(Debug, Clone)]
pub(super) struct ClipboardDataOffer {
    pub(super) offer: wl_data_offer::WlDataOffer,
    pub(super) target_client_id: ClientId,
    pub(super) target_id: u32,
    pub(super) source_generation: u64,
    pub(super) broker_offer_id: Option<u64>,
    pub(super) source_key: Option<SelectionSourceKey>,
    pub(super) mime_types: Vec<String>,
    pub(super) kind: DataOfferKind,
    pub(super) accepted_mime: Option<String>,
    pub(super) selected_action: Option<u32>,
    pub(super) drag_phase: Option<DragOfferPhase>,
    pub(super) source_actions: u32,
    pub(super) destination_actions: Option<u32>,
    pub(super) preferred_action: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DataSourceUse {
    Unused,
    Selection,
    DragSource,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DataOfferKind {
    Selection,
    DragAndDrop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DragSessionPhase {
    Dragging,
    DropPendingXwaylandTarget,
    DroppedAwaitingFinish,
    DroppedAwaitingAskResolution,
    Finished,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DragOfferPhase {
    Entered,
    Dropped,
    Finished,
    Destroyed,
}

#[derive(Debug, Clone)]
pub(super) enum ActiveDragOrigin {
    WaylandSource {
        source: wl_data_source::WlDataSource,
        origin_surface: wl_surface::WlSurface,
        #[allow(dead_code)]
        initiating_serial: u32,
    },
    WaylandSourceless {
        initiating_client: ClientId,
        origin_surface: wl_surface::WlSurface,
        #[allow(dead_code)]
        initiating_serial: u32,
    },
    Xwayland {
        offer: XwaylandDndOffer,
    },
}

impl ActiveDragOrigin {
    pub(super) fn wayland_source(&self) -> Option<&wl_data_source::WlDataSource> {
        match self {
            Self::WaylandSource { source, .. } => Some(source),
            Self::WaylandSourceless { .. } | Self::Xwayland { .. } => None,
        }
    }

    pub(super) fn wayland_origin_surface(&self) -> Option<&wl_surface::WlSurface> {
        match self {
            Self::WaylandSource { origin_surface, .. }
            | Self::WaylandSourceless { origin_surface, .. } => Some(origin_surface),
            Self::Xwayland { .. } => None,
        }
    }

    pub(super) fn wayland_initiating_client(&self) -> Option<ClientId> {
        match self {
            Self::WaylandSource { source, .. } => source.client().map(|client| client.id()),
            Self::WaylandSourceless {
                initiating_client, ..
            } => Some(initiating_client.clone()),
            Self::Xwayland { .. } => None,
        }
    }

    pub(super) const fn is_wayland_sourceless(&self) -> bool {
        matches!(self, Self::WaylandSourceless { .. })
    }

    pub(super) const fn xwayland_offer(&self) -> Option<&XwaylandDndOffer> {
        match self {
            Self::Xwayland { offer } => Some(offer),
            Self::WaylandSource { .. } | Self::WaylandSourceless { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DragLifecycleDriver {
    WaylandImplicitPointerGrab,
    Xwayland,
}

#[derive(Debug, Clone)]
pub(super) enum ActiveDragTarget {
    Wayland {
        surface: wl_surface::WlSurface,
        client_id: ClientId,
        offer: Option<wl_data_offer::WlDataOffer>,
    },
    Xwayland {
        window: crate::xwayland::X11WindowHandle,
    },
}

impl ActiveDragTarget {
    pub(super) fn wayland_offer(&self) -> Option<&wl_data_offer::WlDataOffer> {
        match self {
            Self::Wayland {
                offer: Some(offer), ..
            } => Some(offer),
            Self::Wayland { offer: None, .. } | Self::Xwayland { .. } => None,
        }
    }

    pub(super) fn wayland_client(&self) -> Option<&ClientId> {
        match self {
            Self::Wayland { client_id, .. } => Some(client_id),
            Self::Xwayland { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ActiveDrag {
    pub(super) id: CanonicalDndSessionId,
    pub(super) origin: ActiveDragOrigin,
    pub(super) lifecycle_driver: DragLifecycleDriver,
    pub(super) icon_surface: Option<wl_surface::WlSurface>,
    pub(super) target: Option<ActiveDragTarget>,
    pub(super) accepted_mime: Option<String>,
    pub(super) target_action: Option<crate::xwayland::XwaylandDndAction>,
    pub(super) selected_action: u32,
    pub(super) destination_actions: Option<u32>,
    pub(super) last_offer_action: Option<u32>,
    pub(super) last_source_action: Option<u32>,
    pub(super) phase: DragSessionPhase,
}

#[derive(Debug, Clone)]
pub(super) struct PrimarySourceBinding {
    pub(super) source: zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
    pub(super) selection_key: SelectionSourceKey,
    pub(super) client_id: ClientId,
    pub(super) mime_types: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct PrimaryDeviceBinding {
    pub(super) device: zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
    pub(super) client_id: ClientId,
    pub(super) seat_id: ObjectId,
}

#[derive(Debug, Clone)]
pub(super) struct PrimaryOfferBinding {
    pub(super) offer: zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
    pub(super) target_client_id: ClientId,
    pub(super) target_id: u32,
    pub(super) broker_offer_id: u64,
    pub(super) source_generation: u64,
    pub(super) source_key: SelectionSourceKey,
    pub(super) mime_types: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct DataControlSourceBinding {
    pub(super) source: ext_data_control_source_v1::ExtDataControlSourceV1,
    pub(super) selection_key: SelectionSourceKey,
    pub(super) client_id: ClientId,
    pub(super) mime_types: Vec<String>,
    pub(super) used: bool,
}

#[derive(Debug, Clone)]
pub(super) struct DataControlDeviceBinding {
    pub(super) device: ext_data_control_device_v1::ExtDataControlDeviceV1,
    pub(super) client_id: ClientId,
    pub(super) seat_id: ObjectId,
}

#[derive(Debug, Clone)]
pub(super) struct DataControlOfferBinding {
    pub(super) offer: ext_data_control_offer_v1::ExtDataControlOfferV1,
    pub(super) target_client_id: ClientId,
    pub(super) target_id: u32,
    pub(super) broker_offer_id: u64,
    pub(super) kind: SelectionKind,
    pub(super) source_generation: u64,
    pub(super) source_key: SelectionSourceKey,
    pub(super) mime_types: Vec<String>,
}
