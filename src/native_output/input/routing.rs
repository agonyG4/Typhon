use super::*;

pub(crate) enum NativeInputEventFds<'a> {
    Libinput(Option<RawFd>),
    Raw(std::slice::Iter<'a, NativeInputDevice>),
}

impl Iterator for NativeInputEventFds<'_> {
    type Item = RawFd;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Libinput(fd) => fd.take(),
            Self::Raw(devices) => devices.next().map(|device| device.file.as_raw_fd()),
        }
    }
}

impl NativeInputBackend {
    pub(crate) fn open(
        plan: NativeInputBackendPlan,
        output_width: u32,
        output_height: u32,
        seat_session: Option<NativeSeatSession>,
    ) -> io::Result<Self> {
        let mut last_error = None;
        for candidate in plan.candidates() {
            match Self::open_kind(candidate, output_width, output_height, seat_session.clone()) {
                Ok(backend) => return Ok(backend),
                Err(error) => {
                    eprintln!(
                        "native input: {} backend failed: {error}",
                        candidate.as_str()
                    );
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            io::Error::other(
                "native input unavailable: no libseat/libinput backend or readable raw evdev fallback",
            )
        }))
    }

    pub(crate) fn open_kind(
        kind: NativeInputBackendKind,
        output_width: u32,
        output_height: u32,
        seat_session: Option<NativeSeatSession>,
    ) -> io::Result<Self> {
        match kind {
            NativeInputBackendKind::LibseatLibinputUdev => {
                let session = seat_session.ok_or_else(|| {
                    io::Error::other("libseat/libinput requested but no active seat session exists")
                })?;
                let backend = LibinputInputBackend::open_with_libseat(
                    session,
                    "seat0",
                    output_width,
                    output_height,
                )?;
                backend.ensure_initial_devices()?;
                Ok(Self::LibseatLibinput(backend))
            }
            NativeInputBackendKind::DirectLibinputUdev => {
                let backend =
                    LibinputInputBackend::open_direct("seat0", output_width, output_height)?;
                backend.ensure_initial_devices()?;
                Ok(Self::DirectLibinput(backend))
            }
            NativeInputBackendKind::RawEvdev => {
                Ok(Self::RawEvdev(NativeInputDevices::open_readable()))
            }
            NativeInputBackendKind::Unavailable => {
                Err(io::Error::other("native input backend is unavailable"))
            }
        }
    }

    pub(crate) const fn kind(&self) -> NativeInputBackendKind {
        match self {
            Self::LibseatLibinput(_) => NativeInputBackendKind::LibseatLibinputUdev,
            Self::DirectLibinput(_) => NativeInputBackendKind::DirectLibinputUdev,
            Self::RawEvdev(_) => NativeInputBackendKind::RawEvdev,
        }
    }

    pub(crate) fn event_fds(&self) -> NativeInputEventFds<'_> {
        match self {
            Self::LibseatLibinput(backend) | Self::DirectLibinput(backend) => {
                NativeInputEventFds::Libinput(Some(backend.input.as_fd().as_raw_fd()))
            }
            Self::RawEvdev(backend) => NativeInputEventFds::Raw(backend.devices.iter()),
        }
    }

    pub(crate) fn suspend_for_session(&mut self) {
        match self {
            Self::LibseatLibinput(backend) | Self::DirectLibinput(backend) => {
                backend.suspend_for_session();
            }
            Self::RawEvdev(backend) => backend.suspend_for_session(),
        }
    }

    pub(crate) fn resume_after_session(&mut self) -> io::Result<()> {
        match self {
            Self::LibseatLibinput(backend) | Self::DirectLibinput(backend) => {
                backend.resume_after_session()?;
            }
            Self::RawEvdev(backend) => backend.resume_after_session(),
        }
        Ok(())
    }

    pub(crate) fn discard_suspended_events(&mut self) {
        match self {
            Self::LibseatLibinput(backend) | Self::DirectLibinput(backend) => {
                backend.discard_events_unconditionally();
            }
            Self::RawEvdev(backend) => backend.discard_events_unconditionally(),
        }
    }

    pub(crate) fn drain_events(&mut self) -> Vec<NativeHardwareInputEvent> {
        match self {
            Self::LibseatLibinput(backend) | Self::DirectLibinput(backend) => {
                backend.drain_events()
            }
            Self::RawEvdev(backend) => backend.drain_events(),
        }
    }
}

pub(crate) struct LibinputInputBackend {
    pub(crate) input: ::input::Libinput,
    pub(crate) seat_name: String,
    pub(crate) output_width: u32,
    pub(crate) output_height: u32,
    pub(crate) device_count: usize,
    pub(crate) suspended: bool,
}

