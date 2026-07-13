use super::*;

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
}

#[derive(Debug, Clone)]
pub(super) struct ActiveClipboard {
    pub(super) generation: u64,
    pub(super) source: ClipboardSourceBackend,
    pub(super) mime_types: Vec<String>,
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
