use glow::HasContext;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Binary16Reference {
    lower_bits: u16,
    upper_bits: u16,
    nearest_even_bits: u16,
    toward_zero_bits: u16,
}

fn binary16_reference(value: f32) -> Binary16Reference {
    assert!(
        value.is_finite(),
        "the diagnostic reference accepts finite values"
    );
    let toward_zero_bits = f32_to_binary16_toward_zero(value);
    let toward_zero = binary16_to_f32(toward_zero_bits);
    let (lower_bits, upper_bits) = if value == toward_zero {
        (toward_zero_bits, toward_zero_bits)
    } else if value > 0.0 {
        (toward_zero_bits, next_up_binary16_bits(toward_zero_bits))
    } else {
        (next_down_binary16_bits(toward_zero_bits), toward_zero_bits)
    };

    Binary16Reference {
        lower_bits,
        upper_bits,
        nearest_even_bits: f32_to_binary16_nearest_even(value),
        toward_zero_bits,
    }
}

pub(super) fn binary16_toward_zero_for_test(value: f32) -> u16 {
    binary16_reference(value).toward_zero_bits
}

fn f32_to_binary16_nearest_even(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let fraction = bits & 0x007f_ffff;

    if exponent == 0xff {
        if fraction == 0 {
            return sign | 0x7c00;
        }
        let payload = (fraction >> 13) as u16;
        return sign | 0x7c00 | payload.max(1);
    }

    let unbiased_exponent = exponent - 127;
    if unbiased_exponent > 15 {
        return sign | 0x7c00;
    }

    if unbiased_exponent >= -14 {
        let mut half_exponent = (unbiased_exponent + 15) as u16;
        let mut half_fraction = (fraction >> 13) as u16;
        let remainder = fraction & 0x1fff;
        if remainder > 0x1000 || (remainder == 0x1000 && half_fraction & 1 != 0) {
            half_fraction += 1;
            if half_fraction == 0x0400 {
                half_fraction = 0;
                half_exponent += 1;
                if half_exponent >= 0x1f {
                    return sign | 0x7c00;
                }
            }
        }
        return sign | (half_exponent << 10) | half_fraction;
    }

    if unbiased_exponent < -25 {
        return sign;
    }

    let significand = (1 << 23) | fraction;
    let shift = (-unbiased_exponent - 1) as u32;
    let mut half_fraction = (significand >> shift) as u16;
    let mask = (1_u32 << shift) - 1;
    let remainder = significand & mask;
    let halfway = 1_u32 << (shift - 1);
    if remainder > halfway || (remainder == halfway && half_fraction & 1 != 0) {
        half_fraction += 1;
    }
    sign | half_fraction
}

fn f32_to_binary16_toward_zero(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let fraction = bits & 0x007f_ffff;

    if exponent == 0xff {
        if fraction == 0 {
            return sign | 0x7c00;
        }
        return sign | 0x7c00 | ((fraction >> 13) as u16).max(1);
    }

    let unbiased_exponent = exponent - 127;
    if unbiased_exponent > 15 {
        return sign | 0x7bff;
    }
    if unbiased_exponent >= -14 {
        return sign | (((unbiased_exponent + 15) as u16) << 10) | ((fraction >> 13) as u16);
    }
    if unbiased_exponent < -24 {
        return sign;
    }

    let significand = (1 << 23) | fraction;
    let shift = (-unbiased_exponent - 1) as u32;
    sign | ((significand >> shift) as u16)
}

fn binary16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = (bits >> 10) & 0x1f;
    let fraction = u32::from(bits & 0x03ff);
    let value_bits = match exponent {
        0 if fraction == 0 => sign,
        0 => {
            let mut normalized = fraction;
            let mut unbiased_exponent = -14_i32;
            while normalized & 0x0400 == 0 {
                normalized <<= 1;
                unbiased_exponent -= 1;
            }
            let f32_exponent = (unbiased_exponent + 127) as u32;
            sign | (f32_exponent << 23) | ((normalized & 0x03ff) << 13)
        }
        0x1f => sign | 0x7f80_0000 | (fraction << 13),
        _ => {
            let f32_exponent = u32::from(exponent) + (127 - 15);
            sign | (f32_exponent << 23) | (fraction << 13)
        }
    };
    f32::from_bits(value_bits)
}

fn next_up_binary16_bits(bits: u16) -> u16 {
    if bits & 0x7fff == 0 {
        0x0001
    } else if bits & 0x8000 != 0 {
        bits - 1
    } else {
        bits + 1
    }
}

fn next_down_binary16_bits(bits: u16) -> u16 {
    if bits & 0x7fff == 0 {
        0x8001
    } else if bits & 0x8000 != 0 {
        bits + 1
    } else {
        bits - 1
    }
}

