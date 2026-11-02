use super::*;

use std::fmt;

const UEVENT_KERNEL_GROUP: u32 = 1;
const UEVENT_BUFFER_BYTES: usize = 8192;
// A single hotplug burst (one connector toggling, or a dock with several
// connectors) is a handful of datagrams. The cap only bounds one reactor turn;
// the socket is registered level-triggered, so leftovers wake the next turn.
const UEVENT_READS_PER_DRAIN: usize = 64;

/// Kernel `NETLINK_KOBJECT_UEVENT` listener filtered to DRM hotplug events for
/// the runtime's KMS card.
///
/// It reports only that the connector set may have changed; the caller re-reads
/// the connector inventory to learn what changed.
pub(crate) struct DrmHotplugMonitor {
    socket: OwnedFd,
    card_name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DrmHotplugDrain {
    pub(crate) hotplug: bool,
    pub(crate) datagrams: usize,
}

impl DrmHotplugMonitor {
    pub(crate) fn open(kms_device: &Path) -> io::Result<Self> {
        let card_name = kms_device
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| name.starts_with("card"))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not a DRM card node", kms_device.display()),
                )
            })?
            .to_owned();
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_DGRAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                libc::NETLINK_KOBJECT_UEVENT,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let socket = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        address.nl_groups = UEVENT_KERNEL_GROUP;
        let result = unsafe {
            libc::bind(
                socket.as_raw_fd(),
                (&raw const address).cast(),
                std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { socket, card_name })
    }

    pub(crate) fn raw_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }

    pub(crate) fn card_name(&self) -> &str {
        &self.card_name
    }

    pub(crate) fn drain(&self) -> io::Result<DrmHotplugDrain> {
        let mut drain = DrmHotplugDrain::default();
        let mut buffer = [0u8; UEVENT_BUFFER_BYTES];
        for _ in 0..UEVENT_READS_PER_DRAIN {
            let mut sender: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
            let mut sender_len = std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t;
            let result = unsafe {
                libc::recvfrom(
                    self.socket.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    0,
                    (&raw mut sender).cast(),
                    &raw mut sender_len,
                )
            };
            if result < 0 {
                let error = io::Error::last_os_error();
                match error.kind() {
                    io::ErrorKind::WouldBlock => return Ok(drain),
                    io::ErrorKind::Interrupted => continue,
                    // The kernel dropped datagrams under pressure. Whatever was
                    // lost, the inventory rescan observes the current state.
                    _ if error.raw_os_error() == Some(libc::ENOBUFS) => {
                        drain.hotplug = true;
                        continue;
                    }
                    _ => return Err(error),
                }
            }
            drain.datagrams += 1;
            // Only the kernel (port id 0) is a trusted uevent source; any local
            // process can send to this multicast group.
            if sender.nl_pid != 0 {
                continue;
            }
            let length = (result as usize).min(buffer.len());
            if is_drm_hotplug_uevent(&buffer[..length], &self.card_name) {
                drain.hotplug = true;
            }
        }
        Ok(drain)
    }
}

