use super::super::*;

pub(in crate::egl_renderer::tests) fn create_effect_test_texture(
    gl: &glow::Context,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> glow::Texture {
    let texture = unsafe { gl.create_texture().expect("effect test texture creates") };
    unsafe {
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::NEAREST as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::NEAREST as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA as i32,
            width as i32,
            height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(pixels)),
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
    texture
}

pub(in crate::egl_renderer::tests) fn set_effect_test_texture_filter(
    gl: &glow::Context,
    texture: glow::Texture,
    filter: u32,
) {
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter as i32);
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
}

pub(in crate::egl_renderer::tests) fn read_effect_test_pixels(
    gl: &glow::Context,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let mut pixels = vec![0_u8; width as usize * height as usize * 4];
    unsafe {
        gl.flush();
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(
            0,
            0,
            width as i32,
            height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixels)),
        );
    }
    pixels
}

pub(in crate::egl_renderer::tests) fn read_effect_test_pixels_f32(
    gl: &glow::Context,
    width: u32,
    height: u32,
) -> Vec<f32> {
    let mut pixels = vec![0.0_f32; width as usize * height as usize * 4];
    let pixels_bytes: &mut [u8] = bytemuck::cast_slice_mut(&mut pixels);
    unsafe {
        gl.flush();
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(
            0,
            0,
            width as i32,
            height as i32,
            glow::RGBA,
            glow::FLOAT,
            glow::PixelPackData::Slice(Some(pixels_bytes)),
        );
    }
    pixels
}

pub(in crate::egl_renderer::tests) fn assert_effect_test_pixel(
    pixels: &[u8],
    width: u32,
    x: u32,
    y: u32,
    expected: [u8; 4],
) {
    let index = ((y * width + x) * 4) as usize;
    assert_eq!(&pixels[index..index + 4], &expected, "pixel ({x}, {y})");
}

pub(in crate::egl_renderer::tests) fn effect_test_pixel(
    pixels: &[u8],
    width: u32,
    x: u32,
    y: u32,
) -> [u8; 4] {
    let index = ((y * width + x) * 4) as usize;
    pixels[index..index + 4]
        .try_into()
        .expect("RGBA test pixel has four channels")
}

pub(in crate::egl_renderer::tests) fn set_effect_test_uniform_i32(
    gl: &glow::Context,
    program: glow::Program,
    name: &str,
    value: i32,
) {
    let location = unsafe {
        gl.get_uniform_location(program, name)
            .unwrap_or_else(|| panic!("uniform {name} is active"))
    };
    unsafe { gl.uniform_1_i32(Some(&location), value) };
}

pub(in crate::egl_renderer::tests) fn set_effect_test_uniform_2_f32(
    gl: &glow::Context,
    program: glow::Program,
    name: &str,
    x: f32,
    y: f32,
) {
    let location = unsafe {
        gl.get_uniform_location(program, name)
            .unwrap_or_else(|| panic!("uniform {name} is active"))
    };
    unsafe { gl.uniform_2_f32(Some(&location), x, y) };
}

pub(in crate::egl_renderer::tests) fn set_effect_test_uniform_4_f32(
    gl: &glow::Context,
    program: glow::Program,
    name: &str,
    values: [f32; 4],
) {
    let location = unsafe {
        gl.get_uniform_location(program, name)
            .unwrap_or_else(|| panic!("uniform {name} is active"))
    };
    unsafe { gl.uniform_4_f32(Some(&location), values[0], values[1], values[2], values[3]) };
}

#[allow(clippy::too_many_arguments)]
pub(in crate::egl_renderer::tests) fn draw_effect_test(
    gl: &glow::Context,
    program: glow::Program,
    quad: glow::VertexArray,
    input_texture: glow::Texture,
    width: u32,
    height: u32,
    target_flip_y: bool,
    input_uniforms: Option<(&str, bool)>,
    configure: impl Fn(&glow::Context, glow::Program),
) {
    unsafe {
        gl.viewport(0, 0, width as i32, height as i32);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::BLEND);
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.use_program(Some(program));
        set_effect_test_uniform_i32(
            gl,
            program,
            "u_effect_target_flip_y",
            i32::from(target_flip_y),
        );
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));
        if let Some((input_flip_uniform, input_flip_y)) = input_uniforms {
            set_effect_test_uniform_i32(gl, program, input_flip_uniform, i32::from(input_flip_y));
            set_effect_test_uniform_i32(gl, program, "u_effect_input", 0);
        }
        configure(gl, program);
        gl.bind_vertex_array(Some(quad));
        gl.draw_arrays(glow::TRIANGLES, 0, 6);
        gl.bind_vertex_array(None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.use_program(None);
    }
}

pub(in crate::egl_renderer::tests) fn canonical_two_by_two_pixels() -> Vec<u8> {
    [
        [0, 0, 255, 255],
        [0, 0, 255, 255],
        [255, 0, 0, 255],
        [255, 0, 0, 255],
    ]
    .into_iter()
    .flatten()
    .collect()
}
