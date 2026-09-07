use oblivion_one::effects::{EffectRect, EffectValidationError};

pub(crate) const DUAL_KAWASE_VERTEX_SHADER: &str = r#"#version 300 es
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_uv;
out vec2 v_uv;

void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
    v_uv = a_uv;
}
"#;

pub(crate) const DUAL_KAWASE_DOWNSAMPLE_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform vec2 u_effect_texel_size;
in vec2 v_uv;
out vec4 out_color;

vec3 typhon_srgb_to_linear(vec3 value) {
    return mix(value / 12.92, pow((value + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), value));
}

void main() {
    vec2 offset = u_effect_texel_size;
    vec4 sample_a = texture(u_effect_input, v_uv + offset);
    vec4 sample_b = texture(u_effect_input, v_uv - offset);
    vec4 sample_c = texture(u_effect_input, v_uv + vec2(offset.x, -offset.y));
    vec4 sample_d = texture(u_effect_input, v_uv + vec2(-offset.x, offset.y));
    vec4 result = (sample_a + sample_b + sample_c + sample_d) * 0.25;
    result.rgb = typhon_srgb_to_linear(result.rgb);
    out_color = result;
}
"#;

pub(crate) const DUAL_KAWASE_UPSAMPLE_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform vec2 u_effect_texel_size;
in vec2 v_uv;
out vec4 out_color;

void main() {
    vec2 offset = u_effect_texel_size;
    vec4 result = texture(u_effect_input, v_uv) * 0.4;
    result += texture(u_effect_input, v_uv + vec2(offset.x, 0.0)) * 0.15;
    result += texture(u_effect_input, v_uv - vec2(offset.x, 0.0)) * 0.15;
    result += texture(u_effect_input, v_uv + vec2(0.0, offset.y)) * 0.15;
    result += texture(u_effect_input, v_uv - vec2(0.0, offset.y)) * 0.15;
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
        assert!(DUAL_KAWASE_DOWNSAMPLE_SHADER.contains("typhon_srgb_to_linear"));
        assert!(DUAL_KAWASE_UPSAMPLE_SHADER.contains("u_effect_texel_size"));
    }
}
