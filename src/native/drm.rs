//! Legacy DRM pageflip submission and completion metadata.

use std::{
    ffi::c_void,
    fmt, io, mem,
    os::fd::{AsRawFd, BorrowedFd, RawFd},
    ptr,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmPresentationTimestamp {
    pub seconds: u32,
    pub microseconds: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrmPresentationEvent {
    pub user_data: u64,
    pub sequence: u32,
    pub timestamp: DrmPresentationTimestamp,
    pub crtc_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrmTimestampClock {
    Monotonic,
    Realtime,
}

impl DrmTimestampClock {
    pub fn from_capability_value(value: u64) -> io::Result<Self> {
        match value {
            0 => Ok(Self::Realtime),
            1 => Ok(Self::Monotonic),
            value => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported DRM timestamp clock capability value {value}"),
            )),
        }
    }

    pub const fn clock_id(self) -> libc::clockid_t {
        match self {
            Self::Monotonic => libc::CLOCK_MONOTONIC,
            Self::Realtime => libc::CLOCK_REALTIME,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Monotonic => "monotonic",
            Self::Realtime => "realtime",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrmEventParseError {
    TruncatedHeader {
        remaining: usize,
    },
    InvalidLength {
        offset: usize,
        length: usize,
    },
    TruncatedEvent {
        offset: usize,
        length: usize,
        available: usize,
    },
    InvalidPageFlipLength {
        offset: usize,
        length: usize,
    },
}

impl fmt::Display for DrmEventParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid DRM event buffer: {self:?}")
    }
}

impl std::error::Error for DrmEventParseError {}

pub fn parse_drm_page_flip_events(
    bytes: &[u8],
) -> Result<Vec<DrmPresentationEvent>, DrmEventParseError> {
    let header_size = mem::size_of::<drm_sys::drm_event>();
    let page_flip_size = mem::size_of::<drm_sys::drm_event_vblank>();
    let mut events = Vec::new();
    let mut offset = 0usize;

    while offset < bytes.len() {
        let remaining = bytes.len() - offset;
        if remaining < header_size {
            return Err(DrmEventParseError::TruncatedHeader { remaining });
        }
        // SAFETY: The header bounds were checked above. DRM event buffers are
        // byte-aligned, so an unaligned read is required.
        let header =
            unsafe { ptr::read_unaligned(bytes.as_ptr().add(offset).cast::<drm_sys::drm_event>()) };
        let length = header.length as usize;
        if length < header_size {
            return Err(DrmEventParseError::InvalidLength { offset, length });
        }
        if length > remaining {
            return Err(DrmEventParseError::TruncatedEvent {
                offset,
                length,
                available: remaining,
            });
        }
        if header.type_ == drm_sys::DRM_EVENT_FLIP_COMPLETE {
            if length < page_flip_size {
                return Err(DrmEventParseError::InvalidPageFlipLength { offset, length });
            }
            // SAFETY: The event's declared and available lengths were checked
            // against the complete repr(C) DRM vblank event structure.
            let event = unsafe {
                ptr::read_unaligned(
                    bytes
                        .as_ptr()
                        .add(offset)
                        .cast::<drm_sys::drm_event_vblank>(),
                )
            };
            events.push(DrmPresentationEvent {
                user_data: event.user_data,
                sequence: event.sequence,
                timestamp: DrmPresentationTimestamp {
                    seconds: event.tv_sec,
                    microseconds: event.tv_usec,
                },
                crtc_id: event.crtc_id,
            });
        }
        offset += length;
    }

    Ok(events)
}

pub fn query_drm_timestamp_clock(fd: BorrowedFd<'_>) -> io::Result<DrmTimestampClock> {
    let capability = drm_ffi::get_capability(fd, u64::from(drm_sys::DRM_CAP_TIMESTAMP_MONOTONIC))?;
    DrmTimestampClock::from_capability_value(capability.value)
}

pub fn prime_fd_to_handle(drm: BorrowedFd<'_>, dma_buf: BorrowedFd<'_>) -> io::Result<u32> {
    let imported = drm_ffi::gem::fd_to_handle(drm, dma_buf)?;
    if imported.handle == 0 {
        return Err(io::Error::other(
            "DRM PRIME import returned GEM handle zero",
        ));
    }
    Ok(imported.handle)
}

pub fn gem_close(drm: BorrowedFd<'_>, handle: u32) -> io::Result<()> {
    if handle == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot close GEM handle zero",
        ));
    }
    drm_ffi::gem::close(drm, handle).map(|_| ())
}