#[derive(Debug)]
pub(super) struct MrtProbeReadback {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) stored: Vec<[f32; 4]>,
    pub(super) raw_bits: Vec<[u32; 4]>,
    pub(super) packed_half_bits: Vec<[u32; 4]>,
    pub(super) manual_half_bits: Vec<[u32; 4]>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct MrtClassification {
    pub(super) total_channels: usize,
    pub(super) store_matches_exact_input: usize,
    pub(super) store_matches_nearest_even: usize,
    pub(super) store_matches_toward_zero: usize,
    pub(super) store_matches_lower_neighbor: usize,
    pub(super) store_matches_upper_neighbor: usize,
    pub(super) store_matches_pack_half: usize,
    pub(super) store_matches_manual_half: usize,
    pub(super) coordinate_dependent_raw_values: usize,
}

pub(super) fn classify_mrt(probe: &MrtProbeReadback) -> MrtClassification {
    let mut summary = MrtClassification::default();
    let mut raw_to_store = std::collections::HashMap::<u64, std::collections::HashSet<u16>>::new();
    for index in 0..probe.stored.len() {
        for channel in 0..4 {
            let raw = f32::from_bits(probe.raw_bits[index][channel]);
            let reference = binary16_reference(raw);
            let store_bits = f32_to_binary16_nearest_even(probe.stored[index][channel]);
            summary.total_channels += 1;
            let input_is_exact = reference.lower_bits == reference.upper_bits;
            summary.store_matches_exact_input +=
                usize::from(input_is_exact && store_bits == reference.lower_bits);
            summary.store_matches_nearest_even +=
                usize::from(store_bits == reference.nearest_even_bits);
            summary.store_matches_toward_zero +=
                usize::from(store_bits == reference.toward_zero_bits);
            summary.store_matches_lower_neighbor +=
                usize::from(!input_is_exact && store_bits == reference.lower_bits);
            summary.store_matches_upper_neighbor +=
                usize::from(!input_is_exact && store_bits == reference.upper_bits);
            summary.store_matches_pack_half +=
                usize::from(store_bits == probe.packed_half_bits[index][channel] as u16);
            summary.store_matches_manual_half +=
                usize::from(store_bits == probe.manual_half_bits[index][channel] as u16);
            raw_to_store
                .entry((u64::from(channel as u32) << 32) | u64::from(raw.to_bits()))
                .or_default()
                .insert(store_bits);
        }
    }
    summary.coordinate_dependent_raw_values = raw_to_store
        .values()
        .filter(|store_values| store_values.len() > 1)
        .count();
    summary
}

pub(super) fn report_mrt_samples(label: &str, probe: &MrtProbeReadback, limit: usize) {
    let mut emitted = 0;
    for index in 0..probe.stored.len() {
        let pack_bits = probe.packed_half_bits[index].map(|bits| bits as u16);
        let raw = probe.raw_bits[index].map(f32::from_bits);
        let interesting = (0..4).any(|channel| {
            pack_bits[channel] != f32_to_binary16_nearest_even(raw[channel])
                || f32_to_binary16_nearest_even(probe.stored[index][channel]) != pack_bits[channel]
        });
        if !interesting && emitted != 0 {
            continue;
        }
        let x = index as u32 % probe.width;
        let y = index as u32 / probe.width;
        let packed = pack_bits.map(binary16_to_f32);
        let manual_bits = probe.manual_half_bits[index].map(|bits| bits as u16);
        let manual = manual_bits.map(binary16_to_f32);
        println!(
            "{label} sample=({x},{y}) Mraw={raw:?} Mpack={packed:?} Mmanual={manual:?} M0={:?}",
            probe.stored[index]
        );
        emitted += 1;
        if emitted >= limit {
            break;
        }
    }
}