impl LibinputInputBackend {
    pub(crate) fn open_with_libseat(
        seat_session: NativeSeatSession,
        seat_name: &str,
        output_width: u32,
        output_height: u32,
    ) -> io::Result<Self> {
        let assigned_seat = seat_session
            .seat_name()
            .unwrap_or_else(|| seat_name.to_string());
        let interface = SeatLibinputInterface::new(seat_session.clone());
        let mut input = ::input::Libinput::new_with_udev(interface);
        input.udev_assign_seat(&assigned_seat).map_err(|()| {
            io::Error::other(format!("failed to assign libinput seat {assigned_seat}"))
        })?;
        input.dispatch()?;
        let device_count = drain_initial_libinput_device_events(&mut input);
        println!(
            "native input: libseat/libinput assigned {assigned_seat}, {device_count} device(s)"
        );
        Ok(Self {
            input,
            seat_name: assigned_seat,
            output_width,
            output_height,
            device_count,
            suspended: false,
        })
    }

    pub(crate) fn open_direct(
        seat_name: &str,
        output_width: u32,
        output_height: u32,
    ) -> io::Result<Self> {
        let mut input = ::input::Libinput::new_with_udev(DirectLibinputInterface);
        input.udev_assign_seat(seat_name).map_err(|()| {
            io::Error::other(format!("failed to assign libinput seat {seat_name}"))
        })?;
        input.dispatch()?;
        let device_count = drain_initial_libinput_device_events(&mut input);
        println!("native input: libinput assigned {seat_name}, {device_count} device(s)");
        Ok(Self {
            input,
            seat_name: seat_name.to_string(),
            output_width,
            output_height,
            device_count,
            suspended: false,
        })
    }

    pub(crate) fn drain_events(&mut self) -> Vec<NativeHardwareInputEvent> {
        if self.suspended {
            return Vec::new();
        }
        self.drain_events_unconditionally()
    }

    fn drain_events_unconditionally(&mut self) -> Vec<NativeHardwareInputEvent> {
        let mut events = Vec::new();
        if let Err(error) = self.input.dispatch() {
            eprintln!("native input: libinput dispatch failed: {error}");
            return events;
        }
        for event in &mut self.input {
            if let Some(event) =
                hardware_input_event_from_libinput(event, self.output_width, self.output_height)
            {
                events.push(event);
                if events.len() >= 256 {
                    break;
                }
            }
        }
        events
    }

    fn discard_events_unconditionally(&mut self) {
        while self.drain_events_unconditionally().len() == 256 {}
    }

    fn suspend_for_session(&mut self) {
        self.suspended = true;
        self.input.suspend();
        self.discard_events_unconditionally();
    }

    fn resume_after_session(&mut self) -> io::Result<()> {
        self.input
            .resume()
            .map_err(|()| io::Error::other("failed to resume libinput"))?;
        self.suspended = false;
        self.discard_events_unconditionally();
        Ok(())
    }

    pub(crate) fn ensure_initial_devices(&self) -> io::Result<()> {
        if self.device_count == 0 {
            Err(io::Error::other(format!(
                "libinput seat {} reported no keyboard or pointer devices",
                self.seat_name
            )))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeSeatEvent {
    Enabled,
    Disabled,
}

#[derive(Clone)]
pub(crate) struct NativeSeatSession {
    pub(crate) inner: Rc<RefCell<NativeSeatSessionInner>>,
    pub(crate) events: Rc<RefCell<Vec<NativeSeatEvent>>>,
    pub(crate) active: Rc<Cell<bool>>,
    pub(crate) disable_pending: Rc<Cell<bool>>,
}

pub(crate) struct NativeSeatSessionInner {
    pub(crate) seat: libseat::Seat,
    pub(crate) devices: HashMap<RawFd, libseat::Device>,
}

pub(crate) struct NativeSeatDeviceFile {
    pub(crate) file: Option<fs::File>,
    pub(crate) key: RawFd,
    pub(crate) session: NativeSeatSession,
}

impl NativeSeatDeviceFile {
    pub(crate) fn file(&self) -> &fs::File {
        self.file
            .as_ref()
            .expect("seat-managed native device file should be open")
    }
}

impl Drop for NativeSeatDeviceFile {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            drop(file);
        }
        self.session.close_device_key(self.key);
    }
}

impl NativeSeatSession {
    pub(crate) fn open() -> io::Result<Self> {
        let active = Rc::new(Cell::new(false));
        let events = Rc::new(RefCell::new(Vec::new()));
        let disable_pending = Rc::new(Cell::new(false));
        let callback_active = Rc::clone(&active);
        let callback_events = Rc::clone(&events);
        let callback_disable_pending = Rc::clone(&disable_pending);
        let seat = libseat::Seat::open(move |_seat, event| match event {
            libseat::SeatEvent::Enable => {
                callback_disable_pending.set(false);
                callback_active.set(true);
                callback_events.borrow_mut().push(NativeSeatEvent::Enabled);
            }
            libseat::SeatEvent::Disable => {
                callback_active.set(false);
                if !callback_disable_pending.replace(true) {
                    callback_events.borrow_mut().push(NativeSeatEvent::Disabled);
                }
            }
        })
        .map_err(io::Error::from)?;

        let session = Self {
            inner: Rc::new(RefCell::new(NativeSeatSessionInner {
                seat,
                devices: HashMap::new(),
            })),
            events,
            active,
            disable_pending,
        };
        session.wait_for_activation()?;
        Ok(session)
    }

