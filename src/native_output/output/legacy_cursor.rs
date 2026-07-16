use super::*;
use oblivion_one::cursor_theme::CompositorCursorImage;

pub(crate) struct NativeLegacyHardwareCursor {
    pub(crate) bo: gbm::BufferObject<()>,
    pub(crate) _device: gbm::Device<OwnedFd>,
    pub(crate) fd: RawFd,
    pub(crate) crtc_id: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) hotspot_x: i32,
    pub(crate) hotspot_y: i32,
    pub(crate) active: bool,
}

impl NativeLegacyHardwareCursor {
    pub(crate) fn create(
        kms: &fs::File,
        crtc_id: u32,
        image: &CompositorCursorImage,
    ) -> io::Result<Self> {
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
        if image.width > bo.width() || image.height > bo.height() {
            return Err(io::Error::other(format!(
                "Legacy cursor theme image {}x{} exceeds cursor buffer {}x{}",
                image.width,
                image.height,
                bo.width(),
                bo.height()
            )));
        }
        let cursor_bytes = native_cursor_argb_bytes(
            &image.pixels_argb8888,
            image.width,
            image.height,
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
            hotspot_x: image.hotspot_x,
            hotspot_y: image.hotspot_y,
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
        {
            let (x, y) = legacy_cursor_position(x, y, self.hotspot_x, self.hotspot_y);
            drm_ffi::mode::move_cursor(fd, self.crtc_id, x, y)?;
        }
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

pub(crate) const fn legacy_cursor_position(
    pointer_x: i32,
    pointer_y: i32,
    hotspot_x: i32,
    hotspot_y: i32,
) -> (i32, i32) {
    (
        pointer_x.saturating_sub(hotspot_x),
        pointer_y.saturating_sub(hotspot_y),
    )
}

impl Drop for NativeLegacyHardwareCursor {
    fn drop(&mut self) {
        let _ = self.disable();
    }
}

#[cfg(test)]
mod tests {
    use super::legacy_cursor_position;

    #[test]
    fn legacy_cursor_move_subtracts_hotspot() {
        assert_eq!(legacy_cursor_position(40, 30, 7, 9), (33, 21));
    }
}