pub(super) fn instrument_mrt_fragment_shader(source: &str, final_statement: &str) -> String {
    let declarations = r#"layout(location = 0) out highp vec4 out_half;
layout(location = 1) out highp uvec4 out_raw_bits;
layout(location = 2) out highp uvec4 out_packed_half_bits;
layout(location = 3) out highp uvec4 out_manual_half_bits;"#;
    let emit = r#"out_half = result;
    out_raw_bits = uvec4(
        floatBitsToUint(result.r), floatBitsToUint(result.g),
        floatBitsToUint(result.b), floatBitsToUint(result.a)
    );
    uint rg = packHalf2x16(result.rg);
    uint ba = packHalf2x16(result.ba);
    out_packed_half_bits = uvec4(rg & 0xffffu, rg >> 16u, ba & 0xffffu, ba >> 16u);
    out_manual_half_bits = uvec4(
        typhon_float_to_binary16_rtz(result.r), typhon_float_to_binary16_rtz(result.g),
        typhon_float_to_binary16_rtz(result.b), typhon_float_to_binary16_rtz(result.a)
    );"#;

    assert_eq!(source.matches("out vec4 out_color;").count(), 1);
    assert_eq!(source.matches(final_statement).count(), 1);
    let source = source.replace("out vec4 out_color;", declarations);
    let source = source.replace(
        "void main() {",
        &format!(
            "{}\n\nvoid main() {{",
            crate::egl_renderer::effects::RGBA16F_RTZ_GLSL
        ),
    );
    let stored_expression = final_statement
        .strip_prefix("    out_color = ")
        .and_then(|statement| statement.strip_suffix(';'))
        .expect("the production output statement has the expected form");
    let replacement = if stored_expression == "result" {
        emit.to_owned()
    } else {
        format!("    result = {stored_expression};\n{emit}")
    };
    source.replace(final_statement, &replacement)
}

pub(super) fn upload_rgba16f_texture(
    gl: &glow::Context,
    width: u32,
    height: u32,
    pixels: &[[f32; 4]],
    linear_filter: bool,
) -> glow::Texture {
    assert_eq!(pixels.len(), width as usize * height as usize);
    let bytes: Vec<u8> = pixels
        .iter()
        .flat_map(|pixel| pixel.iter().flat_map(|value| value.to_ne_bytes()))
        .collect();
    unsafe {
        let texture = gl.create_texture().expect("probe input texture creates");
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        let filter = if linear_filter {
            glow::LINEAR
        } else {
            glow::NEAREST
        };
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter as i32);
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
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA16F as i32,
            width as i32,
            height as i32,
            0,
            glow::RGBA,
            glow::FLOAT,
            glow::PixelUnpackData::Slice(Some(&bytes)),
        );
        texture
    }
}

pub(super) fn run_mrt_probe(
    gl: &glow::Context,
    width: u32,
    height: u32,
    vertex_source: &str,
    fragment_source: &str,
    input_texture: Option<glow::Texture>,
    dither: bool,
    set_uniforms: impl FnOnce(&glow::Context, glow::Program),
) -> MrtProbeReadback {
    assert!(width > 0 && height > 0);
    let max_draw_buffers = unsafe { gl.get_parameter_i32(glow::MAX_DRAW_BUFFERS) };
    let max_color_attachments = unsafe { gl.get_parameter_i32(glow::MAX_COLOR_ATTACHMENTS) };
    println!(
        "RGBA16F_MRT max_draw_buffers={max_draw_buffers} max_color_attachments={max_color_attachments} selected_attachments=4"
    );
    assert!(
        max_draw_buffers >= 4,
        "MRT probe requires four draw buffers"
    );
    assert!(
        max_color_attachments >= 4,
        "MRT probe requires four color attachments"
    );

    unsafe {
        let program = compile_probe_program(gl, vertex_source, fragment_source);
        let framebuffer = gl.create_framebuffer().expect("MRT FBO creates");
        let mut textures = Vec::with_capacity(4);
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
        for index in 0..4 {
            let texture = gl.create_texture().expect("MRT attachment texture creates");
            textures.push(texture);
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
            let (internal, format, data_type) = if index == 0 {
                (glow::RGBA16F as i32, glow::RGBA, glow::HALF_FLOAT)
            } else {
                (
                    glow::RGBA32UI as i32,
                    glow::RGBA_INTEGER,
                    glow::UNSIGNED_INT,
                )
            };
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                internal,
                width as i32,
                height as i32,
                0,
                format,
                data_type,
                glow::PixelUnpackData::Slice(None),
            );
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0 + index,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
        }
        let draw_buffers = [
            glow::COLOR_ATTACHMENT0,
            glow::COLOR_ATTACHMENT1,
            glow::COLOR_ATTACHMENT2,
            glow::COLOR_ATTACHMENT3,
        ];
        gl.draw_buffers(&draw_buffers);
        assert_eq!(
            gl.check_framebuffer_status(glow::FRAMEBUFFER),
            glow::FRAMEBUFFER_COMPLETE,
            "same-invocation MRT FBO must be complete"
        );

        let vertices: [f32; 24] = [
            -1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0, -1.0, -1.0, 0.0, 1.0,
            1.0, 1.0, 1.0, 0.0, -1.0, 1.0, 0.0, 0.0,
        ];
        let vertex_array = gl.create_vertex_array().expect("MRT VAO creates");
        let vertex_buffer = gl.create_buffer().expect("MRT quad buffer creates");
        let previous_dither = gl.is_enabled(glow::DITHER);
        gl.bind_vertex_array(Some(vertex_array));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
        gl.buffer_data_u8_slice(
            glow::ARRAY_BUFFER,
            bytemuck::cast_slice(&vertices),
            glow::STATIC_DRAW,
        );
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 4 * size_of::<f32>() as i32, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(
            1,
            2,
            glow::FLOAT,
            false,
            4 * size_of::<f32>() as i32,
            2 * size_of::<f32>() as i32,
        );
        gl.disable(glow::BLEND);
        gl.disable(glow::SCISSOR_TEST);
        if dither {
            gl.enable(glow::DITHER);
        } else {
            gl.disable(glow::DITHER);
        }
        gl.viewport(0, 0, width as i32, height as i32);
        gl.use_program(Some(program));
        if let Some(texture) = input_texture {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            if let Some(location) = gl.get_uniform_location(program, "u_effect_input") {
                gl.uniform_1_i32(Some(&location), 0);
            }
        }
        if let Some(location) = gl.get_uniform_location(program, "u_effect_target_flip_y") {
            gl.uniform_1_i32(Some(&location), 0);
        }
        set_uniforms(gl, program);
        gl.bind_vertex_array(Some(vertex_array));
        gl.draw_arrays(glow::TRIANGLES, 0, 6);
        gl.bind_vertex_array(None);
        if input_texture.is_some() {
            gl.bind_texture(glow::TEXTURE_2D, None);
        }

        let stored = read_float_attachment(gl, framebuffer, 0, width, height);
        let raw_bits = read_uint_attachment(gl, framebuffer, 1, width, height);
        let packed_half_bits = read_uint_attachment(gl, framebuffer, 2, width, height);
        let manual_half_bits = read_uint_attachment(gl, framebuffer, 3, width, height);

        gl.delete_buffer(vertex_buffer);
        gl.delete_vertex_array(vertex_array);
        gl.delete_program(program);
        gl.delete_framebuffer(framebuffer);
        for texture in textures {
            gl.delete_texture(texture);
        }
        if previous_dither {
            gl.enable(glow::DITHER);
        } else {
            gl.disable(glow::DITHER);
        }

        MrtProbeReadback {
            width,
            height,
            stored,
            raw_bits,
            packed_half_bits,
            manual_half_bits,
        }
    }
}