pub fn sample_clock_microseconds(clock: DrmTimestampClock) -> io::Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(clock.clock_id(), &mut time) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let seconds = u64::try_from(time.tv_sec).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "DRM presentation clock returned negative seconds: {}",
                time.tv_sec
            ),
        )
    })?;
    let nanoseconds = u64::try_from(time.tv_nsec)
        .ok()
        .filter(|nanoseconds| *nanoseconds < 1_000_000_000)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "DRM presentation clock returned invalid nanoseconds: {}",
                    time.tv_nsec
                ),
            )
        })?;
    Ok(seconds
        .saturating_mul(1_000_000)
        .saturating_add(nanoseconds / 1_000))
}

pub fn submit_legacy_page_flip(
    fd: BorrowedFd<'_>,
    crtc_id: u32,
    fb_id: u32,
    token: u64,
) -> io::Result<()> {
    let mut flip = drm_sys::drm_mode_crtc_page_flip {
        crtc_id,
        fb_id,
        flags: drm_sys::DRM_MODE_PAGE_FLIP_EVENT,
        reserved: 0,
        user_data: token,
    };
    let result = unsafe {
        libc::ioctl(
            fd.as_raw_fd(),
            drm_iowr::<drm_sys::drm_mode_crtc_page_flip>(0xB0),
            &mut flip,
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn drain_drm_page_flip_events(fd: RawFd) -> io::Result<Vec<DrmPresentationEvent>> {
    let mut events = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = unsafe { libc::read(fd, buffer.as_mut_ptr().cast::<c_void>(), buffer.len()) };
        if read < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock {
                return Ok(events);
            }
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if read == 0 {
            return Ok(events);
        }
        // DRM guarantees that one read returns only complete events. Looping
        // reads handles more queued data than fits in this fixed buffer.
        let parsed = parse_drm_page_flip_events(&buffer[..read as usize])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        events.extend(parsed);
    }
}

fn drm_iowr<T>(number: u8) -> libc::c_ulong {
    const IOC_NRBITS: libc::c_ulong = 8;
    const IOC_TYPEBITS: libc::c_ulong = 8;
    const IOC_SIZEBITS: libc::c_ulong = 14;
    const IOC_WRITE: libc::c_ulong = 1;
    const IOC_READ: libc::c_ulong = 2;
    const IOC_NRSHIFT: libc::c_ulong = 0;
    const IOC_TYPESHIFT: libc::c_ulong = IOC_NRSHIFT + IOC_NRBITS;
    const IOC_SIZESHIFT: libc::c_ulong = IOC_TYPESHIFT + IOC_TYPEBITS;
    const IOC_DIRSHIFT: libc::c_ulong = IOC_SIZESHIFT + IOC_SIZEBITS;

    ((IOC_READ | IOC_WRITE) << IOC_DIRSHIFT)
        | ((mem::size_of::<T>() as libc::c_ulong) << IOC_SIZESHIFT)
        | ((b'd' as libc::c_ulong) << IOC_TYPESHIFT)
        | ((number as libc::c_ulong) << IOC_NRSHIFT)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER_SIZE: usize = 8;
    const PAGE_FLIP_SIZE: usize = 32;

    fn page_flip_event(
        token: u64,
        seconds: u32,
        microseconds: u32,
        sequence: u32,
        crtc_id: u32,
    ) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(PAGE_FLIP_SIZE);
        bytes.extend_from_slice(&drm_sys::DRM_EVENT_FLIP_COMPLETE.to_ne_bytes());
        bytes.extend_from_slice(&(PAGE_FLIP_SIZE as u32).to_ne_bytes());
        bytes.extend_from_slice(&token.to_ne_bytes());
        bytes.extend_from_slice(&seconds.to_ne_bytes());
        bytes.extend_from_slice(&microseconds.to_ne_bytes());
        bytes.extend_from_slice(&sequence.to_ne_bytes());
        bytes.extend_from_slice(&crtc_id.to_ne_bytes());
        bytes
    }

    fn unknown_event(event_type: u32, payload: &[u8]) -> Vec<u8> {
        let length = HEADER_SIZE + payload.len();
        let mut bytes = Vec::with_capacity(length);
        bytes.extend_from_slice(&event_type.to_ne_bytes());
        bytes.extend_from_slice(&(length as u32).to_ne_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn parses_valid_page_flip_event() {
        let events = parse_drm_page_flip_events(&page_flip_event(7, 12, 345, 99, 4)).unwrap();

        assert_eq!(
            events,
            [DrmPresentationEvent {
                user_data: 7,
                sequence: 99,
                timestamp: DrmPresentationTimestamp {
                    seconds: 12,
                    microseconds: 345,
                },
                crtc_id: 4,
            }]
        );
    }

    #[test]
    fn parses_two_page_flip_events_from_one_buffer() {
        let mut bytes = page_flip_event(1, 2, 3, 4, 5);
        bytes.extend(page_flip_event(6, 7, 8, 9, 10));

        let events = parse_drm_page_flip_events(&bytes).unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].user_data, 1);
        assert_eq!(events[1].user_data, 6);
    }

    #[test]
    fn skips_valid_event_followed_by_unknown_event() {
        let mut bytes = page_flip_event(1, 2, 3, 4, 5);
        bytes.extend(unknown_event(0x8000_0001, &[1, 2, 3, 4]));

        let events = parse_drm_page_flip_events(&bytes).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].user_data, 1);
    }

    #[test]
    fn skips_unknown_event_before_valid_page_flip() {
        let mut bytes = unknown_event(0x8000_0001, &[1, 2, 3, 4]);
        bytes.extend(page_flip_event(9, 8, 7, 6, 5));

        let events = parse_drm_page_flip_events(&bytes).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].user_data, 9);
    }

    #[test]
    fn rejects_truncated_header() {
        assert_eq!(
            parse_drm_page_flip_events(&[0; HEADER_SIZE - 1]),
            Err(DrmEventParseError::TruncatedHeader {
                remaining: HEADER_SIZE - 1
            })
        );
    }

    #[test]
    fn rejects_declared_length_smaller_than_header() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&drm_sys::DRM_EVENT_FLIP_COMPLETE.to_ne_bytes());
        bytes.extend_from_slice(&4u32.to_ne_bytes());

        assert_eq!(
            parse_drm_page_flip_events(&bytes),
            Err(DrmEventParseError::InvalidLength {
                offset: 0,
                length: 4
            })
        );
    }

    #[test]
    fn rejects_declared_length_larger_than_available_bytes() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&drm_sys::DRM_EVENT_FLIP_COMPLETE.to_ne_bytes());
        bytes.extend_from_slice(&64u32.to_ne_bytes());
        bytes.extend_from_slice(&[0; 8]);

        assert_eq!(
            parse_drm_page_flip_events(&bytes),
            Err(DrmEventParseError::TruncatedEvent {
                offset: 0,
                length: 64,
                available: 16,
            })
        );
    }

    #[test]
    fn rejects_zero_event_length() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&drm_sys::DRM_EVENT_FLIP_COMPLETE.to_ne_bytes());
        bytes.extend_from_slice(&0u32.to_ne_bytes());

        assert_eq!(
            parse_drm_page_flip_events(&bytes),
            Err(DrmEventParseError::InvalidLength {
                offset: 0,
                length: 0
            })
        );
    }

    #[test]
    fn rejects_page_flip_body_shorter_than_uapi_structure() {
        let mut bytes = unknown_event(drm_sys::DRM_EVENT_FLIP_COMPLETE, &[0; 8]);
        bytes[4..8].copy_from_slice(&16u32.to_ne_bytes());

        assert_eq!(
            parse_drm_page_flip_events(&bytes),
            Err(DrmEventParseError::InvalidPageFlipLength {
                offset: 0,
                length: 16
            })
        );
    }

    #[test]
    fn preserves_maximum_sequence() {
        let events = parse_drm_page_flip_events(&page_flip_event(1, 2, 3, u32::MAX, 4)).unwrap();

        assert_eq!(events[0].sequence, u32::MAX);
    }

    #[test]
    fn preserves_zero_microseconds() {
        let events = parse_drm_page_flip_events(&page_flip_event(1, 2, 0, 3, 4)).unwrap();

        assert_eq!(events[0].timestamp.microseconds, 0);
    }

    #[test]
    fn preserves_last_valid_microsecond() {
        let events = parse_drm_page_flip_events(&page_flip_event(1, 2, 999_999, 3, 4)).unwrap();

        assert_eq!(events[0].timestamp.microseconds, 999_999);
    }

    #[test]
    fn preserves_full_user_data_token() {
        let token = 0xfedc_ba98_7654_3210;
        let events = parse_drm_page_flip_events(&page_flip_event(token, 2, 3, 4, 5)).unwrap();

        assert_eq!(events[0].user_data, token);
    }

    #[test]
    fn timestamp_clock_capability_maps_only_known_values() {
        assert_eq!(
            DrmTimestampClock::from_capability_value(0).unwrap(),
            DrmTimestampClock::Realtime
        );
        assert_eq!(
            DrmTimestampClock::from_capability_value(1).unwrap(),
            DrmTimestampClock::Monotonic
        );
        assert!(DrmTimestampClock::from_capability_value(2).is_err());
    }
}
