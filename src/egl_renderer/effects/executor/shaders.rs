pub(crate) const COPY_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

void main() {
    out_color = texture(u_effect_input, typhon_effect_sample_uv(v_uv));
}
"#;

pub(crate) const NORMALIZE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform vec4 u_effect_input_domain;
uniform vec4 u_effect_output_domain;
uniform int u_effect_decode_srgb;
uniform int u_effect_encode_srgb;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

float typhon_decode_srgb_channel(float value);
float typhon_encode_srgb_channel(float value);
vec4 typhon_sanitize_premultiplied(vec4 value);

vec4 typhon_decode_premultiplied_srgb(vec4 value) {
    value = typhon_sanitize_premultiplied(value);
    if (value.a <= 0.00001) return vec4(0.0);
    vec3 straight = clamp(value.rgb / value.a, vec3(0.0), vec3(1.0));
    vec3 linear = vec3(typhon_decode_srgb_channel(straight.r), typhon_decode_srgb_channel(straight.g), typhon_decode_srgb_channel(straight.b));
    return typhon_sanitize_premultiplied(vec4(linear * value.a, value.a));
}

vec4 typhon_encode_premultiplied_srgb(vec4 value) {
    value = typhon_sanitize_premultiplied(value);
    if (value.a <= 0.00001) return vec4(0.0);
    vec3 straight = clamp(value.rgb / value.a, vec3(0.0), vec3(1.0));
    vec3 encoded = vec3(typhon_encode_srgb_channel(straight.r), typhon_encode_srgb_channel(straight.g), typhon_encode_srgb_channel(straight.b));
    return typhon_sanitize_premultiplied(vec4(encoded * value.a, value.a));
}

float typhon_decode_srgb_channel(float value) {
    value = max(value, 0.0);
    if (value <= 0.04045) return value / 12.92;
    return pow((value + 0.055) / 1.055, 2.4);
}

float typhon_encode_srgb_channel(float value) {
    value = max(value, 0.0);
    if (value <= 0.0031308) return value * 12.92;
    return 1.055 * pow(value, 1.0 / 2.4) - 0.055;
}

vec4 typhon_sanitize_premultiplied(vec4 value) {
    if (any(isnan(value)) || any(isinf(value))) return vec4(0.0);
    float alpha = clamp(value.a, 0.0, 1.0);
    return vec4(clamp(value.rgb, vec3(0.0), vec3(alpha)), alpha);
}

void main() {
    vec2 output_position = u_effect_output_domain.xy + v_uv * u_effect_output_domain.zw;
    vec2 logical_input_uv = (output_position - u_effect_input_domain.xy) /
        u_effect_input_domain.zw;
    if (any(lessThan(logical_input_uv, vec2(0.0))) || any(greaterThan(logical_input_uv, vec2(1.0)))) {
        out_color = vec4(0.0);
        return;
    }
    vec4 result = texture(u_effect_input, typhon_effect_sample_uv(logical_input_uv));
    if (u_effect_decode_srgb != 0) result = typhon_decode_premultiplied_srgb(result);
    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);
    out_color = typhon_sanitize_premultiplied(result);
}
"#;

pub(crate) const COMPOSITE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform float u_presentation_opacity;
uniform vec4 u_effect_input_domain;
uniform vec2 u_effect_output_size;
uniform int u_effect_encode_srgb;
uniform int u_effect_force_opaque;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

float typhon_encode_srgb_channel(float value) {
    value = max(value, 0.0);
    if (value <= 0.0031308) return value * 12.92;
    return 1.055 * pow(value, 1.0 / 2.4) - 0.055;
}

vec4 typhon_sanitize_premultiplied(vec4 value) {
    if (any(isnan(value)) || any(isinf(value))) return vec4(0.0);
    float alpha = clamp(value.a, 0.0, 1.0);
    return vec4(clamp(value.rgb, vec3(0.0), vec3(alpha)), alpha);
}

vec4 typhon_encode_premultiplied_srgb(vec4 value) {
    value = typhon_sanitize_premultiplied(value);
    if (value.a <= 0.00001) return vec4(0.0);
    vec3 straight = clamp(value.rgb / value.a, vec3(0.0), vec3(1.0));
    vec3 encoded = vec3(typhon_encode_srgb_channel(straight.r), typhon_encode_srgb_channel(straight.g), typhon_encode_srgb_channel(straight.b));
    return typhon_sanitize_premultiplied(vec4(encoded * value.a, value.a));
}

void main() {
    vec2 output_position = v_uv * u_effect_output_size;
    vec2 logical_input_uv = (output_position - u_effect_input_domain.xy) /
        u_effect_input_domain.zw;
    if (any(lessThan(logical_input_uv, vec2(0.0))) || any(greaterThan(logical_input_uv, vec2(1.0)))) {
        discard;
    }
    vec4 result = texture(u_effect_input, typhon_effect_sample_uv(logical_input_uv));
    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);
    if (u_effect_force_opaque != 0) result.a = 1.0;
    result *= clamp(u_presentation_opacity, 0.0, 1.0);
    out_color = typhon_sanitize_premultiplied(result);
}
"#;