unsafe fn compile_probe_program(
    gl: &glow::Context,
    vertex_source: &str,
    fragment_source: &str,
) -> glow::Program {
    let vertex_shader =
        unsafe { gl.create_shader(glow::VERTEX_SHADER) }.expect("MRT vertex shader creates");
    unsafe {
        gl.shader_source(vertex_shader, vertex_source);
        gl.compile_shader(vertex_shader);
    }
    assert!(
        unsafe { gl.get_shader_compile_status(vertex_shader) },
        "MRT vertex shader compilation failed: {}",
        unsafe { gl.get_shader_info_log(vertex_shader) }
    );
    let fragment_shader =
        unsafe { gl.create_shader(glow::FRAGMENT_SHADER) }.expect("MRT fragment shader creates");
    unsafe {
        gl.shader_source(fragment_shader, fragment_source);
        gl.compile_shader(fragment_shader);
    }
    assert!(
        unsafe { gl.get_shader_compile_status(fragment_shader) },
        "MRT fragment shader compilation failed: {}",
        unsafe { gl.get_shader_info_log(fragment_shader) }
    );
    let program = unsafe { gl.create_program() }.expect("MRT program creates");
    unsafe {
        gl.attach_shader(program, vertex_shader);
        gl.attach_shader(program, fragment_shader);
        gl.link_program(program);
    }
    assert!(
        unsafe { gl.get_program_link_status(program) },
        "MRT program link failed: {}",
        unsafe { gl.get_program_info_log(program) }
    );
    unsafe {
        gl.delete_shader(vertex_shader);
        gl.delete_shader(fragment_shader);
    }
    program
}

unsafe fn read_float_attachment(
    gl: &glow::Context,
    framebuffer: glow::Framebuffer,
    attachment: u32,
    width: u32,
    height: u32,
) -> Vec<[f32; 4]> {
    let mut bytes = vec![0; width as usize * height as usize * 4 * size_of::<f32>()];
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
        gl.read_buffer(glow::COLOR_ATTACHMENT0 + attachment);
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(
            0,
            0,
            width as i32,
            height as i32,
            glow::RGBA,
            glow::FLOAT,
            glow::PixelPackData::Slice(Some(&mut bytes)),
        );
    }
    bytes
        .chunks_exact(4 * size_of::<f32>())
        .map(|pixel| {
            std::array::from_fn(|channel| {
                let start = channel * size_of::<f32>();
                f32::from_ne_bytes(pixel[start..start + 4].try_into().unwrap())
            })
        })
        .collect()
}