    pub(crate) fn seat_name(&self) -> Option<String> {
        let mut inner = self.inner.try_borrow_mut().ok()?;
        Some(inner.seat.name().to_string())
    }

    pub(crate) fn dispatch(&self) -> io::Result<()> {
        let mut inner = self.inner.borrow_mut();
        inner.seat.dispatch(0).map(|_| ()).map_err(io::Error::from)
    }

    pub(crate) fn event_fd(&self) -> io::Result<RawFd> {
        let mut inner = self.inner.borrow_mut();
        inner
            .seat
            .get_fd()
            .map(|fd| fd.as_raw_fd())
            .map_err(io::Error::from)
    }

    pub(crate) fn acknowledge_disable(&self) -> io::Result<bool> {
        if !self.disable_pending.replace(false) {
            return Ok(false);
        }
        let mut inner = self.inner.borrow_mut();
        inner.seat.disable().map(|()| true).map_err(io::Error::from)
    }

    pub(crate) fn switch_session(&self, session: i32) -> io::Result<()> {
        let mut inner = self.inner.borrow_mut();
        inner.seat.switch_session(session).map_err(io::Error::from)
    }

    pub(crate) fn wait_for_activation(&self) -> io::Result<()> {
        for _ in 0..10 {
            if self.active.get() {
                return Ok(());
            }
            let mut inner = self.inner.borrow_mut();
            inner.seat.dispatch(50).map_err(io::Error::from)?;
        }
        if self.active.get() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "libseat did not activate this session",
            ))
        }
    }

    pub(crate) fn drain_events(&self) -> Vec<NativeSeatEvent> {
        std::mem::take(&mut *self.events.borrow_mut())
    }

    pub(crate) fn open_device_file(&self, path: &Path) -> io::Result<NativeSeatDeviceFile> {
        let fd = self
            .open_device_fd(path)
            .map_err(io::Error::from_raw_os_error)?;
        let key = fd.as_raw_fd();
        Ok(NativeSeatDeviceFile {
            file: Some(fs::File::from(fd)),
            key,
            session: self.clone(),
        })
    }

    fn open_restricted(&self, path: &Path, _flags: i32) -> Result<OwnedFd, i32> {
        self.open_device_fd(path)
    }

    pub(crate) fn open_device_fd(&self, path: &Path) -> Result<OwnedFd, i32> {
        if !self.active.get() {
            return Err(libc::EACCES);
        }
        let mut inner = self.inner.borrow_mut();
        let device = inner.seat.open_device(&path).map_err(i32::from)?;
        let duplicated_fd = duplicate_fd_cloexec(device.as_fd().as_raw_fd())?;
        let key = duplicated_fd.as_raw_fd();
        inner.devices.insert(key, device);
        Ok(duplicated_fd)
    }

    fn close_restricted(&self, fd: OwnedFd) {
        let key = fd.as_raw_fd();
        drop(fd);
        self.close_device_key(key);
    }

    pub(crate) fn close_device_key(&self, key: RawFd) {
        let mut inner = self.inner.borrow_mut();
        let Some(device) = inner.devices.remove(&key) else {
            return;
        };
        if let Err(error) = inner.seat.close_device(device) {
            eprintln!("native seat: failed to close libseat device: {error}");
        }
    }
}

pub(crate) fn duplicate_fd_cloexec(fd: RawFd) -> Result<OwnedFd, i32> {
    let duplicated = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicated < 0 {
        Err(io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO))
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(duplicated) })
    }
}

#[derive(Clone)]
pub(crate) struct SeatLibinputInterface {
    pub(crate) session: NativeSeatSession,
}

impl SeatLibinputInterface {
    pub(crate) fn new(session: NativeSeatSession) -> Self {
        Self { session }
    }
}

impl ::input::LibinputInterface for SeatLibinputInterface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        self.session.open_restricted(path, flags)
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        self.session.close_restricted(fd);
    }
}

pub(crate) struct DirectLibinputInterface;

impl ::input::LibinputInterface for DirectLibinputInterface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        let access_mode = flags & libc::O_ACCMODE;
        OpenOptions::new()
            .custom_flags(flags | libc::O_CLOEXEC)
            .read(matches!(access_mode, libc::O_RDONLY | libc::O_RDWR))
            .write(matches!(access_mode, libc::O_WRONLY | libc::O_RDWR))
            .open(path)
            .map(Into::into)
            .map_err(|error| error.raw_os_error().unwrap_or(libc::EIO))
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        drop(fd);
    }
}