pub(crate) const FRAGMENT_STAGE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform int u_effect_decode_srgb;
uniform int u_effect_encode_srgb;
uniform mat4 u_effect_color_matrix;
uniform vec4 u_effect_color_bias;
uniform vec4 u_effect_tint_color;
uniform float u_effect_tint_amount;
uniform float u_effect_noise_amount;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

float typhon_decode_srgb_channel(float value);
float typhon_encode_srgb_channel(float value);
vec4 typhon_sanitize_premultiplied(vec4 value);

float typhon_decode_srgb_channel(float value) {
    value = max(value, 0.0);
    if (value <= 0.04045) return value / 12.92;
    return pow((value + 0.055) / 1.055, 2.4);
}

float typhon_encode_srgb_channel(float value) {
    value = max(value, 0.0);
    if (value <= 0.0031308) return value * 12.92;
    return 1.055 * pow(value, 1.0 / 2.4) - 0.055;
}

vec4 typhon_sanitize_premultiplied(vec4 value) {
    if (any(isnan(value)) || any(isinf(value))) return vec4(0.0);
    float alpha = clamp(value.a, 0.0, 1.0);
    return vec4(clamp(value.rgb, vec3(0.0), vec3(alpha)), alpha);
}

vec4 typhon_decode_premultiplied_srgb(vec4 value) {
    value = typhon_sanitize_premultiplied(value);
    if (value.a <= 0.00001) return vec4(0.0);
    vec3 straight = clamp(value.rgb / value.a, vec3(0.0), vec3(1.0));
    vec3 linear = vec3(typhon_decode_srgb_channel(straight.r), typhon_decode_srgb_channel(straight.g), typhon_decode_srgb_channel(straight.b));
    return typhon_sanitize_premultiplied(vec4(linear * value.a, value.a));
}

vec4 typhon_encode_premultiplied_srgb(vec4 value) {
    value = typhon_sanitize_premultiplied(value);
    if (value.a <= 0.00001) return vec4(0.0);
    vec3 straight = clamp(value.rgb / value.a, vec3(0.0), vec3(1.0));
    vec3 encoded = vec3(typhon_encode_srgb_channel(straight.r), typhon_encode_srgb_channel(straight.g), typhon_encode_srgb_channel(straight.b));
    return typhon_sanitize_premultiplied(vec4(encoded * value.a, value.a));
}

void main() {
    vec4 result = texture(u_effect_input, typhon_effect_sample_uv(v_uv));
    if (u_effect_decode_srgb != 0) result = typhon_decode_premultiplied_srgb(result);
    result = u_effect_color_matrix * result + u_effect_color_bias;
    result.rgb = mix(result.rgb, result.rgb * u_effect_tint_color.rgb, clamp(u_effect_tint_amount, 0.0, 1.0));
    float noise = fract(sin(dot(v_uv, vec2(12.9898, 78.233))) * 43758.5453) - 0.5;
    result.rgb += noise * u_effect_noise_amount;
    result = typhon_sanitize_premultiplied(result);
    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);
    out_color = typhon_sanitize_premultiplied(result);
}
"#;

pub(crate) const MASK_STAGE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform int u_effect_decode_srgb;
uniform int u_effect_encode_srgb;
uniform int u_effect_inverted;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

float typhon_decode_srgb_channel(float value);
float typhon_encode_srgb_channel(float value);
vec4 typhon_sanitize_premultiplied(vec4 value);

float typhon_decode_srgb_channel(float value) {
    value = max(value, 0.0);
    if (value <= 0.04045) return value / 12.92;
    return pow((value + 0.055) / 1.055, 2.4);
}

float typhon_encode_srgb_channel(float value) {
    value = max(value, 0.0);
    if (value <= 0.0031308) return value * 12.92;
    return 1.055 * pow(value, 1.0 / 2.4) - 0.055;
}

vec4 typhon_sanitize_premultiplied(vec4 value) {
    if (any(isnan(value)) || any(isinf(value))) return vec4(0.0);
    float alpha = clamp(value.a, 0.0, 1.0);
    return vec4(clamp(value.rgb, vec3(0.0), vec3(alpha)), alpha);
}

vec4 typhon_decode_premultiplied_srgb(vec4 value) {
    value = typhon_sanitize_premultiplied(value);
    if (value.a <= 0.00001) return vec4(0.0);
    vec3 straight = clamp(value.rgb / value.a, vec3(0.0), vec3(1.0));
    vec3 linear = vec3(typhon_decode_srgb_channel(straight.r), typhon_decode_srgb_channel(straight.g), typhon_decode_srgb_channel(straight.b));
    return typhon_sanitize_premultiplied(vec4(linear * value.a, value.a));
}