unsafe fn read_uint_attachment(
    gl: &glow::Context,
    framebuffer: glow::Framebuffer,
    attachment: u32,
    width: u32,
    height: u32,
) -> Vec<[u32; 4]> {
    let mut bytes = vec![0; width as usize * height as usize * 4 * size_of::<u32>()];
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
        gl.read_buffer(glow::COLOR_ATTACHMENT0 + attachment);
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(
            0,
            0,
            width as i32,
            height as i32,
            glow::RGBA_INTEGER,
            glow::UNSIGNED_INT,
            glow::PixelPackData::Slice(Some(&mut bytes)),
        );
    }
    bytes
        .chunks_exact(4 * size_of::<u32>())
        .map(|pixel| {
            std::array::from_fn(|channel| {
                let start = channel * size_of::<u32>();
                u32::from_ne_bytes(pixel[start..start + 4].try_into().unwrap())
            })
        })
        .collect()
}

#[cfg(test)]
mod cpu_reference_tests {
    use super::super::rgba16f_store::binary16_reference;

    #[test]
    fn binary16_reference_matches_terminal_diagnostic_samples() {
        let cases = [
            (0.111_809_48_f32, 0x2f27, 0x2f28, 0x2f28, 0x2f27),
            (0.186_779_71_f32, 0x31fa, 0x31fb, 0x31fa, 0x31fa),
            (0.158_193_42_f32, 0x310f, 0x3110, 0x3110, 0x310f),
        ];

        for (value, lower_bits, upper_bits, nearest_even_bits, toward_zero_bits) in cases {
            let reference = binary16_reference(value);
            assert_eq!(reference.lower_bits, lower_bits, "input={value}");
            assert_eq!(reference.upper_bits, upper_bits, "input={value}");
            assert_eq!(
                reference.nearest_even_bits, nearest_even_bits,
                "input={value}"
            );
            assert_eq!(
                reference.toward_zero_bits, toward_zero_bits,
                "input={value}"
            );
        }
    }

    #[test]
    fn binary16_reference_classifies_midpoints_and_exact_values() {
        let midpoint = 1.000_488_281_25_f32;
        let below = f32::from_bits(midpoint.to_bits() - 1);
        let above = f32::from_bits(midpoint.to_bits() + 1);

        let below_ref = binary16_reference(below);
        assert_eq!(
            (below_ref.lower_bits, below_ref.upper_bits),
            (0x3c00, 0x3c01)
        );
        assert_eq!(below_ref.nearest_even_bits, 0x3c00);
        assert_eq!(below_ref.toward_zero_bits, 0x3c00);

        let midpoint_ref = binary16_reference(midpoint);
        assert_eq!(
            (midpoint_ref.lower_bits, midpoint_ref.upper_bits),
            (0x3c00, 0x3c01)
        );
        assert_eq!(midpoint_ref.nearest_even_bits, 0x3c00);
        assert_eq!(midpoint_ref.toward_zero_bits, 0x3c00);

        let above_ref = binary16_reference(above);
        assert_eq!(
            (above_ref.lower_bits, above_ref.upper_bits),
            (0x3c00, 0x3c01)
        );
        assert_eq!(above_ref.nearest_even_bits, 0x3c01);
        assert_eq!(above_ref.toward_zero_bits, 0x3c00);

        let exact_ref = binary16_reference(0.125);
        assert_eq!(
            (exact_ref.lower_bits, exact_ref.upper_bits),
            (0x3000, 0x3000)
        );
        assert_eq!(exact_ref.nearest_even_bits, 0x3000);
        assert_eq!(exact_ref.toward_zero_bits, 0x3000);
    }
}

#[cfg(test)]
mod gles_microtest {
    use super::{binary16_reference, f32_to_binary16_nearest_even};
    use glow::HasContext;
    use std::collections::BTreeMap;

    use super::super::GlesEffectTestHarness;

    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 64;

    const PROBE_VERTEX_SHADER: &str = r#"#version 300 es
precision highp float;
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

    const CONSTANT_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform highp vec4 u_value;
layout(location = 0) out highp vec4 out_color;
void main() {
    out_color = u_value;
}
"#;

