use std::{
    borrow::Cow,
    ffi::CString,
    fs::File,
    io::{self, Write as _},
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
};
use wayland_server::Resource as _;

#[cfg(test)]
use std::io::{Read as _, Seek as _, SeekFrom};

use crate::egl_renderer::OutputFramebufferOrigin;

pub(crate) const MAX_CAPTURE_PAYLOAD_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const RGBA8888_FORMAT: u32 = 0x3432_4152;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CaptureDimensions {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) stride: u32,
    pub(crate) payload_len: usize,
}

impl CaptureDimensions {
    pub(crate) fn checked(width: u32, height: u32) -> io::Result<Self> {
        if width == 0 || height == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "capture dimensions must be non-zero",
            ));
        }
        let stride = width.checked_mul(4).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "capture stride overflow")
        })?;
        let payload_len = usize::try_from(stride)
            .ok()
            .and_then(|stride| stride.checked_mul(usize::try_from(height).ok()?))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "capture size overflow"))?;
        if payload_len > MAX_CAPTURE_PAYLOAD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "capture payload exceeds 256 MiB",
            ));
        }
        Ok(Self {
            width,
            height,
            stride,
            payload_len,
        })
    }
}

pub(crate) fn normalize_rgba_readback(
    readback: Vec<u8>,
    width: u32,
    height: u32,
    origin: OutputFramebufferOrigin,
    preserve_alpha: bool,
) -> io::Result<Vec<u8>> {
    let dimensions = CaptureDimensions::checked(width, height)?;
    if readback.len() != dimensions.payload_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "GL readback length does not match capture dimensions",
        ));
    }
    let row_len = usize::try_from(dimensions.stride).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "capture stride is not addressable",
        )
    })?;
    let mut normalized = vec![0; readback.len()];
    for displayed_row in 0..usize::try_from(height).unwrap_or(0) {
        let source_row = match origin {
            OutputFramebufferOrigin::BottomLeft => {
                usize::try_from(height).unwrap_or(0) - displayed_row - 1
            }
            OutputFramebufferOrigin::TopLeftScanout => displayed_row,
        };
        let source = &readback[source_row * row_len..(source_row + 1) * row_len];
        let destination = &mut normalized[displayed_row * row_len..(displayed_row + 1) * row_len];
        destination.copy_from_slice(source);
        // Output captures are opaque by definition. A window preview keeps the
        // window's own premultiplied alpha so translucent windows stay
        // translucent instead of collapsing onto black.
        if !preserve_alpha {
            for alpha in destination.chunks_exact_mut(4).map(|pixel| &mut pixel[3]) {
                *alpha = 0xff;
            }
        }
    }
    Ok(normalized)
}

#[derive(Debug)]
pub(crate) struct SealedMemfd(OwnedFd);

impl SealedMemfd {
    pub(crate) fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }

    #[cfg(test)]
    fn metadata(&self) -> io::Result<std::fs::Metadata> {
        File::from(self.0.try_clone()?).metadata()
    }

    #[cfg(test)]
    fn as_raw_fd_flags(&self) -> io::Result<i32> {
        // SAFETY: the owned descriptor is live for the duration of this call.
        let flags = unsafe { libc::fcntl(self.0.as_raw_fd(), libc::F_GETFD) };
        (flags >= 0)
            .then_some(flags)
            .ok_or_else(io::Error::last_os_error)
    }

    #[cfg(test)]
    fn read_all(&self) -> io::Result<Vec<u8>> {
        let mut file = File::from(self.0.try_clone()?);
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    #[cfg(test)]
    fn has_all_required_seals(&self) -> io::Result<bool> {
        // SAFETY: the owned descriptor is live for the duration of this call.
        let seals = unsafe { libc::fcntl(self.0.as_raw_fd(), libc::F_GET_SEALS) };
        if seals < 0 {
            return Err(io::Error::last_os_error());
        }
        let required =
            libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE | libc::F_SEAL_SEAL;
        Ok(seals & required == required)
    }
}