pub(crate) fn drain_initial_libinput_device_events(input: &mut ::input::Libinput) -> usize {
    input
        .filter(|event| matches!(event, ::input::Event::Device(_)))
        .count()
}

pub(crate) fn hardware_input_event_from_libinput(
    event: ::input::Event,
    output_width: u32,
    output_height: u32,
) -> Option<NativeHardwareInputEvent> {
    use ::input::event::keyboard::{KeyState, KeyboardEvent, KeyboardEventTrait};
    #[allow(deprecated)]
    use ::input::event::pointer::{
        Axis, ButtonState, PointerEvent, PointerEventTrait, PointerScrollEvent,
    };

    match event {
        ::input::Event::Keyboard(KeyboardEvent::Key(event)) => {
            let code = u16::try_from(event.key()).ok()?;
            let value = match event.key_state() {
                KeyState::Pressed => 1,
                KeyState::Released => 0,
            };
            Some(NativeHardwareInputEvent::Key { code, value })
        }
        ::input::Event::Pointer(PointerEvent::Motion(event)) => Some(
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                event.time_usec(),
                RelativeMotion {
                    dx: event.dx(),
                    dy: event.dy(),
                    dx_unaccelerated: event.dx_unaccelerated(),
                    dy_unaccelerated: event.dy_unaccelerated(),
                },
            )),
        ),
        ::input::Event::Pointer(PointerEvent::MotionAbsolute(event)) => Some(
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::absolute(
                event.time_usec(),
                event.absolute_x_transformed(output_width),
                event.absolute_y_transformed(output_height),
            )),
        ),
        ::input::Event::Pointer(PointerEvent::Button(event)) => {
            Some(NativeHardwareInputEvent::PointerButton {
                button: event.button(),
                pressed: event.button_state() == ButtonState::Pressed,
            })
        }
        #[allow(deprecated)]
        ::input::Event::Pointer(PointerEvent::Axis(event)) => {
            let horizontal = libinput_scroll_axis_value(event.has_axis(Axis::Horizontal), || {
                event.axis_value(Axis::Horizontal)
            });
            let vertical = libinput_scroll_axis_value(event.has_axis(Axis::Vertical), || {
                event.axis_value(Axis::Vertical)
            });
            Some(NativeHardwareInputEvent::PointerAxis {
                horizontal,
                vertical,
            })
        }
        ::input::Event::Pointer(PointerEvent::ScrollWheel(event)) => {
            let horizontal = libinput_scroll_axis_value(event.has_axis(Axis::Horizontal), || {
                event.scroll_value(Axis::Horizontal)
            });
            let vertical = libinput_scroll_axis_value(event.has_axis(Axis::Vertical), || {
                event.scroll_value(Axis::Vertical)
            });
            Some(NativeHardwareInputEvent::PointerAxis {
                horizontal,
                vertical,
            })
        }
        ::input::Event::Pointer(PointerEvent::ScrollFinger(event)) => {
            let horizontal = libinput_scroll_axis_value(event.has_axis(Axis::Horizontal), || {
                event.scroll_value(Axis::Horizontal)
            });
            let vertical = libinput_scroll_axis_value(event.has_axis(Axis::Vertical), || {
                event.scroll_value(Axis::Vertical)
            });
            Some(NativeHardwareInputEvent::PointerAxis {
                horizontal,
                vertical,
            })
        }
        ::input::Event::Pointer(PointerEvent::ScrollContinuous(event)) => {
            let horizontal = libinput_scroll_axis_value(event.has_axis(Axis::Horizontal), || {
                event.scroll_value(Axis::Horizontal)
            });
            let vertical = libinput_scroll_axis_value(event.has_axis(Axis::Vertical), || {
                event.scroll_value(Axis::Vertical)
            });
            Some(NativeHardwareInputEvent::PointerAxis {
                horizontal,
                vertical,
            })
        }
        _ => None,
    }
}

