use super::*;

pub(crate) const NATIVE_HARDWARE_CURSOR_SIZE: u32 = 64;

pub(crate) struct NativeHardwareCursor {
    pub(crate) bo: gbm::BufferObject<()>,
    pub(crate) _device: gbm::Device<OwnedFd>,
    pub(crate) fd: RawFd,
    pub(crate) crtc_id: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) active: bool,
}

impl NativeHardwareCursor {
    pub(crate) fn create(kms: &fs::File, crtc_id: u32) -> io::Result<Self> {
        let gbm_fd = duplicate_fd_cloexec(kms.as_raw_fd()).map_err(io::Error::from_raw_os_error)?;
        let device = gbm::Device::new(gbm_fd)?;
        let usage = gbm::BufferObjectFlags::CURSOR | gbm::BufferObjectFlags::WRITE;
        if !device.is_format_supported(gbm::Format::Argb8888, usage) {
            return Err(io::Error::other(
                "GBM device does not support writable ARGB8888 cursor buffers",
            ));
        }

        let mut bo = device.create_buffer_object(
            NATIVE_HARDWARE_CURSOR_SIZE,
            NATIVE_HARDWARE_CURSOR_SIZE,
            gbm::Format::Argb8888,
            usage,
        )?;
        let (texture_width, texture_height) = cursor_texture_size();
        let cursor_bytes = native_cursor_argb_bytes(
            &cursor_texture_pixels(),
            texture_width,
            texture_height,
            bo.width(),
            bo.height(),
            bo.stride(),
        )?;
        bo.write(&cursor_bytes)?;

        Ok(Self {
            fd: kms.as_raw_fd(),
            crtc_id,
            width: bo.width(),
            height: bo.height(),
            bo,
            _device: device,
            active: false,
        })
    }

    pub(crate) fn enable(&mut self) -> io::Result<()> {
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        #[allow(deprecated)]
        drm_ffi::mode::set_cursor(fd, self.crtc_id, self.handle(), self.width, self.height)?;
        self.active = true;
        Ok(())
    }

    pub(crate) fn move_to(&mut self, x: i32, y: i32) -> io::Result<()> {
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        #[allow(deprecated)]
        drm_ffi::mode::move_cursor(fd, self.crtc_id, x, y)?;
        Ok(())
    }

    pub(crate) fn disable(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        #[allow(deprecated)]
        drm_ffi::mode::set_cursor(fd, self.crtc_id, 0, 0, 0)?;
        self.active = false;
        Ok(())
    }

    pub(crate) fn disarm_drm_cleanup(&mut self) {
        self.active = false;
    }

    pub(crate) fn handle(&self) -> u32 {
        unsafe { self.bo.handle().u32_ }
    }
}

impl Drop for NativeHardwareCursor {
    fn drop(&mut self) {
        let _ = self.disable();
    }
}

pub(crate) fn native_cursor_argb_bytes(
    pixels: &[u32],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    pitch: u32,
) -> io::Result<Vec<u8>> {
    if source_width > target_width || source_height > target_height {
        return Err(io::Error::other(
            "native cursor texture exceeds target buffer",
        ));
    }
    let source_width = usize::try_from(source_width)
        .map_err(|_| io::Error::other("native cursor source width overflow"))?;
    let source_height = usize::try_from(source_height)
        .map_err(|_| io::Error::other("native cursor source height overflow"))?;
    let target_width = usize::try_from(target_width)
        .map_err(|_| io::Error::other("native cursor target width overflow"))?;
    let target_height = usize::try_from(target_height)
        .map_err(|_| io::Error::other("native cursor target height overflow"))?;
    let pitch =
        usize::try_from(pitch).map_err(|_| io::Error::other("invalid native cursor pitch"))?;
    let row_bytes = source_width
        .checked_mul(mem::size_of::<u32>())
        .ok_or_else(|| io::Error::other("native cursor source row overflow"))?;
    let min_pitch = target_width
        .checked_mul(mem::size_of::<u32>())
        .ok_or_else(|| io::Error::other("native cursor target row overflow"))?;
    if pitch < min_pitch {
        return Err(io::Error::other("native cursor pitch is too small"));
    }
    let pixel_count = source_width
        .checked_mul(source_height)
        .ok_or_else(|| io::Error::other("native cursor source overflow"))?;
    if pixels.len() < pixel_count {
        return Err(io::Error::other("native cursor source is too small"));
    }
    let byte_len = pitch
        .checked_mul(target_height)
        .ok_or_else(|| io::Error::other("native cursor target overflow"))?;
    let source_bytes_len = pixel_count
        .checked_mul(mem::size_of::<u32>())
        .ok_or_else(|| io::Error::other("native cursor source byte overflow"))?;
    let source_bytes =
        unsafe { slice::from_raw_parts(pixels.as_ptr().cast::<u8>(), source_bytes_len) };
    let mut bytes = vec![0; byte_len];
    for y in 0..source_height {
        let source_start = y
            .checked_mul(row_bytes)
            .ok_or_else(|| io::Error::other("native cursor source offset overflow"))?;
        let target_start = y
            .checked_mul(pitch)
            .ok_or_else(|| io::Error::other("native cursor target offset overflow"))?;
        bytes[target_start..target_start + row_bytes]
            .copy_from_slice(&source_bytes[source_start..source_start + row_bytes]);
    }
    Ok(bytes)
}
