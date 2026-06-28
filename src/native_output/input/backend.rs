use super::*;

pub(crate) fn native_window_drag_action(
    alt_pressed: bool,
    button: u32,
    x: f64,
    y: f64,
) -> Option<NativeWindowAction> {
    if !alt_pressed {
        return None;
    }

    match u16::try_from(button).ok()? {
        BTN_LEFT => Some(NativeWindowAction::BeginMove { x, y }),
        BTN_RIGHT => Some(NativeWindowAction::BeginResize { x, y }),
        _ => None,
    }
}

pub(crate) fn evdev_key_to_text(code: u16, shifted: bool) -> Option<&'static str> {
    match (code, shifted) {
        (KEY_A, false) => Some("a"),
        (KEY_A, true) => Some("A"),
        (KEY_B, false) => Some("b"),
        (KEY_B, true) => Some("B"),
        (KEY_C, false) => Some("c"),
        (KEY_C, true) => Some("C"),
        (KEY_D, false) => Some("d"),
        (KEY_D, true) => Some("D"),
        (KEY_E, false) => Some("e"),
        (KEY_E, true) => Some("E"),
        (KEY_F, false) => Some("f"),
        (KEY_F, true) => Some("F"),
        (KEY_G, false) => Some("g"),
        (KEY_G, true) => Some("G"),
        (KEY_H, false) => Some("h"),
        (KEY_H, true) => Some("H"),
        (KEY_I, false) => Some("i"),
        (KEY_I, true) => Some("I"),
        (KEY_J, false) => Some("j"),
        (KEY_J, true) => Some("J"),
        (KEY_K, false) => Some("k"),
        (KEY_K, true) => Some("K"),
        (KEY_L, false) => Some("l"),
        (KEY_L, true) => Some("L"),
        (KEY_M, false) => Some("m"),
        (KEY_M, true) => Some("M"),
        (KEY_N, false) => Some("n"),
        (KEY_N, true) => Some("N"),
        (KEY_O, false) => Some("o"),
        (KEY_O, true) => Some("O"),
        (KEY_P, false) => Some("p"),
        (KEY_P, true) => Some("P"),
        (KEY_Q, false) => Some("q"),
        (KEY_Q, true) => Some("Q"),
        (KEY_R, false) => Some("r"),
        (KEY_R, true) => Some("R"),
        (KEY_S, false) => Some("s"),
        (KEY_S, true) => Some("S"),
        (KEY_T, false) => Some("t"),
        (KEY_T, true) => Some("T"),
        (KEY_U, false) => Some("u"),
        (KEY_U, true) => Some("U"),
        (KEY_V, false) => Some("v"),
        (KEY_V, true) => Some("V"),
        (KEY_W, false) => Some("w"),
        (KEY_W, true) => Some("W"),
        (KEY_X, false) => Some("x"),
        (KEY_X, true) => Some("X"),
        (KEY_Y, false) => Some("y"),
        (KEY_Y, true) => Some("Y"),
        (KEY_Z, false) => Some("z"),
        (KEY_Z, true) => Some("Z"),
        (KEY_1, false) => Some("1"),
        (KEY_1, true) => Some("!"),
        (KEY_2, false) => Some("2"),
        (KEY_2, true) => Some("@"),
        (KEY_3, false) => Some("3"),
        (KEY_3, true) => Some("#"),
        (KEY_4, false) => Some("4"),
        (KEY_4, true) => Some("$"),
        (KEY_5, false) => Some("5"),
        (KEY_5, true) => Some("%"),
        (KEY_6, false) => Some("6"),
        (KEY_6, true) => Some("^"),
        (KEY_7, false) => Some("7"),
        (KEY_7, true) => Some("&"),
        (KEY_8, false) => Some("8"),
        (KEY_8, true) => Some("*"),
        (KEY_9, false) => Some("9"),
        (KEY_9, true) => Some("("),
        (KEY_0, false) => Some("0"),
        (KEY_0, true) => Some(")"),
        (KEY_SPACE, _) => Some(" "),
        (KEY_MINUS, false) => Some("-"),
        (KEY_MINUS, true) => Some("_"),
        (KEY_EQUAL, false) => Some("="),
        (KEY_EQUAL, true) => Some("+"),
        (KEY_COMMA, false) => Some(","),
        (KEY_COMMA, true) => Some("<"),
        (KEY_DOT, false) => Some("."),
        (KEY_DOT, true) => Some(">"),
        (KEY_SLASH, false) => Some("/"),
        (KEY_SLASH, true) => Some("?"),
        _ => None,
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct LinuxInputEvent {
    pub(crate) _time: libc::timeval,
    pub(crate) type_: u16,
    pub(crate) code: u16,
    pub(crate) value: i32,
}

pub(crate) fn linux_input_event_time_usec(event: LinuxInputEvent) -> u64 {
    let seconds = u64::try_from(event._time.tv_sec).unwrap_or(0);
    let micros = u64::try_from(event._time.tv_usec).unwrap_or(0);
    seconds.saturating_mul(1_000_000).saturating_add(micros)
}

pub(crate) fn open_native_seat_session(
    session_probe: &NativeSessionProbe,
) -> Option<NativeSeatSession> {
    let dependencies = session_probe.plan.dependencies;
    if !(dependencies.seat_manager_available && dependencies.libseat_available) {
        return None;
    }
    match NativeSeatSession::open() {
        Ok(session) => {
            println!(
                "native seat: acquired {}",
                session.seat_name().unwrap_or_else(|| "unknown".to_string())
            );
            Some(session)
        }
        Err(error) => {
            eprintln!("native seat: libseat activation failed; using direct fallbacks: {error}");
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeDrmBackendKind {
    Libseat,
    Direct,
    Unavailable,
}

impl NativeDrmBackendKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Libseat => "libseat DRM",
            Self::Direct => "direct DRM",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeDrmBackendPreference {
    Auto,
    Libseat,
    Direct,
}

impl NativeDrmBackendPreference {
    pub(crate) fn from_env() -> Self {
        match std::env::var("OBLIVION_ONE_DRM_BACKEND") {
            Ok(value) if matches!(value.as_str(), "seat" | "libseat" | "seat-drm") => Self::Libseat,
            Ok(value) if matches!(value.as_str(), "direct" | "kms") => Self::Direct,
            Ok(value) if value == "auto" => Self::Auto,
            Ok(value) => {
                eprintln!("native DRM: unknown OBLIVION_ONE_DRM_BACKEND={value:?}; using auto");
                Self::Auto
            }
            Err(_) => Self::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeDrmBackendChoice {
    pub(crate) preference: NativeDrmBackendPreference,
    pub(crate) seat_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeDrmBackendPlan {
    pub(crate) primary: NativeDrmBackendKind,
    pub(crate) fallbacks: Vec<NativeDrmBackendKind>,
}

impl NativeDrmBackendPlan {
    pub(crate) fn choose(choice: NativeDrmBackendChoice) -> Self {
        match choice.preference {
            NativeDrmBackendPreference::Libseat if choice.seat_available => Self {
                primary: NativeDrmBackendKind::Libseat,
                fallbacks: Vec::new(),
            },
            NativeDrmBackendPreference::Libseat => Self::unavailable(),
            NativeDrmBackendPreference::Direct => Self {
                primary: NativeDrmBackendKind::Direct,
                fallbacks: Vec::new(),
            },
            NativeDrmBackendPreference::Auto if choice.seat_available => Self {
                primary: NativeDrmBackendKind::Libseat,
                fallbacks: vec![NativeDrmBackendKind::Direct],
            },
            NativeDrmBackendPreference::Auto => Self {
                primary: NativeDrmBackendKind::Direct,
                fallbacks: Vec::new(),
            },
        }
    }

    pub(crate) fn unavailable() -> Self {
        Self {
            primary: NativeDrmBackendKind::Unavailable,
            fallbacks: Vec::new(),
        }
    }

    pub(crate) fn candidates(&self) -> impl Iterator<Item = NativeDrmBackendKind> + '_ {
        std::iter::once(self.primary).chain(self.fallbacks.iter().copied())
    }
}

pub(crate) enum NativeDrmDeviceStorage {
    SeatManaged(NativeSeatDeviceFile),
    Direct(fs::File),
}

pub(crate) struct NativeDrmDevice {
    pub(crate) kind: NativeDrmBackendKind,
    pub(crate) storage: NativeDrmDeviceStorage,
}

impl NativeDrmDevice {
    pub(crate) fn open(
        plan: NativeDrmBackendPlan,
        path: &Path,
        seat_session: Option<NativeSeatSession>,
    ) -> io::Result<Self> {
        let mut last_error = None;
        for candidate in plan.candidates() {
            match Self::open_kind(candidate, path, seat_session.as_ref()) {
                Ok(device) => return Ok(device),
                Err(error) => {
                    eprintln!("native DRM: {} backend failed: {error}", candidate.as_str());
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| io::Error::other("native DRM backend is unavailable")))
    }

    pub(crate) fn open_kind(
        kind: NativeDrmBackendKind,
        path: &Path,
        seat_session: Option<&NativeSeatSession>,
    ) -> io::Result<Self> {
        match kind {
            NativeDrmBackendKind::Libseat => {
                let session = seat_session.ok_or_else(|| {
                    io::Error::other("libseat DRM requested but no active seat session exists")
                })?;
                let file = session.open_device_file(path)?;
                println!("native DRM: opened {} through libseat", path.display());
                Ok(Self {
                    kind,
                    storage: NativeDrmDeviceStorage::SeatManaged(file),
                })
            }
            NativeDrmBackendKind::Direct => {
                let file = OpenOptions::new().read(true).write(true).open(path)?;
                println!("native DRM: opened {} directly", path.display());
                Ok(Self {
                    kind,
                    storage: NativeDrmDeviceStorage::Direct(file),
                })
            }
            NativeDrmBackendKind::Unavailable => {
                Err(io::Error::other("native DRM backend is unavailable"))
            }
        }
    }

    pub(crate) const fn kind(&self) -> NativeDrmBackendKind {
        self.kind
    }

    pub(crate) fn file(&self) -> &fs::File {
        match &self.storage {
            NativeDrmDeviceStorage::SeatManaged(device) => device.file(),
            NativeDrmDeviceStorage::Direct(file) => file,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeInputBackendKind {
    LibseatLibinputUdev,
    DirectLibinputUdev,
    RawEvdev,
    Unavailable,
}

impl NativeInputBackendKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::LibseatLibinputUdev => "libseat + libinput udev",
            Self::DirectLibinputUdev => "direct libinput udev",
            Self::RawEvdev => "raw evdev fallback",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeInputBackendPreference {
    Auto,
    LibseatLibinputUdev,
    DirectLibinputUdev,
    RawEvdev,
}

impl NativeInputBackendPreference {
    pub(crate) fn from_env() -> Self {
        match std::env::var("OBLIVION_ONE_INPUT_BACKEND") {
            Ok(value) if matches!(value.as_str(), "seat" | "libseat" | "seat-libinput") => {
                Self::LibseatLibinputUdev
            }
            Ok(value)
                if matches!(
                    value.as_str(),
                    "libinput" | "udev" | "direct-libinput" | "libinput-direct"
                ) =>
            {
                Self::DirectLibinputUdev
            }
            Ok(value) if matches!(value.as_str(), "raw" | "evdev") => Self::RawEvdev,
            Ok(value) if value == "auto" => Self::Auto,
            Ok(value) => {
                eprintln!("native input: unknown OBLIVION_ONE_INPUT_BACKEND={value:?}; using auto");
                Self::Auto
            }
            Err(_) => Self::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeInputBackendChoice {
    pub(crate) preference: NativeInputBackendPreference,
    pub(crate) libseat_available: bool,
    pub(crate) libinput_available: bool,
    pub(crate) raw_evdev_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeInputBackendPlan {
    pub(crate) primary: NativeInputBackendKind,
    pub(crate) fallbacks: Vec<NativeInputBackendKind>,
}

impl NativeInputBackendPlan {
    pub(crate) fn choose(choice: NativeInputBackendChoice) -> Self {
        match choice.preference {
            NativeInputBackendPreference::LibseatLibinputUdev
                if choice.libseat_available && choice.libinput_available =>
            {
                Self {
                    primary: NativeInputBackendKind::LibseatLibinputUdev,
                    fallbacks: Vec::new(),
                }
            }
            NativeInputBackendPreference::LibseatLibinputUdev => Self::unavailable(),
            NativeInputBackendPreference::DirectLibinputUdev if choice.libinput_available => Self {
                primary: NativeInputBackendKind::DirectLibinputUdev,
                fallbacks: Vec::new(),
            },
            NativeInputBackendPreference::DirectLibinputUdev => Self::unavailable(),
            NativeInputBackendPreference::RawEvdev if choice.raw_evdev_available => Self {
                primary: NativeInputBackendKind::RawEvdev,
                fallbacks: Vec::new(),
            },
            NativeInputBackendPreference::RawEvdev => Self::unavailable(),
            NativeInputBackendPreference::Auto
                if choice.libseat_available && choice.libinput_available =>
            {
                let mut fallbacks = Vec::new();
                fallbacks.push(NativeInputBackendKind::DirectLibinputUdev);
                if choice.raw_evdev_available {
                    fallbacks.push(NativeInputBackendKind::RawEvdev);
                }
                Self {
                    primary: NativeInputBackendKind::LibseatLibinputUdev,
                    fallbacks,
                }
            }
            NativeInputBackendPreference::Auto if choice.libinput_available => {
                let mut fallbacks = Vec::new();
                if choice.raw_evdev_available {
                    fallbacks.push(NativeInputBackendKind::RawEvdev);
                }
                Self {
                    primary: NativeInputBackendKind::DirectLibinputUdev,
                    fallbacks,
                }
            }
            NativeInputBackendPreference::Auto if choice.raw_evdev_available => Self {
                primary: NativeInputBackendKind::RawEvdev,
                fallbacks: Vec::new(),
            },
            NativeInputBackendPreference::Auto => Self::unavailable(),
        }
    }

    pub(crate) fn unavailable() -> Self {
        Self {
            primary: NativeInputBackendKind::Unavailable,
            fallbacks: Vec::new(),
        }
    }

    pub(crate) fn candidates(&self) -> impl Iterator<Item = NativeInputBackendKind> + '_ {
        std::iter::once(self.primary).chain(self.fallbacks.iter().copied())
    }
}

pub(crate) enum NativeInputBackend {
    LibseatLibinput(LibinputInputBackend),
    DirectLibinput(LibinputInputBackend),
    RawEvdev(NativeInputDevices),
}