    fn compile_program(gl: &glow::Context, vertex: &str, fragment: &str) -> glow::Program {
        unsafe {
            let vertex_shader = gl
                .create_shader(glow::VERTEX_SHADER)
                .expect("probe vertex shader creates");
            gl.shader_source(vertex_shader, vertex);
            gl.compile_shader(vertex_shader);
            assert!(
                gl.get_shader_compile_status(vertex_shader),
                "probe vertex shader compilation failed: {}",
                gl.get_shader_info_log(vertex_shader)
            );

            let fragment_shader = gl
                .create_shader(glow::FRAGMENT_SHADER)
                .expect("probe fragment shader creates");
            gl.shader_source(fragment_shader, fragment);
            gl.compile_shader(fragment_shader);
            assert!(
                gl.get_shader_compile_status(fragment_shader),
                "probe fragment shader compilation failed: {}",
                gl.get_shader_info_log(fragment_shader)
            );

            let program = gl.create_program().expect("probe program creates");
            gl.attach_shader(program, vertex_shader);
            gl.attach_shader(program, fragment_shader);
            gl.link_program(program);
            assert!(
                gl.get_program_link_status(program),
                "probe program link failed: {}",
                gl.get_program_info_log(program)
            );
            gl.delete_shader(vertex_shader);
            gl.delete_shader(fragment_shader);
            program
        }
    }

