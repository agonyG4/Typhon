use std::{
    fs, io,
    os::fd::AsRawFd,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use evdev::{Device, EventSummary, InputEvent, SynchronizationCode};

use super::frame::{
    ABS_AXIS_COUNT, AxisRange, ControllerFrameBuilder, ControllerInputEvent, KEY_CODE_COUNT,
    is_controller_button,
};
use super::semantic::{
    ControllerActionMask, ControllerSemanticFrame, ControllerSemanticMapper,
    ControllerSemanticMapping,
};

const KEY_WORDS: usize = KEY_CODE_COUNT.div_ceil(u64::BITS as usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ControllerDeviceId(u32);

impl ControllerDeviceId {
    pub(crate) const fn from_raw(value: u32) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub(crate) const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug)]
pub(crate) struct ControllerIdAllocator {
    next: u32,
    exhausted: bool,
}

impl Default for ControllerIdAllocator {
    fn default() -> Self {
        Self {
            next: 1,
            exhausted: false,
        }
    }
}

impl ControllerIdAllocator {
    pub(crate) fn allocate(&mut self) -> Option<ControllerDeviceId> {
        if self.exhausted {
            return None;
        }
        let id = ControllerDeviceId(self.next);
        if let Some(next) = self.next.checked_add(1) {
            self.next = next;
        } else {
            self.exhausted = true;
        }
        Some(id)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct InputDeviceMetadata {
    pub(crate) keyboard: bool,
    pub(crate) mouse: bool,
    pub(crate) touchpad: bool,
    pub(crate) gamepad_hint: bool,
    pub(crate) joystick_hint: bool,
}

impl InputDeviceMetadata {
    pub(crate) fn read_for_event_path(path: &Path, udev_data_root: &Path) -> Self {
        let Some(number) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("event"))
            .and_then(|number| number.parse::<u32>().ok())
        else {
            return Self::default();
        };
        let Some(minor) = number.checked_add(64) else {
            return Self::default();
        };
        let Ok(contents) = fs::read_to_string(udev_data_root.join(format!("c13:{minor}"))) else {
            return Self::default();
        };
        Self {
            keyboard: has_udev_tag(&contents, "ID_INPUT_KEYBOARD"),
            mouse: has_udev_tag(&contents, "ID_INPUT_MOUSE"),
            touchpad: has_udev_tag(&contents, "ID_INPUT_TOUCHPAD"),
            gamepad_hint: has_udev_tag(&contents, "ID_INPUT_GAMEPAD"),
            joystick_hint: has_udev_tag(&contents, "ID_INPUT_JOYSTICK"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ControllerCapabilities {
    keys: [u64; KEY_WORDS],
    absolute_axes: u64,
}

impl ControllerCapabilities {
    fn from_device(device: &Device) -> Self {
        let mut capabilities = Self::default();
        if let Some(keys) = device.supported_keys() {
            for key in keys.iter() {
                capabilities.insert_key(key.code());
            }
        }
        if let Some(axes) = device.supported_absolute_axes() {
            for axis in axes.iter() {
                capabilities.insert_axis(axis.0);
            }
        }
        capabilities
    }

    #[cfg(test)]
    fn from_codes(keys: &[u16], axes: &[u16]) -> Self {
        let mut capabilities = Self::default();
        for &key in keys {
            capabilities.insert_key(key);
        }
        for &axis in axes {
            capabilities.insert_axis(axis);
        }
        capabilities
    }

    fn insert_key(&mut self, code: u16) {
        let index = usize::from(code);
        if index < KEY_CODE_COUNT {
            self.keys[index / u64::BITS as usize] |= 1u64 << (index % u64::BITS as usize);
        }
    }

    fn insert_axis(&mut self, code: u16) {
        if usize::from(code) < ABS_AXIS_COUNT {
            self.absolute_axes |= 1u64 << code;
        }
    }

    fn has_axis(self, code: u16) -> bool {
        usize::from(code) < ABS_AXIS_COUNT && self.absolute_axes & (1u64 << code) != 0
    }

    fn credible_gamepad(self, metadata: InputDeviceMetadata) -> bool {
        let controller_button = self.keys.iter().enumerate().any(|(word, bits)| {
            let mut remaining = *bits;
            while remaining != 0 {
                let bit = remaining.trailing_zeros() as usize;
                let code = word * u64::BITS as usize + bit;
                if code <= u16::MAX as usize && is_credible_controller_button(code as u16) {
                    return true;
                }
                remaining &= remaining - 1;
            }
            false
        });
        let stick_pair = (self.has_axis(0x00) && self.has_axis(0x01))
            || (self.has_axis(0x03) && self.has_axis(0x04));
        let hat_pair = self.has_axis(0x10) && self.has_axis(0x11);
        let other_input_class = metadata.keyboard || metadata.mouse || metadata.touchpad;
        let has_controller_hint = metadata.gamepad_hint || metadata.joystick_hint;

        controller_button && (stick_pair || hat_pair) && (!other_input_class || has_controller_hint)
    }
}

fn is_credible_controller_button(code: u16) -> bool {
    (0x120..=0x13f).contains(&code) || (0x220..=0x223).contains(&code)
}

#[cfg(test)]
pub(crate) fn classify_controller_capabilities(
    keys: &[u16],
    absolute_axes: &[u16],
    metadata: InputDeviceMetadata,
) -> bool {
    ControllerCapabilities::from_codes(keys, absolute_axes).credible_gamepad(metadata)
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct ControllerIdentity {
    pub(crate) name: Option<String>,
    pub(crate) physical_path: Option<String>,
    pub(crate) unique_name: Option<String>,
    pub(crate) bus: u16,
    pub(crate) vendor: u16,
    pub(crate) product: u16,
    pub(crate) version: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransportFingerprint {
    device: u64,
    inode: u64,
    special_device: u64,
}

impl TransportFingerprint {
    pub(crate) fn read(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            special_device: metadata.rdev(),
        })
    }

    fn read_fd(fd: std::os::fd::RawFd) -> io::Result<Self> {
        let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
        if unsafe { libc::fstat(fd, &mut metadata) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            device: metadata.st_dev,
            inode: metadata.st_ino,
            special_device: metadata.st_rdev,
        })
    }
}

#[derive(Debug)]
pub(crate) struct ControllerDevice {
    id: ControllerDeviceId,
    path: PathBuf,
    fingerprint: TransportFingerprint,
    identity: ControllerIdentity,
    evdev: Device,
    frame_builder: ControllerFrameBuilder,
    semantic_mapper: ControllerSemanticMapper,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ControllerBatchStats {
    pub(super) raw_events: usize,
    pub(super) logical_frames: usize,
    pub(super) activity_transitions: usize,
    pub(super) semantic_frames: usize,
    pub(super) semantic_transitions: usize,
}

impl ControllerDevice {
    pub(crate) fn open(
        path: &Path,
        udev_data_root: &Path,
        ids: &mut ControllerIdAllocator,
    ) -> io::Result<Option<Self>> {
        let evdev = Device::open(path)?;
        evdev.set_nonblocking(true)?;
        let fingerprint = TransportFingerprint::read(path)?;
        if TransportFingerprint::read_fd(evdev.as_raw_fd())? != fingerprint {
            return Ok(None);
        }
        let metadata = InputDeviceMetadata::read_for_event_path(path, udev_data_root);
        let capabilities = ControllerCapabilities::from_device(&evdev);
        if !capabilities.credible_gamepad(metadata) {
            return Ok(None);
        }

        let input_id = evdev.input_id();
        let identity = ControllerIdentity {
            name: evdev.name().map(str::to_owned),
            physical_path: evdev.physical_path().map(str::to_owned),
            unique_name: evdev.unique_name().map(str::to_owned),
            bus: input_id.bus_type().0,
            vendor: input_id.vendor(),
            product: input_id.product(),
            version: input_id.version(),
        };
        let mut ranges = [AxisRange::default(); ABS_AXIS_COUNT];
        for (axis, info) in evdev.get_absinfo()? {
            let index = usize::from(axis.0);
            if index < ABS_AXIS_COUNT {
                ranges[index] =
                    AxisRange::new(info.minimum(), info.maximum()).with_initial_value(info.value());
            }
        }
        let mut frame_builder = ControllerFrameBuilder::new(ranges);
        let pressed_keys = evdev.get_key_state()?;
        for key in pressed_keys.iter() {
            let code = key.code();
            if is_controller_button(code) {
                frame_builder.seed_pressed_key(code);
            }
        }
        let semantic_mapper = ControllerSemanticMapper::seeded(
            ControllerSemanticMapping::default(),
            &frame_builder.state_snapshot(),
        );

        let Some(id) = ids.allocate() else {
            return Err(io::Error::other("controller device IDs exhausted"));
        };

        Ok(Some(Self {
            id,
            path: path.to_path_buf(),
            fingerprint,
            identity,
            evdev,
            frame_builder,
            semantic_mapper,
        }))
    }

    pub(crate) const fn id(&self) -> ControllerDeviceId {
        self.id
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) const fn fingerprint(&self) -> TransportFingerprint {
        self.fingerprint
    }

    #[allow(dead_code)]
    pub(crate) const fn identity(&self) -> &ControllerIdentity {
        &self.identity
    }

    pub(crate) fn raw_fd(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd;
        self.evdev.as_raw_fd()
    }

    /// Drains one evdev synchronized batch atomically before exposing its statistics.
    /// The infallible sink has no return path into synchronized iterator traversal.
    pub(crate) fn drain_synchronized_batch(
        &mut self,
        sink: &mut impl FnMut(ControllerDeviceId, ControllerSemanticFrame),
    ) -> io::Result<ControllerBatchStats> {
        let Self {
            id,
            evdev,
            frame_builder,
            semantic_mapper,
            ..
        } = self;
        let events = evdev.fetch_events()?;
        Ok(process_synchronized_events(
            events.map(controller_input_event),
            frame_builder,
            semantic_mapper,
            *id,
            sink,
        ))
    }

    pub(crate) fn clear_session_state(&mut self) {
        self.frame_builder.clear_session_state();
        self.semantic_mapper.clear();
    }

    pub(crate) fn semantic_state(&self) -> ControllerActionMask {
        self.semantic_mapper.held()
    }
}

fn controller_input_event(event: InputEvent) -> ControllerInputEvent {
    match event.destructure() {
        EventSummary::Key(_, code, value) => ControllerInputEvent::Key {
            code: code.0,
            value,
        },
        EventSummary::AbsoluteAxis(_, code, value) => ControllerInputEvent::Absolute {
            code: code.0,
            value,
        },
        EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
            ControllerInputEvent::SynReport
        }
        _ => ControllerInputEvent::Other,
    }
}

pub(super) fn process_synchronized_events(
    events: impl IntoIterator<Item = ControllerInputEvent>,
    frame_builder: &mut ControllerFrameBuilder,
    semantic_mapper: &mut ControllerSemanticMapper,
    device_id: ControllerDeviceId,
    sink: &mut impl FnMut(ControllerDeviceId, ControllerSemanticFrame),
) -> ControllerBatchStats {
    let mut stats = ControllerBatchStats::default();
    // FetchEventsSynced drops complete SYN_REPORT blocks through its consumed_to marker, so this
    // traversal must exhaust the iterator before returning. The sink has no result or break
    // channel into this loop.
    for event in events {
        stats.raw_events = stats.raw_events.saturating_add(1);
        if let Some(frame) = frame_builder.process(event) {
            stats.logical_frames = stats.logical_frames.saturating_add(1);
            if frame.activity_transition {
                stats.activity_transitions = stats.activity_transitions.saturating_add(1);
            }
            if let Some(semantic_frame) = semantic_mapper.map_frame(&frame) {
                stats.semantic_frames = stats.semantic_frames.saturating_add(1);
                stats.semantic_transitions = stats
                    .semantic_transitions
                    .saturating_add(semantic_frame.pressed.count() as usize)
                    .saturating_add(semantic_frame.released.count() as usize);
                sink(device_id, semantic_frame);
            }
        }
    }
    stats
}

fn has_udev_tag(contents: &str, name: &str) -> bool {
    contents.lines().any(|line| line == format!("E:{name}=1"))
}
