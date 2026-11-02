use super::*;
use crate::egl_renderer::screencopy_blit::{ScreencopyBlitRegion, blit_framebuffer_region};

fn rgba_target(
    gl: &glow::Context,
    width: u32,
    height: u32,
    pixels: Option<&[u8]>,
) -> (glow::Texture, glow::Framebuffer) {
    unsafe {
        let texture = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            width as i32,
            height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(pixels),
        );
        let framebuffer = gl.create_framebuffer().unwrap();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        assert_eq!(
            gl.check_framebuffer_status(glow::FRAMEBUFFER),
            glow::FRAMEBUFFER_COMPLETE
        );
        (texture, framebuffer)
    }
}

fn source_pixel(x: u32, y: u32) -> [u8; 4] {
    [x as u8 * 40, y as u8 * 40, 200, 255]
}

/// Memory row 0 is GL row 0 in both the TopLeftScanout output slot and an
/// imported client buffer, so the screencopy blit must keep rows unflipped
/// and crop at the region's top-left corner.
#[test]
fn screencopy_blit_keeps_top_left_rows_and_crops_the_region() {
    let harness = GlesEffectTestHarness::new(8, 8);
    let gl = &harness.gl;
    let (width, height) = (5_u32, 4_u32);
    let source_pixels = (0..height)
        .flat_map(|y| (0..width).flat_map(move |x| source_pixel(x, y)))
        .collect::<Vec<_>>();
    let (source_texture, source) = rgba_target(gl, width, height, Some(&source_pixels));
    let region = ScreencopyBlitRegion {
        x: 1,
        y: 2,
        width: 3,
        height: 2,
    };
    let (destination_texture, destination) = rgba_target(gl, region.width, region.height, None);
    unsafe {
        gl.enable(glow::SCISSOR_TEST);
        gl.scissor(0, 0, 1, 1);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    }

    blit_framebuffer_region(gl, source, destination, region).unwrap();

    let mut readback = vec![0_u8; (region.width * region.height * 4) as usize];
    unsafe {
        // The caller's scissor and framebuffer bindings survive the blit.
        assert!(gl.is_enabled(glow::SCISSOR_TEST));
        assert_eq!(
            gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING),
            None
        );
        gl.disable(glow::SCISSOR_TEST);
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(destination));
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(
            0,
            0,
            region.width as i32,
            region.height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut readback)),
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.delete_framebuffer(source);
        gl.delete_framebuffer(destination);
        gl.delete_texture(source_texture);
        gl.delete_texture(destination_texture);
    }
    for row in 0..region.height {
        for column in 0..region.width {
            let start = ((row * region.width + column) * 4) as usize;
            assert_eq!(
                &readback[start..start + 4],
                &source_pixel(region.x + column, region.y + row),
                "destination pixel ({column}, {row})"
            );
        }
    }
}

#[test]
fn screencopy_blit_region_must_fit_the_source() {
    let region = |x, y, width, height| ScreencopyBlitRegion {
        x,
        y,
        width,
        height,
    };
    assert!(region(0, 0, 1920, 1080).fits_within(1920, 1080));
    assert!(region(1900, 1000, 20, 80).fits_within(1920, 1080));
    assert!(!region(1900, 1000, 21, 80).fits_within(1920, 1080));
    assert!(!region(0, 0, 0, 10).fits_within(1920, 1080));
    assert!(!region(u32::MAX, 0, 2, 2).fits_within(1920, 1080));
}

const SOURCE_BLUE: [u8; 4] = [0, 0, 255, 255];
const CURSOR_RED: [u8; 4] = [255, 0, 0, 255];
const CURSOR_GREEN: [u8; 4] = [0, 255, 0, 255];

fn read_rgba(
    gl: &glow::Context,
    framebuffer: glow::Framebuffer,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let mut readback = vec![0_u8; (width * height * 4) as usize];
    unsafe {
        let previous = gl.get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING);
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(
            0,
            0,
            width as i32,
            height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut readback)),
        );
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, previous);
    }
    readback
}

