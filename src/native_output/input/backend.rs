use super::*;

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