pub(crate) fn libinput_scroll_axis_value<F>(has_axis: bool, read_value: F) -> f64
where
    F: FnOnce() -> f64,
{
    if has_axis { read_value() } else { 0.0 }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum PendingPointerMotion {
    Sample(PointerMotionSample),
}

pub(crate) fn coalesce_pointer_motion_events(
    events: Vec<NativeHardwareInputEvent>,
) -> Vec<NativeHardwareInputEvent> {
    let mut coalesced = Vec::with_capacity(events.len());
    let mut pending_motion = None;

    for event in events {
        match event {
            NativeHardwareInputEvent::PointerMotion(sample) => match pending_motion {
                Some(PendingPointerMotion::Sample(pending_sample)) => {
                    if let Some(coalesced_sample) = pending_sample.coalesce(sample) {
                        pending_motion = Some(PendingPointerMotion::Sample(coalesced_sample));
                    } else {
                        flush_pending_pointer_motion(
                            &mut coalesced,
                            PendingPointerMotion::Sample(pending_sample),
                        );
                        pending_motion = Some(PendingPointerMotion::Sample(sample));
                    }
                }
                None => pending_motion = Some(PendingPointerMotion::Sample(sample)),
            },
            event => {
                if let Some(pending) = pending_motion.take() {
                    flush_pending_pointer_motion(&mut coalesced, pending);
                }
                coalesced.push(event);
            }
        }
    }

    if let Some(pending) = pending_motion {
        flush_pending_pointer_motion(&mut coalesced, pending);
    }

    coalesced
}

pub(crate) fn flush_pending_pointer_motion(
    events: &mut Vec<NativeHardwareInputEvent>,
    pending: PendingPointerMotion,
) {
    match pending {
        PendingPointerMotion::Sample(sample) => {
            events.push(NativeHardwareInputEvent::PointerMotion(sample));
        }
    }
}

#[derive(Debug)]
pub(crate) struct NativeInputDevice {
    pub(crate) file: fs::File,
    pub(crate) path: PathBuf,
}

#[derive(Debug, Default)]
pub(crate) struct NativeInputDevices {
    pub(crate) devices: Vec<NativeInputDevice>,
    pub(crate) suspended: bool,
}

impl NativeInputDevices {
    pub(crate) fn open_readable() -> Self {
        let mut devices = Vec::new();
        let mut denied_paths = Vec::new();
        for path in input_event_paths(Path::new("/dev/input")) {
            match OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
                .open(&path)
            {
                Ok(file) => devices.push(NativeInputDevice { file, path }),
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    denied_paths.push(path);
                }
                Err(_) => {}
            }
        }

        if !denied_paths.is_empty() {
            eprintln!(
                "native input: permission denied for {} keyboard/mouse device(s): {}",
                denied_paths.len(),
                denied_paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            eprintln!(
                "native input: add the user to the input group or run through a seat manager to grant raw input access"
            );
        }
        if devices.is_empty() {
            eprintln!(
                "native input: no readable /dev/input/event* devices; keyboard/mouse disabled"
            );
        } else {
            println!("native input: opened {} device(s)", devices.len());
        }
        Self {
            devices,
            suspended: false,
        }
    }

    pub(crate) fn drain_events(&mut self) -> Vec<NativeHardwareInputEvent> {
        if self.suspended {
            return Vec::new();
        }
        self.drain_events_unconditionally()
    }

    fn drain_events_unconditionally(&mut self) -> Vec<NativeHardwareInputEvent> {
        let mut events = Vec::new();
        for device in &mut self.devices {
            while let Some(event) = read_linux_input_event(device) {
                if let Some(event) = NativeHardwareInputEvent::from_linux_event(event) {
                    events.push(event);
                }
                if events.len() >= 256 {
                    return events;
                }
            }
        }
        events
    }

    fn discard_events_unconditionally(&mut self) {
        while !self.drain_events_unconditionally().is_empty() {}
    }

    pub(crate) fn suspend_for_session(&mut self) {
        self.suspended = true;
        self.discard_events_unconditionally();
    }

    pub(crate) fn resume_after_session(&mut self) {
        self.discard_events_unconditionally();
        self.suspended = false;
        self.discard_events_unconditionally();
    }
}

pub(crate) fn input_event_paths(root: &Path) -> Vec<PathBuf> {
    input_event_paths_with_udev(root, Path::new("/run/udev/data"))
}

pub(crate) fn input_event_paths_with_udev(root: &Path, udev_data_root: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(root)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"))
        })
        .filter(|path| input_event_is_keyboard_or_pointer(path, udev_data_root))
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| input_event_number(path).unwrap_or(u32::MAX));
    paths
}

pub(crate) fn input_event_is_keyboard_or_pointer(path: &Path, udev_data_root: &Path) -> bool {
    let Some(event_number) = input_event_number(path) else {
        return false;
    };
    let Some(minor) = event_number.checked_add(64) else {
        return false;
    };
    let Ok(udev_data) = fs::read_to_string(udev_data_root.join(format!("c13:{minor}"))) else {
        return false;
    };
    udev_data.lines().any(|line| {
        matches!(
            line,
            "E:ID_INPUT_KEYBOARD=1" | "E:ID_INPUT_MOUSE=1" | "E:ID_INPUT_TOUCHPAD=1"
        )
    })
}

pub(crate) fn input_event_number(path: &Path) -> Option<u32> {
    path.file_name()?
        .to_str()?
        .strip_prefix("event")?
        .parse()
        .ok()
}

pub(crate) fn read_linux_input_event(device: &mut NativeInputDevice) -> Option<LinuxInputEvent> {
    let mut event = mem::MaybeUninit::<LinuxInputEvent>::uninit();
    let read = unsafe {
        libc::read(
            device.file.as_raw_fd(),
            event.as_mut_ptr().cast::<c_void>(),
            mem::size_of::<LinuxInputEvent>(),
        )
    };
    if read == mem::size_of::<LinuxInputEvent>() as isize {
        return Some(unsafe { event.assume_init() });
    }
    if read < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::WouldBlock {
            eprintln!(
                "native input: failed reading {}: {error}",
                device.path.display()
            );
        }
    }
    None
}

