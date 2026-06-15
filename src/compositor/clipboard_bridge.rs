use std::os::fd::OwnedFd;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostClipboardOfferId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardBridgeEvent {
    HostSelectionChanged {
        offer_id: HostClipboardOfferId,
        mime_types: Vec<String>,
    },
    HostSelectionCleared,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardBridgeError {
    Unavailable,
}

pub trait ClipboardBridge: std::fmt::Debug + Send {
    fn poll_events(&mut self) -> Vec<ClipboardBridgeEvent>;

    fn request_host_data(
        &mut self,
        offer_id: HostClipboardOfferId,
        mime_type: String,
        fd: OwnedFd,
    ) -> Result<(), ClipboardBridgeError>;

    fn publish_internal_selection(
        &mut self,
        generation: u64,
        mime_types: Vec<String>,
    ) -> Result<(), ClipboardBridgeError>;

    fn clear_internal_selection(&mut self) -> Result<(), ClipboardBridgeError>;
}

#[derive(Debug, Default)]
pub struct NoopClipboardBridge;

impl ClipboardBridge for NoopClipboardBridge {
    fn poll_events(&mut self) -> Vec<ClipboardBridgeEvent> {
        Vec::new()
    }

    fn request_host_data(
        &mut self,
        _offer_id: HostClipboardOfferId,
        _mime_type: String,
        _fd: OwnedFd,
    ) -> Result<(), ClipboardBridgeError> {
        Err(ClipboardBridgeError::Unavailable)
    }

    fn publish_internal_selection(
        &mut self,
        _generation: u64,
        _mime_types: Vec<String>,
    ) -> Result<(), ClipboardBridgeError> {
        Ok(())
    }

    fn clear_internal_selection(&mut self) -> Result<(), ClipboardBridgeError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::FromRawFd;

    #[test]
    fn noop_bridge_reports_no_host_events_and_keeps_internal_clipboard_nonfatal() {
        let mut bridge = NoopClipboardBridge;

        assert!(bridge.poll_events().is_empty());
        assert!(
            bridge
                .publish_internal_selection(7, vec!["text/plain".to_string()])
                .is_ok()
        );
        assert!(bridge.clear_internal_selection().is_ok());
    }

    #[test]
    fn noop_bridge_rejects_host_data_requests_without_blocking() {
        let mut bridge = NoopClipboardBridge;
        let fd = null_fd();

        let result =
            bridge.request_host_data(HostClipboardOfferId(3), "text/plain".to_string(), fd);

        assert_eq!(result, Err(ClipboardBridgeError::Unavailable));
    }

    fn null_fd() -> OwnedFd {
        let fd = unsafe { libc::dup(0) };
        assert!(fd >= 0);
        unsafe { OwnedFd::from_raw_fd(fd) }
    }
}