    fn create_rgba16f_target(
        gl: &glow::Context,
        width: u32,
        height: u32,
    ) -> (glow::Texture, glow::Framebuffer) {
        unsafe {
            let texture = gl.create_texture().expect("RGBA16F texture creates");
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
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA16F as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::HALF_FLOAT,
                glow::PixelUnpackData::Slice(None),
            );

            let framebuffer = gl.create_framebuffer().expect("RGBA16F FBO creates");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
            assert_eq!(
                gl.check_framebuffer_status(glow::FRAMEBUFFER),
                glow::FRAMEBUFFER_COMPLETE,
                "RGBA16F microtest framebuffer must be complete"
            );
            (texture, framebuffer)
        }
    }

    fn draw_constant(
        gl: &glow::Context,
        framebuffer: glow::Framebuffer,
        program: glow::Program,
        vertex_array: glow::VertexArray,
        value: [f32; 4],
        dither: bool,
        viewport: (i32, i32, i32, i32),
    ) {
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.disable(glow::BLEND);
            gl.disable(glow::SCISSOR_TEST);
            if dither {
                gl.enable(glow::DITHER);
            } else {
                gl.disable(glow::DITHER);
            }
            gl.viewport(viewport.0, viewport.1, viewport.2, viewport.3);
            gl.use_program(Some(program));
            let location = gl
                .get_uniform_location(program, "u_value")
                .expect("constant value uniform exists");
            gl.uniform_4_f32(Some(&location), value[0], value[1], value[2], value[3]);
            gl.bind_vertex_array(Some(vertex_array));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
        }
    }

    fn read_rgba_f32(
        gl: &glow::Context,
        framebuffer: glow::Framebuffer,
        width: u32,
        height: u32,
    ) -> Vec<[f32; 4]> {
        let mut bytes = vec![0; width as usize * height as usize * 4 * size_of::<f32>()];
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
            gl.read_buffer(glow::COLOR_ATTACHMENT0);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(
                0,
                0,
                width as i32,
                height as i32,
                glow::RGBA,
                glow::FLOAT,
                glow::PixelPackData::Slice(Some(&mut bytes)),
            );
        }
        bytes
            .chunks_exact(4 * size_of::<f32>())
            .map(|pixel| {
                std::array::from_fn(|channel| {
                    let start = channel * size_of::<f32>();
                    f32::from_ne_bytes(pixel[start..start + 4].try_into().unwrap())
                })
            })
            .collect()
    }

    fn distribution(
        values: &[[f32; 4]],
        width: u32,
        channel: usize,
    ) -> Vec<(f32, usize, Vec<(u32, u32)>)> {
        let mut counts = BTreeMap::<u32, (usize, Vec<(u32, u32)>)>::new();
        for (index, pixel) in values.iter().enumerate() {
            let bits = pixel[channel].to_bits();
            let entry = counts.entry(bits).or_default();
            entry.0 += 1;
            if entry.1.len() < 8 {
                entry
                    .1
                    .push(((index as u32) % width, (index as u32) / width));
            }
        }
        counts
            .into_iter()
            .map(|(bits, (count, coordinates))| (f32::from_bits(bits), count, coordinates))
            .collect()
    }

    #[test]
    fn rgba16f_store_microtest_compares_explicit_dither_states() {
        let harness = GlesEffectTestHarness::new(WIDTH, HEIGHT);
        let gl = &harness.gl;
        let initial_dither = unsafe { gl.is_enabled(glow::DITHER) };
        println!("RGBA16F_DITHER initial_enabled={initial_dither}");
        assert!(
            initial_dither,
            "headless GLES context starts with DITHER enabled"
        );

        let program = compile_program(gl, PROBE_VERTEX_SHADER, CONSTANT_FRAGMENT_SHADER);
        let vertex_array = unsafe { gl.create_vertex_array().expect("probe VAO creates") };
        let (texture, framebuffer) = create_rgba16f_target(gl, WIDTH, HEIGHT);
        let midpoint = 1.000_488_281_25_f32;
        let inputs = [
            (
                "diagnostic_samples_and_exact_alpha",
                [0.111_809_48, 0.186_779_71, 0.158_193_42, 0.125],
            ),
            (
                "midpoint_neighbors_and_exact_value",
                [
                    f32::from_bits(midpoint.to_bits() - 1),
                    midpoint,
                    f32::from_bits(midpoint.to_bits() + 1),
                    0.125,
                ],
            ),
        ];
        let full_viewport = (0, 0, WIDTH as i32, HEIGHT as i32);
        let mut first_enabled = None;

        for (label, input) in inputs {
            draw_constant(
                gl,
                framebuffer,
                program,
                vertex_array,
                input,
                true,
                full_viewport,
            );
            let enabled = read_rgba_f32(gl, framebuffer, WIDTH, HEIGHT);
            draw_constant(
                gl,
                framebuffer,
                program,
                vertex_array,
                input,
                false,
                full_viewport,
            );
            let disabled = read_rgba_f32(gl, framebuffer, WIDTH, HEIGHT);
            let state_changed_channels = enabled
                .iter()
                .zip(&disabled)
                .map(|(on, off)| {
                    (0..4)
                        .filter(|channel| on[*channel].to_bits() != off[*channel].to_bits())
                        .count()
                })
                .sum::<usize>();
            println!("RGBA16F_DITHER input={label} value={input:?}");
            println!(
                "  enabled_vs_disabled changed_channels={state_changed_channels} total_channels={}",
                enabled.len() * 4
            );
            for channel in 0..4 {
                let reference = binary16_reference(input[channel]);
                let enabled_values = distribution(&enabled, WIDTH, channel);
                let disabled_values = distribution(&disabled, WIDTH, channel);
                println!(
                    "  channel={channel} lower={} upper={} nearest_even={} toward_zero={} enabled={enabled_values:?} disabled={disabled_values:?}",
                    super::binary16_to_f32(reference.lower_bits),
                    super::binary16_to_f32(reference.upper_bits),
                    super::binary16_to_f32(reference.nearest_even_bits),
                    super::binary16_to_f32(reference.toward_zero_bits),
                );
                assert!(enabled_values.iter().all(|(value, _, _)| {
                    f32_to_binary16_nearest_even(*value) == reference.nearest_even_bits
                        || value.to_bits() == super::binary16_to_f32(reference.lower_bits).to_bits()
                        || value.to_bits() == super::binary16_to_f32(reference.upper_bits).to_bits()
                }));
            }
            if label == "diagnostic_samples_and_exact_alpha" {
                first_enabled = Some(enabled);
            }
        }

        let first_input = inputs[0].1;
        draw_constant(
            gl,
            framebuffer,
            program,
            vertex_array,
            first_input,
            true,
            full_viewport,
        );
        let full = read_rgba_f32(gl, framebuffer, WIDTH, HEIGHT);
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        let offset = (9_i32, 7_i32);
        let translated_viewport = (
            offset.0,
            offset.1,
            WIDTH as i32 - offset.0,
            HEIGHT as i32 - offset.1,
        );
        draw_constant(
            gl,
            framebuffer,
            program,
            vertex_array,
            first_input,
            true,
            translated_viewport,
        );
        let translated = read_rgba_f32(gl, framebuffer, WIDTH, HEIGHT);
        let mut absolute_coordinate_changes = 0;
        let mut viewport_relative_changes = 0;
        for y in offset.1 as u32..HEIGHT {
            for x in offset.0 as u32..WIDTH {
                let index = (y * WIDTH + x) as usize;
                if full[index][0].to_bits() != translated[index][0].to_bits() {
                    absolute_coordinate_changes += 1;
                }
                let relative_index = ((y - offset.1 as u32) * WIDTH + x - offset.0 as u32) as usize;
                if full[relative_index][0].to_bits() != translated[index][0].to_bits() {
                    viewport_relative_changes += 1;
                }
            }
        }
        println!(
            "RGBA16F_DITHER viewport_shift offset={offset:?} absolute_coordinate_changes={absolute_coordinate_changes} viewport_relative_changes={viewport_relative_changes} first_enabled_pixels={:?}",
            first_enabled.map(|pixels| distribution(&pixels, WIDTH, 0))
        );

        unsafe {
            gl.delete_framebuffer(framebuffer);
            gl.delete_texture(texture);
            gl.delete_vertex_array(vertex_array);
            gl.delete_program(program);
        }
    }
}