pub(crate) fn apply_native_input_effect(
    mut effect: NativeInputEffect,
    context: NativeInputApplyContext<'_>,
) -> NativeResult<NativeInputApplication> {
    let mut application = NativeInputApplication {
        redraw_requested: effect.requires_frame_repaint(context.cursor_mode),
        exit_requested: effect.exit_requested,
        launch: None,
        fallback_attempts: 0,
        fallback_spawn_failed: None,
    };
    application.redraw_requested |= apply_compositor_only_pointer_position(&effect, |x, y| {
        context
            .server
            .update_pointer_position_without_client_dispatch(x, y)
    });
    for event in effect.keyboard_events {
        context.server.send_keyboard_key(event.key, event.pressed);
    }
    if let Some((x, y)) = effect.pointer_motion
        && context.server.window_interaction_active()
    {
        let action = NativeWindowAction::UpdateInteraction { x, y };
        let changed =
            apply_native_window_action(action, context.server, context.perf, context.resize_perf);
        application.redraw_requested |= changed;
    } else if effect.pointer_motion.is_some() || effect.relative_motion.is_some() {
        context
            .server
            .send_pointer_motion_sample(CompositorPointerMotionSample {
                timestamp_usec: effect.pointer_motion_usec.unwrap_or(0),
                absolute: effect
                    .pointer_motion
                    .map(|(x, y)| CompositorOutputPosition { x, y }),
                relative: effect.relative_motion.map(Into::into),
            });
    }
    for event in effect.pointer_buttons {
        if !event.pressed
            && context
                .server
                .end_window_interaction_for_button(event.button)
        {
            context.resize_perf.observe_action(
                NativeWindowAction::EndInteraction,
                true,
                context.perf,
            );
            application.redraw_requested = true;
            continue;
        }
        context
            .server
            .send_pointer_button(event.button, event.pressed);
    }
    if let Some((horizontal, vertical)) = effect.pointer_axis {
        context.server.send_pointer_axis(horizontal, vertical);
    }
    for action in effect.window_actions {
        application.redraw_requested |=
            apply_native_window_action(action, context.server, context.perf, context.resize_perf);
    }
    let mut fallback_attempt = None;
    for shortcut in effect.shortcut_events {
        let dispatched = context.server.emit_astrea_shortcut(
            &shortcut.namespace,
            &shortcut.name,
            shortcut.phase,
            effect
                .pointer_motion_usec
                .and_then(|timestamp| u32::try_from(timestamp / 1_000).ok())
                .unwrap_or(0),
        );
        context.perf.log("shortcut_emit", || {
            vec![
                NativePerfField::str("namespace", shortcut.namespace.clone()),
                NativePerfField::str("name", shortcut.name.clone()),
                NativePerfField::str("phase", shortcut.phase.as_str()),
            ]
        });
        context.perf.log("shortcut_client_dispatch", || {
            vec![
                NativePerfField::str("namespace", shortcut.namespace.clone()),
                NativePerfField::str("name", shortcut.name.clone()),
                NativePerfField::usize("clients", dispatched),
            ]
        });
        if dispatched > 0 {
            context.perf.log("shortcut.protocol_dispatched", || {
                vec![
                    NativePerfField::str("namespace", shortcut.namespace.clone()),
                    NativePerfField::str("name", shortcut.name.clone()),
                    NativePerfField::str("phase", shortcut.phase.as_str()),
                    NativePerfField::usize("protocol_clients", dispatched),
                ]
            });
            continue;
        }

        let Some(kind) = astrea_shortcut_fallback_kind(&shortcut, dispatched) else {
            context.perf.log("shortcut.fallback_unavailable", || {
                vec![
                    NativePerfField::str("namespace", shortcut.namespace.clone()),
                    NativePerfField::str("name", shortcut.name.clone()),
                    NativePerfField::str("phase", shortcut.phase.as_str()),
                    NativePerfField::usize("protocol_clients", dispatched),
                    NativePerfField::str("fallback_kind", "none"),
                    NativePerfField::bool("fallback_available", false),
                    NativePerfField::u64("fallback_pid", 0),
                ]
            });
            continue;
        };
        let Some(command) = kind.command() else {
            context.perf.log("shortcut.fallback_unavailable", || {
                vec![
                    NativePerfField::str("namespace", shortcut.namespace.clone()),
                    NativePerfField::str("name", shortcut.name.clone()),
                    NativePerfField::str("phase", shortcut.phase.as_str()),
                    NativePerfField::usize("protocol_clients", dispatched),
                    NativePerfField::str("fallback_kind", kind.as_str()),
                    NativePerfField::bool("fallback_available", false),
                    NativePerfField::u64("fallback_pid", 0),
                ]
            });
            continue;
        };
        if effect.launch_command.is_none() {
            effect.launch_command = Some(command);
            effect.launch_source = Some(kind.source());
            application.fallback_attempts += 1;
            fallback_attempt = Some((shortcut.clone(), kind));
        }
    }
    if let Some(vt) = effect.vt_switch
        && let Some(session) = context.seat_session
    {
        session.switch_session(i32::from(vt))?;
        context.perf.log("vt.switch", || {
            vec![NativePerfField::u64("vt", u64::from(vt))]
        });
    }
    if let Some(command) = effect.launch_command {
        let source = effect
            .launch_source
            .unwrap_or(NativeLaunchSource::Spotlight);
        let launch_result = launch_native_shell_command(
            context.server,
            context.process_supervisor,
            command,
            context.app_gpu_policy,
            source,
        );
        match launch_result {
            Ok(launch) => {
                if let Some((shortcut, kind)) = &fallback_attempt
                    && let Some(launch) = &launch
                {
                    context.perf.log("shortcut.fallback_launched", || {
                        vec![
                            NativePerfField::str("namespace", shortcut.namespace.clone()),
                            NativePerfField::str("name", shortcut.name.clone()),
                            NativePerfField::str("phase", shortcut.phase.as_str()),
                            NativePerfField::usize("protocol_clients", 0),
                            NativePerfField::str("fallback_kind", kind.as_str()),
                            NativePerfField::bool("fallback_available", true),
                            NativePerfField::u64("fallback_pid", u64::from(launch.pid)),
                        ]
                    });
                }
                application.launch = launch;
            }
            Err(error) => {
                if let Some((shortcut, kind)) = &fallback_attempt {
                    eprintln!(
                        "native input: fallback_spawn_failed namespace={} name={} kind={}: {error}",
                        shortcut.namespace,
                        shortcut.name,
                        kind.as_str(),
                    );
                    context.perf.log("shortcut.fallback_spawn_failed", || {
                        vec![
                            NativePerfField::str("namespace", shortcut.namespace.clone()),
                            NativePerfField::str("name", shortcut.name.clone()),
                            NativePerfField::str("phase", shortcut.phase.as_str()),
                            NativePerfField::usize("protocol_clients", 0),
                            NativePerfField::str("fallback_kind", kind.as_str()),
                            NativePerfField::bool("fallback_available", true),
                            NativePerfField::u64("fallback_pid", 0),
                        ]
                    });
                    application.fallback_spawn_failed = Some(*kind);
                } else {
                    return Err(error);
                }
            }
        }
    }
    Ok(application)
}

