#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferSize {
    pub width: u32,
    pub height: u32,
}

impl BufferSize {
    pub const fn new(width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 {
            None
        } else {
            Some(Self { width, height })
        }
    }

    pub fn pixel_count(self) -> Option<usize> {
        let width = usize::try_from(self.width).ok()?;
        let height = usize::try_from(self.height).ok()?;
        width.checked_mul(height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrmFormat {
    Argb8888,
    Xrgb8888,
    Other(u32),
}

impl DrmFormat {
    pub const ARGB8888_FOURCC: u32 = fourcc(b'A', b'R', b'2', b'4');
    pub const XRGB8888_FOURCC: u32 = fourcc(b'X', b'R', b'2', b'4');

    pub const fn from_fourcc(format: u32) -> Self {
        match format {
            Self::ARGB8888_FOURCC => Self::Argb8888,
            Self::XRGB8888_FOURCC => Self::Xrgb8888,
            other => Self::Other(other),
        }
    }

    pub const fn as_fourcc(self) -> u32 {
        match self {
            Self::Argb8888 => Self::ARGB8888_FOURCC,
            Self::Xrgb8888 => Self::XRGB8888_FOURCC,
            Self::Other(format) => format,
        }
    }
}

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    a as u32 | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DrmModifier(pub u64);

impl DrmModifier {
    pub const LINEAR: Self = Self(0);
    pub const INVALID: Self = Self(0x00ff_ffff_ffff_ffff);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmabufPlaneDescriptor {
    pub plane_index: u32,
    pub offset: u32,
    pub stride: u32,
    pub modifier: DrmModifier,
}

#[derive(Debug, Clone)]
pub struct DmabufPlane {
    fd: Arc<OwnedFd>,
    descriptor: DmabufPlaneDescriptor,
}

impl DmabufPlane {
    pub fn new(fd: OwnedFd, descriptor: DmabufPlaneDescriptor) -> Self {
        Self {
            fd: Arc::new(fd),
            descriptor,
        }
    }

    pub const fn descriptor(&self) -> DmabufPlaneDescriptor {
        self.descriptor
    }

    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

#[derive(Debug, Clone)]
pub struct DmabufBufferHandle {
    size: BufferSize,
    format: DrmFormat,
    planes: Vec<DmabufPlane>,
}

impl DmabufBufferHandle {
    pub fn new(
        size: BufferSize,
        format: DrmFormat,
        planes: Vec<DmabufPlane>,
    ) -> Result<Self, BufferValidationError> {
        let descriptors = planes
            .iter()
            .map(DmabufPlane::descriptor)
            .collect::<Vec<_>>();
        validate_dmabuf_planes(size, &descriptors)?;
        Ok(Self {
            size,
            format,
            planes,
        })
    }

    pub const fn size(&self) -> BufferSize {
        self.size
    }

    pub const fn format(&self) -> DrmFormat {
        self.format
    }

    pub fn planes(&self) -> &[DmabufPlane] {
        &self.planes
    }
}

fn validate_dmabuf_planes(
    _size: BufferSize,
    planes: &[DmabufPlaneDescriptor],
) -> Result<(), BufferValidationError> {
    let Some(first) = planes.first() else {
        return Err(BufferValidationError::MissingPlane);
    };
    if first.plane_index != 0 {
        return Err(BufferValidationError::NonContiguousPlaneIndex);
    }
    if first.stride == 0 || first.offset % 4 != 0 {
        return Err(BufferValidationError::PlaneTooSmall);
    }
    if planes
        .windows(2)
        .any(|pair| pair[1].plane_index != pair[0].plane_index.saturating_add(1))
    {
        return Err(BufferValidationError::NonContiguousPlaneIndex);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShmBufferSnapshot {
    size: BufferSize,
    pixels: Vec<u32>,
}

impl ShmBufferSnapshot {
    pub fn new(size: BufferSize, pixels: Vec<u32>) -> Result<Self, BufferValidationError> {
        if size.pixel_count() != Some(pixels.len()) {
            return Err(BufferValidationError::PixelCountMismatch);
        }
        Ok(Self { size, pixels })
    }

    pub const fn size(&self) -> BufferSize {
        self.size
    }

    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    pub fn pixels_mut(&mut self) -> &mut Vec<u32> {
        &mut self.pixels
    }
}

#[derive(Debug, Clone)]
pub enum CommittedSurfaceBuffer {
    ShmSnapshot(ShmBufferSnapshot),
    DmabufHandle(DmabufBufferHandle),
}

impl CommittedSurfaceBuffer {
    pub fn shm_snapshot(size: BufferSize, pixels: Vec<u32>) -> Self {
        Self::ShmSnapshot(
            ShmBufferSnapshot::new(size, pixels)
                .expect("wl_shm snapshots must match their committed buffer size"),
        )
    }

    pub const fn dmabuf_handle(handle: DmabufBufferHandle) -> Self {
        Self::DmabufHandle(handle)
    }

    pub const fn source(&self) -> SurfaceBufferSource {
        match self {
            Self::ShmSnapshot(_) => SurfaceBufferSource::Shm,
            Self::DmabufHandle(_) => SurfaceBufferSource::Dmabuf,
        }
    }

    pub const fn size(&self) -> BufferSize {
        match self {
            Self::ShmSnapshot(snapshot) => snapshot.size(),
            Self::DmabufHandle(handle) => handle.size(),
        }
    }

    pub fn cpu_pixels(&self) -> Option<&[u32]> {
        match self {
            Self::ShmSnapshot(snapshot) => Some(snapshot.pixels()),
            Self::DmabufHandle(_) => None,
        }
    }

    pub fn dmabuf_handle_ref(&self) -> Option<&DmabufBufferHandle> {
        match self {
            Self::ShmSnapshot(_) => None,
            Self::DmabufHandle(handle) => Some(handle),
        }
    }

    pub fn shm_pixels_mut(&mut self) -> Option<&mut Vec<u32>> {
        match self {
            Self::ShmSnapshot(snapshot) => Some(snapshot.pixels_mut()),
            Self::DmabufHandle(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceBufferSource {
    Shm,
    Dmabuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferValidationError {
    PixelCountMismatch,
    MissingPlane,
    NonContiguousPlaneIndex,
    PlaneTooSmall,
}
use std::{
    os::fd::{AsFd, BorrowedFd, OwnedFd},
    sync::Arc,
};