#[cfg(test)]
mod mrt_microtest {
    use glow::HasContext;

    use super::super::GlesEffectTestHarness;
    use super::{
        MrtProbeReadback, binary16_reference, classify_mrt, f32_to_binary16_nearest_even,
        instrument_mrt_fragment_shader, report_mrt_samples, run_mrt_probe,
    };

    const CONSTANT_MRT_SOURCE: &str = r#"#version 300 es
precision highp float;
uniform highp vec4 u_value;
out vec4 out_color;
void main() {
    vec4 result = u_value;
    out_color = result;
}
"#;

    fn assert_constant_provenance(probe: &MrtProbeReadback, value: [f32; 4]) {
        for index in 0..probe.stored.len() {
            for channel in 0..4 {
                let raw_bits = probe.raw_bits[index][channel];
                assert_eq!(raw_bits, value[channel].to_bits(), "raw channel={channel}");
                let reference = binary16_reference(value[channel]);
                assert_eq!(
                    probe.manual_half_bits[index][channel] as u16, reference.toward_zero_bits,
                    "manual half channel={channel}"
                );
            }
        }
    }

    fn print_distribution(label: &str, probe: &MrtProbeReadback, channel: usize) {
        let mut distribution = std::collections::BTreeMap::<u16, (usize, Vec<(u32, u32)>)>::new();
        for (index, pixel) in probe.stored.iter().enumerate() {
            let bits = f32_to_binary16_nearest_even(pixel[channel]);
            let entry = distribution.entry(bits).or_default();
            entry.0 += 1;
            if entry.1.len() < 8 {
                entry
                    .1
                    .push((index as u32 % probe.width, index as u32 / probe.width));
            }
        }
        let expanded = distribution
            .into_iter()
            .map(|(bits, (count, coordinates))| (super::binary16_to_f32(bits), count, coordinates))
            .collect::<Vec<_>>();
        println!("{label} channel={channel} distribution={expanded:?}");
    }

    #[test]
    fn mrt_constant_probe_exposes_same_invocation_store_pack_and_manual_bits() {
        let harness = GlesEffectTestHarness::new(64, 64);
        let gl = &harness.gl;
        let base = instrument_mrt_fragment_shader(CONSTANT_MRT_SOURCE, "    out_color = result;");
        let midpoint = 1.000_488_281_25_f32;
        let cases = [
            (
                "representative_values",
                [0.111_809_48, 0.186_779_71, 0.158_193_42, 0.125],
            ),
            (
                "midpoint_neighbors",
                [
                    f32::from_bits(midpoint.to_bits() - 1),
                    midpoint,
                    f32::from_bits(midpoint.to_bits() + 1),
                    0.125,
                ],
            ),
        ];
        let initial_dither = unsafe { gl.is_enabled(glow::DITHER) };
        println!("RGBA16F_MRT initial_dither_enabled={initial_dither}");

        for (name, value) in cases {
            for dither in [true, false] {
                let probe = run_mrt_probe(
                    gl,
                    64,
                    64,
                    crate::egl_renderer::effects::DUAL_KAWASE_VERTEX_SHADER,
                    &base,
                    None,
                    dither,
                    |gl, program| unsafe {
                        if let Some(location) = gl.get_uniform_location(program, "u_value") {
                            gl.uniform_4_f32(
                                Some(&location),
                                value[0],
                                value[1],
                                value[2],
                                value[3],
                            );
                        }
                    },
                );
                assert_constant_provenance(&probe, value);
                let summary = classify_mrt(&probe);
                if !dither {
                    assert_eq!(
                        summary.store_matches_toward_zero, summary.total_channels,
                        "DITHER-off RGBA16F stores must round toward zero for case={name}"
                    );
                    assert_eq!(
                        summary.store_matches_manual_half, summary.total_channels,
                        "promoted manual RTZ bits must match DITHER-off RGBA16F stores for case={name}"
                    );
                }
                println!("RGBA16F_MRT case={name} dither={dither} classify={summary:?}");
                for channel in 0..4 {
                    print_distribution(&format!("case={name} dither={dither}"), &probe, channel);
                }
                report_mrt_samples(&format!("case={name} dither={dither}"), &probe, 3);
            }
        }

        unsafe {
            if initial_dither {
                gl.enable(glow::DITHER);
            } else {
                gl.disable(glow::DITHER);
            }
        }
    }
}