pub(crate) struct NativeInputApplyContext<'a> {
    pub(crate) server: &'a mut OwnCompositorServer,
    pub(crate) perf: NativePerfLogger,
    pub(crate) resize_perf: &'a mut NativeResizePerfState,
    pub(crate) cursor_mode: NativeCursorRenderMode,
    pub(crate) app_gpu_policy: EffectiveCompositorAppGpuPolicy,
    pub(crate) seat_session: Option<&'a NativeSeatSession>,
    pub(crate) process_supervisor: &'a mut ChildSupervisor,
}

pub(crate) fn apply_compositor_only_pointer_position(
    effect: &NativeInputEffect,
    update: impl FnOnce(f64, f64) -> bool,
) -> bool {
    if effect.pointer_motion.is_some() {
        return false;
    }
    let Some((x, y)) = effect.cursor_position else {
        return false;
    };
    update(f64::from(x), f64::from(y))
}

pub(crate) fn process_native_pointer_constraint_backend_requests(
    server: &mut OwnCompositorServer,
    backend: &mut NativePointerConstraintBackend,
    input_state: &mut NativeInputState,
    hardware_cursor: &mut Option<NativeHardwareCursor>,
    cursor_mode: NativeCursorRenderMode,
) -> NativeResult<bool> {
    let mut redraw_requested = false;
    loop {
        let requests = server.take_pointer_constraint_backend_requests();
        if requests.is_empty() {
            break;
        }
        for request in requests {
            let cursor_position = input_state.cursor_position_f64();
            native_pointer_debug_log(format!(
                "pointer.constraint native_request {:?} cursor=({},{})",
                request, cursor_position.x, cursor_position.y
            ));
            if let Some(id) = pointer_constraint_activation_request_id(&request)
                && !server.pointer_constraint_backend_activation_current(id)
            {
                native_pointer_debug_log(format!(
                    "pointer.constraint native_request dropped stale id={} generation={} rollback=not_needed",
                    id.constraint_id, id.generation
                ));
                continue;
            }
            let action = backend.handle_request(request, cursor_position);
            if let Some((id, reason)) = action.failed {
                native_pointer_debug_log(format!(
                    "pointer.constraint native_failed id={} generation={} reason={}",
                    id.constraint_id, id.generation, reason
                ));
                server.pointer_constraint_backend_failed(id, reason);
            }
            if let Some(constraint) = action.activated {
                native_pointer_debug_log(format!(
                    "pointer.constraint native_activated id={} generation={} mode={:?} anchor=({},{})",
                    constraint.id.constraint_id,
                    constraint.id.generation,
                    constraint.mode,
                    constraint.anchor.x,
                    constraint.anchor.y
                ));
                match constraint.mode {
                    PointerConstraintMode::Locked => {
                        input_state.set_pointer_locked_at(constraint.anchor)
                    }
                    PointerConstraintMode::Confined => {
                        if let Some(region) = constraint.region {
                            input_state.set_pointer_confined(region);
                        }
                    }
                    PointerConstraintMode::None => input_state.clear_pointer_constraint(),
                }
                server.pointer_constraint_backend_activated(constraint.id);
            }
            if let Some(restore_position) = action.restore_position {
                native_pointer_debug_log(format!(
                    "pointer.unlock native_restore output=({},{})",
                    restore_position.x, restore_position.y
                ));
                input_state.clear_pointer_constraint();
                let effect = input_state.restore_cursor_position(restore_position);
                redraw_requested |= effect.requires_frame_repaint(cursor_mode);
                if let Some((cursor_x, cursor_y)) = effect.cursor_position
                    && let Some(cursor) = hardware_cursor.as_mut()
                {
                    cursor.move_to(cursor_x, cursor_y)?;
                }
            }
            if let Some(cursor_position) = action.cursor_position {
                let effect = input_state.restore_cursor_position(cursor_position);
                redraw_requested |= effect.requires_frame_repaint(cursor_mode);
                if let Some((cursor_x, cursor_y)) = effect.cursor_position
                    && let Some(cursor) = hardware_cursor.as_mut()
                {
                    cursor.move_to(cursor_x, cursor_y)?;
                }
            }
            if let Some(id) = action.deactivated {
                native_pointer_debug_log(format!(
                    "pointer.constraint native_deactivated id={} generation={}",
                    id.constraint_id, id.generation
                ));
                server.pointer_constraint_backend_deactivated(id);
            }
            if let Some(visible) = action.cursor_visibility_changed {
                native_pointer_debug_log(format!("cursor visibility native visible={}", visible));
                let changed = input_state.set_cursor_visible(visible);
                if cursor_mode == NativeCursorRenderMode::Software && changed {
                    redraw_requested = true;
                }
                if let Some(cursor) = hardware_cursor.as_mut() {
                    if visible {
                        let (cursor_x, cursor_y) = input_state.cursor_position();
                        cursor
                            .enable()
                            .and_then(|()| cursor.move_to(cursor_x, cursor_y))?;
                    } else {
                        cursor.disable()?;
                    }
                }
            }
        }
    }
    input_state.pointer_constraint = backend.active_constraint_state();
    Ok(redraw_requested)
}

