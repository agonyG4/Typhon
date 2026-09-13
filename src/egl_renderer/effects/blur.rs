use oblivion_one::effects::{EffectRect, EffectValidationError};

pub(crate) const DUAL_KAWASE_VERTEX_SHADER: &str = r#"#version 300 es
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_uv;
uniform int u_effect_target_flip_y;
out vec2 v_uv;

void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
    v_uv = u_effect_target_flip_y != 0 ? vec2(a_uv.x, 1.0 - a_uv.y) : a_uv;
}
"#;

pub(crate) const DUAL_KAWASE_DOWNSAMPLE_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform vec2 u_effect_texel_size;
uniform float u_effect_blur_radius;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

vec4 typhon_decode_premultiplied_srgb(vec4 value) {
    if (any(isnan(value)) || any(isinf(value))) return vec4(0.0);
    value.a = clamp(value.a, 0.0, 1.0);
    value.rgb = clamp(value.rgb, vec3(0.0), vec3(value.a));
    if (value.a <= 0.00001) return vec4(0.0);
    vec3 straight = clamp(value.rgb / value.a, vec3(0.0), vec3(1.0));
    vec3 linear = vec3(
        straight.r <= 0.04045 ? straight.r / 12.92 : pow((straight.r + 0.055) / 1.055, 2.4),
        straight.g <= 0.04045 ? straight.g / 12.92 : pow((straight.g + 0.055) / 1.055, 2.4),
        straight.b <= 0.04045 ? straight.b / 12.92 : pow((straight.b + 0.055) / 1.055, 2.4)
    );
    return vec4(clamp(linear * value.a, vec3(0.0), vec3(value.a)), value.a);
}

void main() {
    vec2 offset = u_effect_texel_size * u_effect_blur_radius;
    vec2 sample_uv = typhon_effect_sample_uv(v_uv);
    vec4 sample_a = texture(u_effect_input, sample_uv + offset);
    vec4 sample_b = texture(u_effect_input, sample_uv - offset);
    vec4 sample_c = texture(u_effect_input, sample_uv + vec2(offset.x, -offset.y));
    vec4 sample_d = texture(u_effect_input, sample_uv + vec2(-offset.x, offset.y));
    vec4 result = (
        typhon_decode_premultiplied_srgb(sample_a) +
        typhon_decode_premultiplied_srgb(sample_b) +
        typhon_decode_premultiplied_srgb(sample_c) +
        typhon_decode_premultiplied_srgb(sample_d)
    ) * 0.25;
    if (any(isnan(result)) || any(isinf(result))) result = vec4(0.0);
    result.a = clamp(result.a, 0.0, 1.0);
    result.rgb = clamp(result.rgb, vec3(0.0), vec3(result.a));
    out_color = result;
}
"#;

pub(crate) const DUAL_KAWASE_DOWNSAMPLE_LINEAR_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform vec2 u_effect_texel_size;
uniform float u_effect_blur_radius;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

void main() {
    vec2 offset = u_effect_texel_size * u_effect_blur_radius;
    vec2 sample_uv = typhon_effect_sample_uv(v_uv);
    vec4 sample_a = texture(u_effect_input, sample_uv + offset);
    vec4 sample_b = texture(u_effect_input, sample_uv - offset);
    vec4 sample_c = texture(u_effect_input, sample_uv + vec2(offset.x, -offset.y));
    vec4 sample_d = texture(u_effect_input, sample_uv + vec2(-offset.x, offset.y));
    out_color = (sample_a + sample_b + sample_c + sample_d) * 0.25;
}
"#;

pub(crate) const DUAL_KAWASE_UPSAMPLE_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform vec2 u_effect_texel_size;
uniform float u_effect_blur_radius;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

void main() {
    vec2 offset = u_effect_texel_size * u_effect_blur_radius;
    vec2 sample_uv = typhon_effect_sample_uv(v_uv);
    vec4 result = texture(u_effect_input, sample_uv) * 0.4;
    result += texture(u_effect_input, sample_uv + vec2(offset.x, 0.0)) * 0.15;
    result += texture(u_effect_input, sample_uv - vec2(offset.x, 0.0)) * 0.15;
    result += texture(u_effect_input, sample_uv + vec2(0.0, offset.y)) * 0.15;
    result += texture(u_effect_input, sample_uv - vec2(0.0, offset.y)) * 0.15;
    out_color = result;
}
"#;

#[allow(dead_code)] // The graph compiler owns dimensions; this guards future lowering callers.
pub(crate) fn scaled_blur_dimensions(
    bounds: EffectRect,
    scale: f32,
) -> Result<(u32, u32), EffectValidationError> {
    if !scale.is_finite() || !(0.0625..=1.0).contains(&scale) {
        return Err(EffectValidationError::InvalidScale);
    }
    let width = (f64::from(bounds.width) * f64::from(scale)).ceil();
    let height = (f64::from(bounds.height) * f64::from(scale)).ceil();
    if width < 1.0 || height < 1.0 {
        return Ok((1, 1));
    }
    Ok((width as u32, height as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_blur_dimensions_never_drop_below_one_pixel() {
        assert_eq!(
            scaled_blur_dimensions(EffectRect::new(0, 0, 1, 1).unwrap(), 0.0625).unwrap(),
            (1, 1)
        );
    }

    #[test]
    fn built_in_blur_shaders_define_srgb_conversion_boundaries() {
        assert!(
            DUAL_KAWASE_DOWNSAMPLE_SHADER.contains("typhon_decode_premultiplied_srgb(sample_a)")
        );
        assert!(
            DUAL_KAWASE_DOWNSAMPLE_SHADER.contains("typhon_decode_premultiplied_srgb(sample_d)")
        );
        assert!(DUAL_KAWASE_UPSAMPLE_SHADER.contains("u_effect_texel_size"));
        assert!(DUAL_KAWASE_DOWNSAMPLE_SHADER.contains("u_effect_blur_radius"));
        assert!(DUAL_KAWASE_DOWNSAMPLE_LINEAR_SHADER.contains("u_effect_blur_radius"));
        assert!(!DUAL_KAWASE_DOWNSAMPLE_LINEAR_SHADER.contains("typhon_srgb_to_linear"));
    }

    #[test]
    fn effect_vertex_shader_uses_explicit_y_flip_semantics() {
        assert!(DUAL_KAWASE_VERTEX_SHADER.contains("uniform int u_effect_target_flip_y;"));
        assert!(
            DUAL_KAWASE_VERTEX_SHADER
                .contains("u_effect_target_flip_y != 0 ? vec2(a_uv.x, 1.0 - a_uv.y) : a_uv")
        );
        assert!(!DUAL_KAWASE_VERTEX_SHADER.contains("u_effect_flip_y"));
    }

    #[test]
    fn blur_fragment_shaders_define_explicit_input_sample_conversion() {
        for shader in [
            DUAL_KAWASE_DOWNSAMPLE_SHADER,
            DUAL_KAWASE_DOWNSAMPLE_LINEAR_SHADER,
            DUAL_KAWASE_UPSAMPLE_SHADER,
        ] {
            assert!(shader.contains("uniform int u_effect_input_flip_y;"));
            assert!(shader.contains("vec2 typhon_effect_sample_uv(vec2 logical_uv)"));
            assert!(shader.contains("typhon_effect_sample_uv(v_uv)"));
        }
    }
}
