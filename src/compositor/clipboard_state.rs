use super::*;

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
    pub(super) source_generation: u64,
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
pub(super) struct ActiveClipboard {
    pub(super) generation: u64,
    pub(super) source: ClipboardSourceBackend,
    pub(super) mime_types: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ActiveDrag {
    pub(super) source: Option<wl_data_source::WlDataSource>,
    #[allow(dead_code)]
    pub(super) origin_surface: wl_surface::WlSurface,
    pub(super) icon_surface: Option<wl_surface::WlSurface>,
    #[allow(dead_code)]
    pub(super) initiating_serial: u32,
    pub(super) target_surface: Option<wl_surface::WlSurface>,
    pub(super) target_client: Option<ClientId>,
    pub(super) offer: Option<wl_data_offer::WlDataOffer>,
    pub(super) accepted_mime: Option<String>,
    pub(super) selected_action: u32,
    pub(super) destination_actions: Option<u32>,
    pub(super) last_offer_action: Option<u32>,
    pub(super) last_source_action: Option<u32>,
    pub(super) phase: DragSessionPhase,
}

#[derive(Debug, Clone)]
pub(super) enum ClipboardSourceBackend {
    InternalWayland {
        source: wl_data_source::WlDataSource,
        client_id: ClientId,
    },
    HostBridge {
        offer_id: HostClipboardOfferId,
    },
}