/// Kernel uevent datagrams are `ACTION@DEVPATH\0KEY=VALUE\0...`.
pub(crate) fn is_drm_hotplug_uevent(datagram: &[u8], card_name: &str) -> bool {
    let mut fields = datagram.split(|byte| *byte == 0);
    let Some(header) = fields.next() else {
        return false;
    };
    if !header.contains(&b'@') {
        return false;
    }
    let mut drm = false;
    let mut hotplug = false;
    let mut card = false;
    for field in fields {
        let Some(separator) = field.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let (key, value) = (&field[..separator], &field[separator + 1..]);
        match key {
            b"SUBSYSTEM" => drm = value == b"drm",
            b"HOTPLUG" => hotplug = value == b"1",
            b"DEVNAME" => {
                card = value
                    .strip_prefix(b"dri/")
                    .is_some_and(|name| name == card_name.as_bytes());
            }
            _ => {}
        }
    }
    drm && hotplug && card
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectorInventoryEntry {
    pub(crate) connector_id: u32,
    pub(crate) name: String,
    pub(crate) connected: bool,
    pub(crate) physical_size_mm: (u32, u32),
    pub(crate) preferred_mode: Option<(u16, u16, u32)>,
    pub(crate) mode_count: usize,
}

impl ConnectorInventoryEntry {
    /// Whether the same connector now carries a different sink. Without EDID the
    /// physical size and preferred mode are the cheapest stable proxy.
    fn sink_differs(&self, other: &Self) -> bool {
        self.connected != other.connected
            || self.physical_size_mm != other.physical_size_mm
            || self.preferred_mode != other.preferred_mode
            || self.mode_count != other.mode_count
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ConnectorInventory {
    entries: Vec<ConnectorInventoryEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ConnectorInventoryDelta {
    pub(crate) connected: Vec<ConnectorInventoryEntry>,
    pub(crate) disconnected: Vec<ConnectorInventoryEntry>,
    pub(crate) changed: Vec<ConnectorInventoryEntry>,
}

impl ConnectorInventoryDelta {
    pub(crate) fn is_empty(&self) -> bool {
        self.connected.is_empty() && self.disconnected.is_empty() && self.changed.is_empty()
    }

    pub(crate) fn touches(&self, connector_id: u32) -> bool {
        self.connected
            .iter()
            .chain(&self.disconnected)
            .chain(&self.changed)
            .any(|entry| entry.connector_id == connector_id)
    }
}

impl ConnectorInventory {
    #[cfg(test)]
    pub(crate) fn from_entries(mut entries: Vec<ConnectorInventoryEntry>) -> Self {
        entries.sort_by_key(|entry| entry.connector_id);
        Self { entries }
    }

    /// Reads connector state without forcing a probe: after a hotplug uevent
    /// the kernel has already probed, and forcing would re-read EDID on every
    /// connector from the compositor thread.
    pub(crate) fn query(file: &fs::File) -> io::Result<Self> {
        let mut connector_ids = Vec::new();
        drm_ffi::mode::get_resources(file.as_fd(), None, None, Some(&mut connector_ids), None)?;
        let mut entries = Vec::with_capacity(connector_ids.len());
        for connector_id in connector_ids {
            let mut modes = Vec::new();
            let connector = drm_ffi::mode::get_connector(
                file.as_fd(),
                connector_id,
                None,
                None,
                Some(&mut modes),
                None,
                false,
            )?;
            let preferred_mode = modes
                .iter()
                .find(|mode| mode.type_ & drm_sys::DRM_MODE_TYPE_PREFERRED != 0)
                .or_else(|| modes.first())
                .map(|mode| {
                    (
                        mode.hdisplay,
                        mode.vdisplay,
                        drm_mode_refresh_millihz(mode).unwrap_or(0),
                    )
                });
            entries.push(ConnectorInventoryEntry {
                connector_id,
                name: drm_connector_presentation_name(&connector),
                connected: connector.connection == 1,
                physical_size_mm: (connector.mm_width, connector.mm_height),
                preferred_mode,
                mode_count: modes.len(),
            });
        }
        entries.sort_by_key(|entry| entry.connector_id);
        Ok(Self { entries })
    }

    pub(crate) fn connected(&self) -> impl Iterator<Item = &ConnectorInventoryEntry> {
        self.entries.iter().filter(|entry| entry.connected)
    }

    pub(crate) fn entry(&self, connector_id: u32) -> Option<&ConnectorInventoryEntry> {
        self.entries
            .iter()
            .find(|entry| entry.connector_id == connector_id)
    }

    pub(crate) fn diff(&self, next: &Self) -> ConnectorInventoryDelta {
        let mut delta = ConnectorInventoryDelta::default();
        for entry in &next.entries {
            match self.entry(entry.connector_id) {
                None if entry.connected => delta.connected.push(entry.clone()),
                None => {}
                Some(previous) if previous.connected && !entry.connected => {
                    delta.disconnected.push(previous.clone());
                }
                Some(previous) if !previous.connected && entry.connected => {
                    delta.connected.push(entry.clone());
                }
                Some(previous) if entry.connected && previous.sink_differs(entry) => {
                    delta.changed.push(entry.clone());
                }
                Some(_) => {}
            }
        }
        // Connectors that vanished entirely (DP MST branches) are disconnects.
        for previous in &self.entries {
            if previous.connected && next.entry(previous.connector_id).is_none() {
                delta.disconnected.push(previous.clone());
            }
        }
        delta
    }
}

impl fmt::Display for ConnectorInventoryEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.name, self.connector_id)?;
        if let Some((width, height, refresh_millihz)) = self.preferred_mode {
            write!(
                f,
                " {width}x{height}@{}.{:03}",
                refresh_millihz / 1000,
                refresh_millihz % 1000
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uevent(fields: &[&str]) -> Vec<u8> {
        let mut datagram = b"change@/devices/pci0000:00/0000:01:00.0/drm/card1".to_vec();
        for field in fields {
            datagram.push(0);
            datagram.extend_from_slice(field.as_bytes());
        }
        datagram.push(0);
        datagram
    }

    #[test]
    fn hotplug_uevent_for_runtime_card_is_accepted() {
        let datagram = uevent(&[
            "ACTION=change",
            "DEVPATH=/devices/pci0000:00/0000:01:00.0/drm/card1",
            "SUBSYSTEM=drm",
            "HOTPLUG=1",
            "CONNECTOR=96",
            "DEVNAME=dri/card1",
            "SEQNUM=4242",
        ]);
        assert!(is_drm_hotplug_uevent(&datagram, "card1"));
    }

    #[test]
    fn hotplug_uevent_for_another_card_is_ignored() {
        let datagram = uevent(&["SUBSYSTEM=drm", "HOTPLUG=1", "DEVNAME=dri/card10"]);
        assert!(!is_drm_hotplug_uevent(&datagram, "card1"));
        assert!(is_drm_hotplug_uevent(&datagram, "card10"));
    }

    #[test]
    fn non_hotplug_drm_and_other_subsystems_are_ignored() {
        let lease = uevent(&["SUBSYSTEM=drm", "LEASE=1", "DEVNAME=dri/card1"]);
        let input = uevent(&["SUBSYSTEM=input", "HOTPLUG=1", "DEVNAME=dri/card1"]);
        let render = uevent(&["SUBSYSTEM=drm", "HOTPLUG=1", "DEVNAME=dri/renderD128"]);
        assert!(!is_drm_hotplug_uevent(&lease, "card1"));
        assert!(!is_drm_hotplug_uevent(&input, "card1"));
        assert!(!is_drm_hotplug_uevent(&render, "card1"));
    }

    #[test]
    fn udev_rebroadcast_without_header_is_ignored() {
        assert!(!is_drm_hotplug_uevent(
            b"libudev\0SUBSYSTEM=drm\0HOTPLUG=1\0DEVNAME=dri/card1\0",
            "card1"
        ));
        assert!(!is_drm_hotplug_uevent(b"", "card1"));
    }

    fn entry(connector_id: u32, name: &str, connected: bool) -> ConnectorInventoryEntry {
        ConnectorInventoryEntry {
            connector_id,
            name: name.to_owned(),
            connected,
            physical_size_mm: if connected { (600, 340) } else { (0, 0) },
            preferred_mode: connected.then_some((2560, 1440, 164_000)),
            mode_count: if connected { 12 } else { 0 },
        }
    }

    #[test]
    fn inventory_diff_reports_connects_and_disconnects() {
        let before = ConnectorInventory::from_entries(vec![
            entry(96, "DP-1", true),
            entry(97, "DP-2", false),
        ]);
        let after = ConnectorInventory::from_entries(vec![
            entry(96, "DP-1", false),
            entry(97, "DP-2", true),
        ]);
        let delta = before.diff(&after);
        assert_eq!(delta.connected, vec![entry(97, "DP-2", true)]);
        assert_eq!(delta.disconnected, vec![entry(96, "DP-1", true)]);
        assert!(delta.changed.is_empty());
        assert!(delta.touches(96) && delta.touches(97) && !delta.touches(98));
    }

    #[test]
    fn inventory_diff_reports_sink_swap_on_same_connector() {
        let before = ConnectorInventory::from_entries(vec![entry(96, "DP-1", true)]);
        let mut swapped = entry(96, "DP-1", true);
        swapped.preferred_mode = Some((3840, 2160, 60_000));
        swapped.physical_size_mm = (700, 390);
        let after = ConnectorInventory::from_entries(vec![swapped.clone()]);
        let delta = before.diff(&after);
        assert_eq!(delta.changed, vec![swapped]);
        assert!(delta.connected.is_empty() && delta.disconnected.is_empty());
    }

    #[test]
    fn inventory_diff_is_empty_for_identical_state_and_handles_vanished_connectors() {
        let before = ConnectorInventory::from_entries(vec![
            entry(96, "DP-1", true),
            entry(120, "DP-3", true),
        ]);
        assert!(before.diff(&before).is_empty());
        let after = ConnectorInventory::from_entries(vec![entry(96, "DP-1", true)]);
        let delta = before.diff(&after);
        assert_eq!(delta.disconnected, vec![entry(120, "DP-3", true)]);
    }
}
