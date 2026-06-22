use std::{
    num::NonZeroU64,
    os::fd::{AsFd, BorrowedFd, OwnedFd},
    sync::{Arc, Weak},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BufferId(NonZeroU64);

impl BufferId {
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone)]
pub struct BufferIdentity {
    id: BufferId,
    lifetime: Arc<()>,
}

impl BufferIdentity {
    pub const fn id(&self) -> BufferId {
        self.id
    }

    pub fn downgrade(&self) -> WeakBufferIdentity {
        WeakBufferIdentity {
            id: self.id,
            lifetime: Arc::downgrade(&self.lifetime),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WeakBufferIdentity {
    id: BufferId,
    lifetime: Weak<()>,
}

impl WeakBufferIdentity {
    pub const fn id(&self) -> BufferId {
        self.id
    }

    pub fn is_alive(&self) -> bool {
        self.lifetime.strong_count() != 0
    }
}

#[derive(Debug)]
pub struct BufferIdAllocator {
    next: Option<NonZeroU64>,
}

impl Default for BufferIdAllocator {
    fn default() -> Self {
        Self {
            next: NonZeroU64::new(1),
        }
    }
}

impl BufferIdAllocator {
    pub fn allocate(&mut self) -> Option<BufferIdentity> {
        let next = self.next?;
        self.next = next.get().checked_add(1).and_then(NonZeroU64::new);
        Some(BufferIdentity {
            id: BufferId(next),
            lifetime: Arc::new(()),
        })
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DmabufImageKey {
    buffer_id: BufferId,
    width: u32,
    height: u32,
    format: u32,
    planes: Vec<DmabufPlaneLayout>,
}

impl DmabufImageKey {
    pub fn from_handle(buffer_id: BufferId, handle: &DmabufBufferHandle) -> Self {
        let size = handle.size();
        let planes = handle
            .planes()
            .iter()
            .map(|plane| {
                let descriptor = plane.descriptor();
                DmabufPlaneLayout {
                    plane_index: descriptor.plane_index,
                    offset: descriptor.offset,
                    stride: descriptor.stride,
                    modifier: descriptor.modifier.0,
                }
            })
            .collect();
        Self {
            buffer_id,
            width: size.width,
            height: size.height,
            format: handle.format().as_fourcc(),
            planes,
        }
    }

    pub const fn buffer_id(&self) -> BufferId {
        self.buffer_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DmabufPlaneLayout {
    plane_index: u32,
    offset: u32,
    stride: u32,
    modifier: u64,
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
    ShmSnapshot {
        identity: BufferIdentity,
        snapshot: ShmBufferSnapshot,
    },
    DmabufHandle {
        identity: BufferIdentity,
        handle: DmabufBufferHandle,
    },
}

impl CommittedSurfaceBuffer {
    pub fn shm_snapshot(identity: BufferIdentity, size: BufferSize, pixels: Vec<u32>) -> Self {
        Self::ShmSnapshot {
            identity,
            snapshot: ShmBufferSnapshot::new(size, pixels)
                .expect("wl_shm snapshots must match their committed buffer size"),
        }
    }

    pub const fn dmabuf_handle(identity: BufferIdentity, handle: DmabufBufferHandle) -> Self {
        Self::DmabufHandle { identity, handle }
    }

    pub const fn buffer_id(&self) -> BufferId {
        match self {
            Self::ShmSnapshot { identity, .. } | Self::DmabufHandle { identity, .. } => {
                identity.id()
            }
        }
    }

    pub fn buffer_identity(&self) -> &BufferIdentity {
        match self {
            Self::ShmSnapshot { identity, .. } | Self::DmabufHandle { identity, .. } => identity,
        }
    }

    pub const fn source(&self) -> SurfaceBufferSource {
        match self {
            Self::ShmSnapshot { .. } => SurfaceBufferSource::Shm,
            Self::DmabufHandle { .. } => SurfaceBufferSource::Dmabuf,
        }
    }

    pub const fn size(&self) -> BufferSize {
        match self {
            Self::ShmSnapshot { snapshot, .. } => snapshot.size(),
            Self::DmabufHandle { handle, .. } => handle.size(),
        }
    }

    pub fn cpu_pixels(&self) -> Option<&[u32]> {
        match self {
            Self::ShmSnapshot { snapshot, .. } => Some(snapshot.pixels()),
            Self::DmabufHandle { .. } => None,
        }
    }

    pub fn dmabuf_handle_ref(&self) -> Option<&DmabufBufferHandle> {
        match self {
            Self::ShmSnapshot { .. } => None,
            Self::DmabufHandle { handle, .. } => Some(handle),
        }
    }

    pub fn shm_pixels_mut(&mut self) -> Option<&mut Vec<u32>> {
        match self {
            Self::ShmSnapshot { snapshot, .. } => Some(snapshot.pixels_mut()),
            Self::DmabufHandle { .. } => None,
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

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn buffer_ids_are_nonzero_monotonic_and_not_reused() {
        let mut allocator = BufferIdAllocator::default();

        let first = allocator.allocate().expect("first buffer identity");
        let second = allocator.allocate().expect("second buffer identity");

        assert_ne!(first.id(), second.id());
        assert!(first.id().get() > 0);
        assert!(second.id().get() > first.id().get());
    }

    #[test]
    fn cloned_buffer_identity_keeps_the_same_lifecycle() {
        let mut allocator = BufferIdAllocator::default();
        let identity = allocator.allocate().expect("buffer identity");
        let clone = identity.clone();
        let weak = identity.downgrade();

        drop(identity);
        assert!(weak.is_alive());
        assert_eq!(clone.id(), weak.id());

        drop(clone);
        assert!(!weak.is_alive());
    }
}