pub(crate) fn create_sealed_memfd(payload: &[u8]) -> io::Result<SealedMemfd> {
    if payload.len() > MAX_CAPTURE_PAYLOAD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "capture payload exceeds 256 MiB",
        ));
    }
    let name = CString::new("astrea-screenshot").expect("static memfd name has no nul");
    // SAFETY: `name` is a valid nul-terminated string and the flags are the
    // Linux memfd API contract. On success ownership is transferred below.
    let raw_fd =
        unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if raw_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw_fd` is a unique descriptor returned by memfd_create.
    let owned_fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    let mut file = File::from(owned_fd);
    file.write_all(payload)?;
    file.flush()?;
    let raw_fd = file.as_raw_fd();
    let seal_flags =
        libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE | libc::F_SEAL_SEAL;
    // SAFETY: `raw_fd` remains owned by `file` and is valid for this call.
    let result = unsafe { libc::fcntl(raw_fd, libc::F_ADD_SEALS, seal_flags) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(SealedMemfd(file.into()))
}

/// Hand one rendered RGBA payload to the protocol that requested it.
pub(crate) fn deliver_screen_capture_payload(
    request: oblivion_one::compositor::ScreenCaptureRequest,
    payload: &[u8],
    dimensions: CaptureDimensions,
) {
    use oblivion_one::compositor::ScreenCaptureRequest;
    match request {
        ScreenCaptureRequest::Astrea(pending) => {
            let mut window_size = None;
            let (payload, dimensions) = if let Some(target) = pending.window_preview {
                window_size = target.window_size;
                let Some((width, height)) = target.image_size else {
                    pending.capture.failed("render_failed".to_owned());
                    return;
                };
                match compact_window_preview(payload, dimensions, width, height) {
                    Ok(image) => image,
                    Err(_) => {
                        pending.capture.failed("render_failed".to_owned());
                        return;
                    }
                }
            } else {
                (Cow::Borrowed(payload), dimensions)
            };
            let memfd = match create_sealed_memfd(&payload) {
                Ok(memfd) => memfd,
                Err(error) => {
                    eprintln!("native screen capture allocation failed: {error}");
                    pending.capture.failed("allocation_failed".to_owned());
                    return;
                }
            };
            if let Some((width, height)) = window_size
                && pending.capture.version() >= 3
            {
                pending.capture.window_size(width, height);
            }
            pending.capture.ready(
                memfd.as_fd(),
                dimensions.width,
                dimensions.height,
                dimensions.stride,
                RGBA8888_FORMAT,
            );
        }
        ScreenCaptureRequest::Screencopy(pending) => {
            if let Err(error) = pending.deliver_rgba(payload, dimensions.width, dimensions.height) {
                eprintln!("native screencopy delivery failed: {error}");
            }
        }
    }
}

// Native capture uses its existing isolated framebuffer. Export just the
// bounded top-left window image, never the unused framebuffer padding.
fn compact_window_preview(
    payload: &[u8],
    frame: CaptureDimensions,
    width: u32,
    height: u32,
) -> io::Result<(Cow<'_, [u8]>, CaptureDimensions)> {
    if payload.len() != frame.payload_len || width > frame.width || height > frame.height {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid preview bounds",
        ));
    }
    let image = CaptureDimensions::checked(width, height)?;
    if image == frame {
        return Ok((Cow::Borrowed(payload), image));
    }
    let mut pixels = Vec::with_capacity(image.payload_len);
    for row in payload
        .chunks_exact(frame.stride as usize)
        .take(height as usize)
    {
        pixels.extend_from_slice(&row[..image.stride as usize]);
    }
    Ok((Cow::Owned(pixels), image))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_preview_export_compacts_rows_and_rejects_out_of_frame_bounds() {
        let frame = CaptureDimensions::checked(3, 2).unwrap();
        let payload = (0_u8..24).collect::<Vec<_>>();
        let (image, size) = compact_window_preview(&payload, frame, 2, 2).unwrap();
        assert_eq!(
            image.as_ref(),
            &[0, 1, 2, 3, 4, 5, 6, 7, 12, 13, 14, 15, 16, 17, 18, 19]
        );
        assert_eq!((size.width, size.height, size.stride), (2, 2, 8));
        assert!(compact_window_preview(&payload, frame, 4, 2).is_err());
        assert!(compact_window_preview(&payload[..23], frame, 2, 2).is_err());
        assert!(compact_window_preview(&payload, frame, 0, 2).is_err());
    }
    use crate::egl_renderer::OutputFramebufferOrigin;

    #[test]
    fn rgba_dimensions_are_checked_against_the_export_limit() {
        assert_eq!(CaptureDimensions::checked(2, 3).unwrap().stride, 8);
        assert!(CaptureDimensions::checked(u32::MAX, 2).is_err());
        assert!(CaptureDimensions::checked(16_385, 4_096).is_err());
    }

    #[test]
    fn readback_normalization_is_top_left_and_opaque_for_both_origins() {
        let bottom_left = vec![
            1, 2, 3, 0, // displayed bottom row
            4, 5, 6, 7, // displayed top row
        ];
        let top_left = bottom_left.clone();

        assert_eq!(
            normalize_rgba_readback(
                bottom_left.clone(),
                1,
                2,
                OutputFramebufferOrigin::BottomLeft,
                false
            )
            .unwrap(),
            vec![4, 5, 6, 255, 1, 2, 3, 255]
        );
        assert_eq!(
            normalize_rgba_readback(
                top_left.clone(),
                1,
                2,
                OutputFramebufferOrigin::TopLeftScanout,
                false
            )
            .unwrap(),
            vec![1, 2, 3, 255, 4, 5, 6, 255]
        );
        assert_eq!(
            normalize_rgba_readback(bottom_left, 1, 2, OutputFramebufferOrigin::BottomLeft, true)
                .unwrap(),
            vec![4, 5, 6, 7, 1, 2, 3, 0],
            "window previews keep their translucency"
        );
    }

    #[test]
    fn sealed_memfd_has_exact_payload_cloexec_and_required_seals() {
        let payload = vec![9, 8, 7, 6];
        let fd = create_sealed_memfd(&payload).unwrap();
        assert_eq!(fd.metadata().unwrap().len(), payload.len() as u64);
        assert_eq!(
            fd.as_raw_fd_flags().unwrap() & libc::FD_CLOEXEC,
            libc::FD_CLOEXEC
        );
        assert_eq!(fd.read_all().unwrap(), payload);
        assert!(fd.has_all_required_seals().unwrap());
    }
}