vec4 typhon_encode_premultiplied_srgb(vec4 value) {
    value = typhon_sanitize_premultiplied(value);
    if (value.a <= 0.00001) return vec4(0.0);
    vec3 straight = clamp(value.rgb / value.a, vec3(0.0), vec3(1.0));
    vec3 encoded = vec3(typhon_encode_srgb_channel(straight.r), typhon_encode_srgb_channel(straight.g), typhon_encode_srgb_channel(straight.b));
    return typhon_sanitize_premultiplied(vec4(encoded * value.a, value.a));
}

void main() {
    vec4 result = texture(u_effect_input, typhon_effect_sample_uv(v_uv));
    if (u_effect_decode_srgb != 0) result = typhon_decode_premultiplied_srgb(result);
    float coverage = u_effect_inverted != 0 ? 1.0 - result.a : result.a;
    result *= clamp(coverage, 0.0, 1.0);
    result = typhon_sanitize_premultiplied(result);
    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);
    out_color = typhon_sanitize_premultiplied(result);
}
"#;

pub(crate) const BLEND_STAGE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform sampler2D u_effect_input_secondary;
uniform int u_effect_decode_srgb;
uniform int u_effect_encode_srgb;
uniform int u_effect_blend_mode;
uniform float u_effect_blend_opacity;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

float typhon_decode_srgb_channel(float value);
float typhon_encode_srgb_channel(float value);
vec4 typhon_sanitize_premultiplied(vec4 value);

float typhon_decode_srgb_channel(float value) {
    value = max(value, 0.0);
    if (value <= 0.04045) return value / 12.92;
    return pow((value + 0.055) / 1.055, 2.4);
}

float typhon_encode_srgb_channel(float value) {
    value = max(value, 0.0);
    if (value <= 0.0031308) return value * 12.92;
    return 1.055 * pow(value, 1.0 / 2.4) - 0.055;
}

vec4 typhon_sanitize_premultiplied(vec4 value) {
    if (any(isnan(value)) || any(isinf(value))) return vec4(0.0);
    float alpha = clamp(value.a, 0.0, 1.0);
    return vec4(clamp(value.rgb, vec3(0.0), vec3(alpha)), alpha);
}

vec4 typhon_decode_premultiplied_srgb(vec4 value) {
    value = typhon_sanitize_premultiplied(value);
    if (value.a <= 0.00001) return vec4(0.0);
    vec3 straight = clamp(value.rgb / value.a, vec3(0.0), vec3(1.0));
    vec3 linear = vec3(typhon_decode_srgb_channel(straight.r), typhon_decode_srgb_channel(straight.g), typhon_decode_srgb_channel(straight.b));
    return typhon_sanitize_premultiplied(vec4(linear * value.a, value.a));
}

vec4 typhon_encode_premultiplied_srgb(vec4 value) {
    value = typhon_sanitize_premultiplied(value);
    if (value.a <= 0.00001) return vec4(0.0);
    vec3 straight = clamp(value.rgb / value.a, vec3(0.0), vec3(1.0));
    vec3 encoded = vec3(typhon_encode_srgb_channel(straight.r), typhon_encode_srgb_channel(straight.g), typhon_encode_srgb_channel(straight.b));
    return typhon_sanitize_premultiplied(vec4(encoded * value.a, value.a));
}

void main() {
    vec2 sample_uv = typhon_effect_sample_uv(v_uv);
    vec4 first = texture(u_effect_input, sample_uv);
    vec4 second = texture(u_effect_input_secondary, sample_uv);
    if (u_effect_decode_srgb != 0) {
        first = typhon_decode_premultiplied_srgb(first);
        second = typhon_decode_premultiplied_srgb(second);
    }
    float opacity = clamp(u_effect_blend_opacity, 0.0, 1.0);
    float second_alpha = clamp(second.a * opacity, 0.0, 1.0);
    vec3 second_premultiplied = second.rgb * opacity;
    vec3 blended = second.rgb;
    if (u_effect_blend_mode == 1) {
        blended = first.rgb + second.rgb;
    } else if (u_effect_blend_mode == 2 || u_effect_blend_mode == 3) {
        vec3 first_straight = first.a > 0.00001 ? first.rgb / first.a : vec3(0.0);
        vec3 second_straight = second.a > 0.00001 ? second.rgb / second.a : vec3(0.0);
        blended = u_effect_blend_mode == 2
            ? first_straight * second_straight
            : 1.0 - (1.0 - first_straight) * (1.0 - second_straight);
    }
    vec3 rgb = u_effect_blend_mode == 0
        ? second.rgb * opacity + first.rgb * (1.0 - second_alpha)
        : u_effect_blend_mode == 1
            ? first.rgb + second.rgb * opacity
            : first.rgb * (1.0 - second_alpha) + second_premultiplied * (1.0 - first.a) + blended * first.a * second_alpha;
    float alpha = second_alpha + first.a * (1.0 - second_alpha);
    vec4 result = typhon_sanitize_premultiplied(vec4(rgb, alpha));
    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);
    out_color = typhon_sanitize_premultiplied(result);
}
"#;
