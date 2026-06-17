use std::{
    cmp::Ordering as CmpOrdering,
    fs::File,
    io,
    os::unix::fs::FileExt,
    sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    },
};

use wayland_server::{WEnum, protocol::wl_shm};

use super::RenderableSurfaceDamage;

pub(super) const WL_SHM_FORMAT_ABGR8888: u32 = 0x3432_4241;
pub(super) const WL_SHM_FORMAT_XBGR8888: u32 = 0x3432_4258;
pub(super) const WL_SHM_FORMAT_ARGB2101010: u32 = 0x3033_5241;
pub(super) const WL_SHM_FORMAT_XRGB2101010: u32 = 0x3033_5258;
pub(super) const WL_SHM_FORMAT_ABGR2101010: u32 = 0x3033_4241;
pub(super) const WL_SHM_FORMAT_XBGR2101010: u32 = 0x3033_4258;

#[derive(Debug)]
pub(super) struct ShmPoolData {
    pub(super) file: Arc<File>,
    size: AtomicI32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShmPoolResizeError {
    Shrinking,
}

impl ShmPoolData {
    pub(super) fn new(file: Arc<File>, size: i32) -> Self {
        Self {
            file,
            size: AtomicI32::new(size),
        }
    }

    pub(super) fn size(&self) -> i32 {
        self.size.load(Ordering::Relaxed)
    }