pub(crate) fn pointer_constraint_activation_request_id(
    request: &PointerConstraintBackendRequest,
) -> Option<PointerConstraintBackendId> {
    match request {
        PointerConstraintBackendRequest::ActivateLocked { id, .. }
        | PointerConstraintBackendRequest::ActivateConfined { id, .. } => Some(*id),
        _ => None,
    }
}

#[derive(Debug)]
pub(crate) struct NativeInputApplication {
    pub(crate) redraw_requested: bool,
    pub(crate) exit_requested: bool,
    pub(crate) launch: Option<NativeAppLaunchPerf>,
    pub(crate) fallback_attempts: usize,
    pub(crate) fallback_spawn_failed: Option<AstreaShortcutFallbackKind>,
}

pub(crate) fn apply_native_window_action(
    action: NativeWindowAction,
    server: &mut OwnCompositorServer,
    perf: NativePerfLogger,
    resize_perf: &mut NativeResizePerfState,
) -> bool {
    let changed = match action {
        NativeWindowAction::BeginMove {
            x,
            y,
            trigger_button,
        } => {
            if let Some(button) = trigger_button {
                server.begin_window_move_at_with_trigger(x, y, button)
            } else {
                server.begin_window_move_at(x, y)
            }
        }
        NativeWindowAction::BeginResize {
            x,
            y,
            trigger_button,
        } => {
            if let Some(button) = trigger_button {
                server.begin_window_resize_at_with_trigger(x, y, button)
            } else {
                server.begin_window_resize_at(x, y)
            }
        }
        NativeWindowAction::UpdateInteraction { x, y } => server.update_window_interaction(x, y),
        NativeWindowAction::EndInteraction => {
            let was_active = server.window_interaction_active();
            server.end_window_interaction();
            was_active
        }
        NativeWindowAction::CloseActiveWindow => false,
        NativeWindowAction::ToggleFullscreen => server.toggle_fullscreen_focused_window(),
    };
    resize_perf.observe_action(action, changed, perf);
    changed
}