/// Blit `region` of a solid blue 8x6 output into a fresh destination, with
/// `cursor_at` (output position, hotspot applied by the renderer) staged as
/// the frame's capture cursor, and return the destination pixels.
fn blit_with_capture_cursor(
    cursor_at: Option<(i32, i32)>,
    region: ScreencopyBlitRegion,
    overlay_cursor: bool,
) -> Vec<u8> {
    let mut harness = GlesEffectTestHarness::new(8, 8);
    // 2x2 cursor, hotspot (1, 1): red, green / transparent, red.
    let cursor_image = std::sync::Arc::new(
        oblivion_one::cursor_theme::CompositorCursorImage::from_argb8888(
            vec![0xffff_0000, 0xff00_ff00, 0x0000_0000, 0xffff_0000],
            2,
            2,
            1,
            1,
        )
        .expect("test cursor image"),
    );
    harness.renderer.set_cursor_image(cursor_image.clone());
    let renderer = &mut harness.renderer;
    renderer
        .resources
        .ensure_cursor_resource(&renderer.gl, &harness.egl, harness.display, &cursor_image)
        .expect("cursor texture uploads");
    renderer.scene_state.capture_cursor = cursor_at.and_then(|position| {
        crate::egl_renderer::scene_state::theme_cursor_quad(
            position,
            &cursor_image,
            renderer.resources.texture_view().cursor_size(),
        )
    });

    let gl = &harness.gl;
    let (source_width, source_height) = (8_u32, 6_u32);
    let source_pixels = SOURCE_BLUE.repeat((source_width * source_height) as usize);
    let (source_texture, source) =
        rgba_target(gl, source_width, source_height, Some(&source_pixels));
    let (destination_texture, destination) = rgba_target(gl, region.width, region.height, None);
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.enable(glow::SCISSOR_TEST);
        gl.scissor(0, 0, 1, 1);
        gl.viewport(0, 0, 7, 3);
        gl.disable(glow::BLEND);
        gl.use_program(None);
    }

    blit_framebuffer_region(gl, source, destination, region).unwrap();
    if overlay_cursor {
        harness
            .renderer
            .draw_capture_cursor(destination, region)
            .expect("capture cursor draws");
    }

    let gl = &harness.gl;
    unsafe {
        // Everything the cursor draw touched is restored.
        assert_eq!(
            gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING),
            None
        );
        assert_eq!(
            gl.get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING),
            None
        );
        assert!(gl.is_enabled(glow::SCISSOR_TEST));
        assert!(!gl.is_enabled(glow::BLEND));
        let mut viewport = [0; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
        assert_eq!(viewport, [0, 0, 7, 3]);
        assert_eq!(gl.get_parameter_program(glow::CURRENT_PROGRAM), None);
        gl.disable(glow::SCISSOR_TEST);
    }
    let pixels = read_rgba(gl, destination, region.width, region.height);
    unsafe {
        gl.delete_framebuffer(source);
        gl.delete_framebuffer(destination);
        gl.delete_texture(source_texture);
        gl.delete_texture(destination_texture);
    }
    pixels
}

fn pixel_at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * width + x) * 4) as usize;
    pixels[start..start + 4].try_into().unwrap()
}

/// A cursor-overlay copy of a hardware-cursor frame gets the cursor sprite
/// at (position - hotspot - region origin), alpha blended over the frame.
#[test]
fn capture_cursor_lands_in_the_copy_at_the_region_relative_hotspot_position() {
    let region = ScreencopyBlitRegion {
        x: 2,
        y: 1,
        width: 5,
        height: 4,
    };
    // Output position (4, 3) minus hotspot (1, 1) minus region origin (2, 1).
    let pixels = blit_with_capture_cursor(Some((4, 3)), region, true);
    let expected = |x: u32, y: u32| match (x, y) {
        (1, 1) | (2, 2) => CURSOR_RED,
        (2, 1) => CURSOR_GREEN,
        _ => SOURCE_BLUE,
    };
    for y in 0..region.height {
        for x in 0..region.width {
            assert_eq!(
                pixel_at(&pixels, region.width, x, y),
                expected(x, y),
                "destination pixel ({x}, {y})"
            );
        }
    }
}

/// A cursor partly outside the region is clipped to it.
#[test]
fn capture_cursor_is_clipped_to_the_region() {
    let region = ScreencopyBlitRegion {
        x: 2,
        y: 1,
        width: 5,
        height: 4,
    };
    // Top-left lands at (-1, -1): only the bottom-right red texel is inside.
    let pixels = blit_with_capture_cursor(Some((2, 1)), region, true);
    for y in 0..region.height {
        for x in 0..region.width {
            let expected = if (x, y) == (0, 0) {
                CURSOR_RED
            } else {
                SOURCE_BLUE
            };
            assert_eq!(
                pixel_at(&pixels, region.width, x, y),
                expected,
                "({x}, {y})"
            );
        }
    }
}

/// Copies without `overlay_cursor`, and frames with no staged capture cursor
/// (software-composited or hidden cursor), are the plain blit.
#[test]
fn capture_cursor_is_absent_without_overlay_or_staged_cursor() {
    let region = ScreencopyBlitRegion {
        x: 0,
        y: 0,
        width: 8,
        height: 6,
    };
    let plain = SOURCE_BLUE.repeat(48);
    assert_eq!(blit_with_capture_cursor(Some((4, 3)), region, false), plain);
    assert_eq!(blit_with_capture_cursor(None, region, true), plain);
}