    pub(super) fn grow_to(&self, new_size: i32) -> Result<(), ShmPoolResizeError> {
        loop {
            let current_size = self.size();
            match new_size.cmp(&current_size) {
                CmpOrdering::Less => return Err(ShmPoolResizeError::Shrinking),
                CmpOrdering::Equal => return Ok(()),
                CmpOrdering::Greater => {
                    if self
                        .size
                        .compare_exchange(
                            current_size,
                            new_size,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                        )
                        .is_ok()
                    {
                        return Ok(());
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ShmBufferData {
    pub(super) pool_size: i32,
    pub(super) file: Arc<File>,
    pub(super) offset: i32,
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) stride: i32,
    pub(super) format: WEnum<wl_shm::Format>,
}

impl ShmBufferData {
    pub(super) fn fits_in_pool(&self) -> bool {
        let _ = &self.format;
        if self.offset < 0 || self.width <= 0 || self.height <= 0 || self.stride <= 0 {
            return false;
        }
        let Some(bytes) = self.height.checked_mul(self.stride) else {
            return false;
        };
        let Some(end) = self.offset.checked_add(bytes) else {
            return false;
        };

        end <= self.pool_size
    }

    pub(super) fn width(&self) -> io::Result<u32> {
        self.width.try_into().map_err(|_| invalid_shm_buffer())
    }

    pub(super) fn height(&self) -> io::Result<u32> {
        self.height.try_into().map_err(|_| invalid_shm_buffer())
    }

    pub(super) fn read_pixels(&self) -> io::Result<Vec<u32>> {
        let pixel_count = self.pixel_count()?;
        let mut pixels = vec![0; pixel_count];
        self.read_pixels_into(&mut pixels)?;
        Ok(pixels)
    }

    pub(super) fn read_pixels_into(&self, pixels: &mut Vec<u32>) -> io::Result<()> {
        if !self.fits_in_pool() {
            return Err(invalid_shm_buffer());
        }

        let width = usize::try_from(self.width).map_err(|_| invalid_shm_buffer())?;
        let height = usize::try_from(self.height).map_err(|_| invalid_shm_buffer())?;
        let stride = usize::try_from(self.stride).map_err(|_| invalid_shm_buffer())?;
        let row_pixels_bytes = width.checked_mul(4).ok_or_else(invalid_shm_buffer)?;
        if row_pixels_bytes > stride {
            return Err(invalid_shm_buffer());
        }

        let pixel_count = self.pixel_count()?;
        pixels.resize(pixel_count, 0);
        if row_pixels_bytes == stride {
            self.file.read_exact_at(
                bytemuck::cast_slice_mut(pixels.as_mut_slice()),
                self.offset as u64,
            )?;
        } else {
            for row_index in 0..height {
                let source_offset = self.offset as u64 + (row_index * stride) as u64;
                let target_start = row_index * width;
                let target_end = target_start + width;
                self.file.read_exact_at(
                    bytemuck::cast_slice_mut(&mut pixels[target_start..target_end]),
                    source_offset,
                )?;
            }
        }

        normalize_shm_argb_pixels(self.format, pixels.iter_mut())
    }

    pub(super) fn read_pixels_into_with_damage(
        &self,
        pixels: &mut Vec<u32>,
        damage: &RenderableSurfaceDamage,
    ) -> io::Result<()> {
        let pixel_count = self.pixel_count()?;
        let width = self.width()?;
        let height = self.height()?;
        if damage.is_full() || damage.covers_surface(width, height) || pixels.len() != pixel_count {
            return self.read_pixels_into(pixels);
        }
        if !self.fits_in_pool() {
            return Err(invalid_shm_buffer());
        }

        let stride = usize::try_from(self.stride).map_err(|_| invalid_shm_buffer())?;
        let surface_width = usize::try_from(width).map_err(|_| invalid_shm_buffer())?;
        let rects = damage.clipped_rects(width, height);
        if rects.is_empty() {
            return Ok(());
        }

        for rect in &rects {
            let rect_x = usize::try_from(rect.x).map_err(|_| invalid_shm_buffer())?;
            let rect_y = usize::try_from(rect.y).map_err(|_| invalid_shm_buffer())?;
            let rect_width = usize::try_from(rect.width).map_err(|_| invalid_shm_buffer())?;
            let rect_height = usize::try_from(rect.height).map_err(|_| invalid_shm_buffer())?;
            let row_bytes = rect_width.checked_mul(4).ok_or_else(invalid_shm_buffer)?;
            for row_index in 0..rect_height {
                let source_offset = self.offset as u64
                    + ((rect_y + row_index) * stride) as u64
                    + (rect_x * 4) as u64;
                let target_start = (rect_y + row_index) * surface_width + rect_x;
                let target_end = target_start + rect_width;
                let Some(target_row) = pixels.get_mut(target_start..target_end) else {
                    return Err(invalid_shm_buffer());
                };
                self.file
                    .read_exact_at(bytemuck::cast_slice_mut(target_row), source_offset)?;
                debug_assert_eq!(row_bytes, target_row.len() * 4);
            }
        }

        for rect in rects {
            let rect_x = usize::try_from(rect.x).map_err(|_| invalid_shm_buffer())?;
            let rect_y = usize::try_from(rect.y).map_err(|_| invalid_shm_buffer())?;
            let rect_width = usize::try_from(rect.width).map_err(|_| invalid_shm_buffer())?;
            let rect_height = usize::try_from(rect.height).map_err(|_| invalid_shm_buffer())?;
            for row_index in 0..rect_height {
                let start = (rect_y + row_index) * surface_width + rect_x;
                let end = start + rect_width;
                let Some(row) = pixels.get_mut(start..end) else {
                    return Err(invalid_shm_buffer());
                };
                normalize_shm_argb_pixels(self.format, row.iter_mut())?;
            }
        }

        Ok(())
    }

    fn pixel_count(&self) -> io::Result<usize> {
        let width = usize::try_from(self.width).map_err(|_| invalid_shm_buffer())?;
        let height = usize::try_from(self.height).map_err(|_| invalid_shm_buffer())?;
        width.checked_mul(height).ok_or_else(invalid_shm_buffer)
    }
}

fn normalize_shm_argb_pixels<'a>(
    format: WEnum<wl_shm::Format>,
    pixels: impl IntoIterator<Item = &'a mut u32>,
) -> io::Result<()> {
    match format {
        WEnum::Value(wl_shm::Format::Argb8888) => {}
        WEnum::Value(wl_shm::Format::Xrgb8888) => {
            for pixel in pixels {
                *pixel |= 0xff00_0000;
            }
        }
        WEnum::Unknown(WL_SHM_FORMAT_ABGR8888) => {
            for pixel in pixels {
                *pixel = abgr8888_to_argb8888(*pixel);
            }
        }
        WEnum::Unknown(WL_SHM_FORMAT_XBGR8888) => {
            for pixel in pixels {
                *pixel = abgr8888_to_argb8888(*pixel) | 0xff00_0000;
            }
        }
        WEnum::Unknown(WL_SHM_FORMAT_ARGB2101010) => {
            for pixel in pixels {
                *pixel = argb2101010_to_argb8888(*pixel);
            }
        }
        WEnum::Unknown(WL_SHM_FORMAT_XRGB2101010) => {
            for pixel in pixels {
                *pixel = xrgb2101010_to_argb8888(*pixel);
            }
        }
        WEnum::Unknown(WL_SHM_FORMAT_ABGR2101010) => {
            for pixel in pixels {
                *pixel = abgr2101010_to_argb8888(*pixel);
            }
        }
        WEnum::Unknown(WL_SHM_FORMAT_XBGR2101010) => {
            for pixel in pixels {
                *pixel = xbgr2101010_to_argb8888(*pixel);
            }
        }
        _ => return Err(invalid_shm_buffer()),
    }

    Ok(())
}

fn abgr8888_to_argb8888(pixel: u32) -> u32 {
    (pixel & 0xff00_ff00) | ((pixel & 0x00ff_0000) >> 16) | ((pixel & 0x0000_00ff) << 16)
}

fn scale_10_to_8(value: u32) -> u32 {
    (value * 255 + 511) / 1023
}

fn scale_2_to_8(value: u32) -> u32 {
    value * 85
}

fn pack_argb8888(alpha: u32, red: u32, green: u32, blue: u32) -> u32 {
    (alpha << 24) | (red << 16) | (green << 8) | blue
}

fn argb2101010_to_argb8888(pixel: u32) -> u32 {
    let alpha = scale_2_to_8((pixel >> 30) & 0x3);
    let red = scale_10_to_8((pixel >> 20) & 0x3ff);
    let green = scale_10_to_8((pixel >> 10) & 0x3ff);
    let blue = scale_10_to_8(pixel & 0x3ff);
    pack_argb8888(alpha, red, green, blue)
}

fn xrgb2101010_to_argb8888(pixel: u32) -> u32 {
    argb2101010_to_argb8888(pixel) | 0xff00_0000
}

fn abgr2101010_to_argb8888(pixel: u32) -> u32 {
    let alpha = scale_2_to_8((pixel >> 30) & 0x3);
    let blue = scale_10_to_8((pixel >> 20) & 0x3ff);
    let green = scale_10_to_8((pixel >> 10) & 0x3ff);
    let red = scale_10_to_8(pixel & 0x3ff);
    pack_argb8888(alpha, red, green, blue)
}

fn xbgr2101010_to_argb8888(pixel: u32) -> u32 {
    abgr2101010_to_argb8888(pixel) | 0xff00_0000
}

pub(super) fn invalid_shm_buffer() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid wl_shm buffer")
}

pub(super) fn invalid_buffer_for_cpu_read() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "linux-dmabuf buffers do not expose CPU pixels",
    )
}
