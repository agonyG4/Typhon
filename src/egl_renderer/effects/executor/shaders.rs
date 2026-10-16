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
uniform int u_effect_coverage_enabled;
uniform int u_effect_coverage_has_rounded_rect;
uniform vec4 u_effect_coverage_rounded_rect;
uniform float u_effect_coverage_radius;
uniform int u_effect_coverage_has_triangle;
uniform vec2 u_effect_coverage_triangle_a;
uniform vec2 u_effect_coverage_triangle_b;
uniform vec2 u_effect_coverage_triangle_c;
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

float typhon_rounded_rect_distance(vec2 p) {
    vec2 size = u_effect_coverage_rounded_rect.zw;
    float radius = max(u_effect_coverage_radius, 0.0);
    vec2 half_size = size * 0.5;
    vec2 center = u_effect_coverage_rounded_rect.xy + half_size;
    vec2 q = abs(p - center) - (half_size - vec2(radius));
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

float typhon_cross(vec2 a, vec2 b) {
    return a.x * b.y - a.y * b.x;
}

float typhon_segment_distance(vec2 p, vec2 a, vec2 b) {
    vec2 edge = b - a;
    float length_squared = dot(edge, edge);
    float t = clamp(dot(p - a, edge) / max(length_squared, 1.0e-8), 0.0, 1.0);
    return length(p - (a + t * edge));
}

float typhon_triangle_distance(vec2 p) {
    vec2 a = u_effect_coverage_triangle_a;
    vec2 b = u_effect_coverage_triangle_b;
    vec2 c = u_effect_coverage_triangle_c;
    float area = typhon_cross(b - a, c - a);
    if (isnan(area) || isinf(area) || abs(area) <= 1.0e-6) return 1.0e20;
    vec3 side = vec3(
        typhon_cross(b - a, p - a),
        typhon_cross(c - b, p - b),
        typhon_cross(a - c, p - c)
    );
    bool inside = all(greaterThanEqual(side, vec3(0.0))) ||
        all(lessThanEqual(side, vec3(0.0)));
    float distance = min(
        typhon_segment_distance(p, a, b),
        min(typhon_segment_distance(p, b, c), typhon_segment_distance(p, c, a))
    );
    return inside ? -distance : distance;
}

float typhon_analytic_coverage(vec2 p) {
    if (u_effect_coverage_enabled == 0) return 1.0;
    float distance = 1.0e20;
    if (u_effect_coverage_has_rounded_rect != 0) {
        distance = min(distance, typhon_rounded_rect_distance(p));
    }
    if (u_effect_coverage_has_triangle != 0) {
        distance = min(distance, typhon_triangle_distance(p));
    }
    if (isnan(distance) || isinf(distance)) return 0.0;
    float aa_width = max(fwidth(distance), 1.0e-4);
    return clamp(1.0 - smoothstep(-aa_width, aa_width, distance), 0.0, 1.0);
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
    result *= typhon_analytic_coverage(output_position);
    out_color = typhon_sanitize_premultiplied(result);
}
"#;

pub(crate) const TERMINAL_KAWASE_COMPOSITE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform float u_presentation_opacity;
uniform vec4 u_effect_input_domain;
uniform vec2 u_effect_output_size;
uniform vec2 u_effect_texel_size;
uniform float u_effect_blur_radius;
uniform int u_effect_encode_srgb;
uniform int u_effect_force_opaque;
uniform int u_effect_input_flip_y;
uniform int u_effect_coverage_enabled;
uniform int u_effect_coverage_has_rounded_rect;
uniform vec4 u_effect_coverage_rounded_rect;
uniform float u_effect_coverage_radius;
uniform int u_effect_coverage_has_triangle;
uniform vec2 u_effect_coverage_triangle_a;
uniform vec2 u_effect_coverage_triangle_b;
uniform vec2 u_effect_coverage_triangle_c;
uniform mat4 u_effect_color_matrix;
uniform vec4 u_effect_color_bias;
uniform vec4 u_effect_tint_color;
uniform float u_effect_tint_amount;
uniform float u_effect_noise_amount;
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

float typhon_rounded_rect_distance(vec2 p) {
    vec2 size = u_effect_coverage_rounded_rect.zw;
    float radius = max(u_effect_coverage_radius, 0.0);
    vec2 half_size = size * 0.5;
    vec2 center = u_effect_coverage_rounded_rect.xy + half_size;
    vec2 q = abs(p - center) - (half_size - vec2(radius));
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

float typhon_cross(vec2 a, vec2 b) { return a.x * b.y - a.y * b.x; }

float typhon_segment_distance(vec2 p, vec2 a, vec2 b) {
    vec2 edge = b - a;
    float length_squared = dot(edge, edge);
    float t = clamp(dot(p - a, edge) / max(length_squared, 1.0e-8), 0.0, 1.0);
    return length(p - (a + t * edge));
}

float typhon_triangle_distance(vec2 p) {
    vec2 a = u_effect_coverage_triangle_a;
    vec2 b = u_effect_coverage_triangle_b;
    vec2 c = u_effect_coverage_triangle_c;
    float area = typhon_cross(b - a, c - a);
    if (isnan(area) || isinf(area) || abs(area) <= 1.0e-6) return 1.0e20;
    vec3 side = vec3(typhon_cross(b - a, p - a), typhon_cross(c - b, p - b), typhon_cross(a - c, p - c));
    bool inside = all(greaterThanEqual(side, vec3(0.0))) || all(lessThanEqual(side, vec3(0.0)));
    float distance = min(typhon_segment_distance(p, a, b), min(typhon_segment_distance(p, b, c), typhon_segment_distance(p, c, a)));
    return inside ? -distance : distance;
}

float typhon_analytic_coverage(vec2 p) {
    if (u_effect_coverage_enabled == 0) return 1.0;
    float distance = 1.0e20;
    if (u_effect_coverage_has_rounded_rect != 0) distance = min(distance, typhon_rounded_rect_distance(p));
    if (u_effect_coverage_has_triangle != 0) distance = min(distance, typhon_triangle_distance(p));
    if (isnan(distance) || isinf(distance)) return 0.0;
    float aa_width = max(fwidth(distance), 1.0e-4);
    return clamp(1.0 - smoothstep(-aa_width, aa_width, distance), 0.0, 1.0);
}

void main() {
    vec2 output_position = v_uv * u_effect_output_size;
    vec2 logical_input_uv = (output_position - u_effect_input_domain.xy) / u_effect_input_domain.zw;
    if (any(lessThan(logical_input_uv, vec2(0.0))) || any(greaterThan(logical_input_uv, vec2(1.0)))) discard;

    // Reconstruct the old full-resolution pass's local pixel-center UV. The
    // previous formula is equivalent algebraically, but its extra viewport
    // mapping rounds differently before filtered sampling and the noise hash.
    vec2 effect_local_pixel = floor(output_position - u_effect_input_domain.xy);
    vec2 effect_local_uv =
        (effect_local_pixel + vec2(0.5)) / u_effect_input_domain.zw;

    // Match the legacy final Dual Kawase upsample in retained-input texels.
    vec2 sample_uv = typhon_effect_sample_uv(effect_local_uv);
    vec2 offset = u_effect_texel_size * u_effect_blur_radius;
    vec4 result = texture(u_effect_input, sample_uv) * 0.40;
    result += texture(u_effect_input, sample_uv + vec2(offset.x, 0.0)) * 0.15;
    result += texture(u_effect_input, sample_uv - vec2(offset.x, 0.0)) * 0.15;
    result += texture(u_effect_input, sample_uv + vec2(0.0, offset.y)) * 0.15;
    result += texture(u_effect_input, sample_uv - vec2(0.0, offset.y)) * 0.15;
    result = u_effect_color_matrix * result + u_effect_color_bias;
    result.rgb = mix(result.rgb, result.rgb * u_effect_tint_color.rgb, clamp(u_effect_tint_amount, 0.0, 1.0));
    float noise = fract(sin(dot(effect_local_uv, vec2(12.9898, 78.233))) * 43758.5453) - 0.5;
    result.rgb += noise * u_effect_noise_amount;
    result = typhon_sanitize_premultiplied(result);
    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);
    if (u_effect_force_opaque != 0) result.a = 1.0;
    result *= clamp(u_presentation_opacity, 0.0, 1.0);
    result *= typhon_analytic_coverage(output_position);
    out_color = typhon_sanitize_premultiplied(result);
}
"#;

macro_rules! canonical_effect_coordinates_glsl {
    () => {
        r#"vec2 typhon_canonical_effect_local_pixel() {
    ivec2 storage_pixel = ivec2(floor(gl_FragCoord.xy));
    int logical_y = u_effect_target_flip_y != 0
        ? storage_pixel.y
        : int(u_effect_output_size.y) - 1 - storage_pixel.y;
    vec2 logical_pixel = vec2(float(storage_pixel.x), float(logical_y)) +
        u_effect_output_domain.xy;
    return logical_pixel - u_effect_input_domain.xy;
}

vec2 typhon_canonical_effect_local_uv() {
    return (typhon_canonical_effect_local_pixel() + vec2(0.5)) /
        u_effect_input_domain.zw;
}

float typhon_canonical_effect_noise() {
    vec2 canonical_noise_uv = typhon_canonical_effect_local_uv();
    return fract(
        sin(dot(canonical_noise_uv, vec2(12.9898, 78.233))) * 43758.5453
    ) - 0.5;
}"#
    };
}

#[cfg(test)]
pub(crate) const CANONICAL_EFFECT_COORDINATES: &str = canonical_effect_coordinates_glsl!();

macro_rules! rgba16f_rtz_glsl {
    () => {
        r#"precision highp int;
uint typhon_float_to_binary16_rtz(float value) {
    uint bits = floatBitsToUint(value);
    uint sign = (bits >> 16u) & 0x8000u;
    uint exponent = (bits >> 23u) & 0xffu;
    uint fraction = bits & 0x007fffffu;
    if (exponent == 0xffu) {
        if (fraction == 0u) return sign | 0x7c00u;
        return sign | 0x7c00u | max(fraction >> 13u, 1u);
    }
    int unbiased = int(exponent) - 127;
    if (unbiased > 15) return sign | 0x7bffu;
    if (unbiased >= -14) {
        uint half_exponent = uint(unbiased + 15);
        return sign | (half_exponent << 10u) | (fraction >> 13u);
    }
    if (unbiased < -24) return sign;
    uint significand = 0x800000u | fraction;
    uint shift = uint(-unbiased - 1);
    return sign | (significand >> shift);
}

float typhon_binary16_to_float(uint half_bits) {
    uint sign = (half_bits & 0x8000u) << 16u;
    uint exponent = (half_bits >> 10u) & 0x1fu;
    uint fraction = half_bits & 0x03ffu;
    if (exponent == 0u) {
        if (fraction == 0u) return uintBitsToFloat(sign);
        int unbiased = -14;
        while ((fraction & 0x0400u) == 0u) {
            fraction <<= 1u;
            unbiased -= 1;
        }
        fraction &= 0x03ffu;
        return uintBitsToFloat(
            sign | (uint(unbiased + 127) << 23u) | (fraction << 13u)
        );
    }
    if (exponent == 0x1fu) {
        return uintBitsToFloat(sign | 0x7f800000u | (fraction << 13u));
    }
    return uintBitsToFloat(
        sign | ((exponent + 112u) << 23u) | (fraction << 13u)
    );
}"#
    };
}

macro_rules! virtual_rgba16f_rtz_glsl {
    () => {
        r#"vec4 typhon_virtual_rgba16f_rtz(vec4 value) {
    return vec4(
        typhon_binary16_to_float(typhon_float_to_binary16_rtz(value.r)),
        typhon_binary16_to_float(typhon_float_to_binary16_rtz(value.g)),
        typhon_binary16_to_float(typhon_float_to_binary16_rtz(value.b)),
        typhon_binary16_to_float(typhon_float_to_binary16_rtz(value.a))
    );
}"#
    };
}

#[cfg(test)]
pub(crate) const RGBA16F_RTZ_GLSL: &str = rgba16f_rtz_glsl!();
#[cfg(test)]
pub(crate) const VIRTUAL_RGBA16F_RTZ_GLSL: &str = virtual_rgba16f_rtz_glsl!();

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

pub(crate) const FRAGMENT_STAGE_FRAGMENT_CANONICAL_SHADER: &str = concat!(
    r#"#version 300 es
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
uniform highp int u_effect_target_flip_y;
uniform vec2 u_effect_output_size;
uniform vec4 u_effect_input_domain;
uniform vec4 u_effect_output_domain;
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

"#,
    canonical_effect_coordinates_glsl!(),
    r#"

void main() {
    vec4 result = texture(u_effect_input, typhon_effect_sample_uv(v_uv));
    if (u_effect_decode_srgb != 0) result = typhon_decode_premultiplied_srgb(result);
    result = u_effect_color_matrix * result + u_effect_color_bias;
    result.rgb = mix(result.rgb, result.rgb * u_effect_tint_color.rgb, clamp(u_effect_tint_amount, 0.0, 1.0));
    float noise = typhon_canonical_effect_noise();
    result.rgb += noise * u_effect_noise_amount;
    result = typhon_sanitize_premultiplied(result);
    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);
    out_color = typhon_sanitize_premultiplied(result);
}
"#
);

pub(crate) const TERMINAL_KAWASE_COMPOSITE_CANONICAL_FRAGMENT_SHADER: &str = concat!(
    r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform float u_presentation_opacity;
uniform vec4 u_effect_input_domain;
uniform vec4 u_effect_output_domain;
uniform vec2 u_effect_output_size;
uniform vec2 u_effect_texel_size;
uniform float u_effect_blur_radius;
uniform int u_effect_encode_srgb;
uniform int u_effect_force_opaque;
uniform int u_effect_input_flip_y;
uniform highp int u_effect_target_flip_y;
uniform int u_effect_coverage_enabled;
uniform int u_effect_coverage_has_rounded_rect;
uniform vec4 u_effect_coverage_rounded_rect;
uniform float u_effect_coverage_radius;
uniform int u_effect_coverage_has_triangle;
uniform vec2 u_effect_coverage_triangle_a;
uniform vec2 u_effect_coverage_triangle_b;
uniform vec2 u_effect_coverage_triangle_c;
uniform mat4 u_effect_color_matrix;
uniform vec4 u_effect_color_bias;
uniform vec4 u_effect_tint_color;
uniform float u_effect_tint_amount;
uniform float u_effect_noise_amount;
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

float typhon_rounded_rect_distance(vec2 p) {
    vec2 size = u_effect_coverage_rounded_rect.zw;
    float radius = max(u_effect_coverage_radius, 0.0);
    vec2 half_size = size * 0.5;
    vec2 center = u_effect_coverage_rounded_rect.xy + half_size;
    vec2 q = abs(p - center) - (half_size - vec2(radius));
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

float typhon_cross(vec2 a, vec2 b) {
    return a.x * b.y - a.y * b.x;
}

float typhon_segment_distance(vec2 p, vec2 a, vec2 b) {
    vec2 edge = b - a;
    float length_squared = dot(edge, edge);
    float t = clamp(dot(p - a, edge) / max(length_squared, 1.0e-8), 0.0, 1.0);
    return length(p - (a + t * edge));
}

float typhon_triangle_distance(vec2 p) {
    vec2 a = u_effect_coverage_triangle_a;
    vec2 b = u_effect_coverage_triangle_b;
    vec2 c = u_effect_coverage_triangle_c;
    float area = typhon_cross(b - a, c - a);
    if (isnan(area) || isinf(area) || abs(area) <= 1.0e-6) return 1.0e20;
    vec3 side = vec3(
        typhon_cross(b - a, p - a),
        typhon_cross(c - b, p - b),
        typhon_cross(a - c, p - c)
    );
    bool inside = all(greaterThanEqual(side, vec3(0.0))) ||
        all(lessThanEqual(side, vec3(0.0)));
    float distance = min(
        typhon_segment_distance(p, a, b),
        min(typhon_segment_distance(p, b, c), typhon_segment_distance(p, c, a))
    );
    return inside ? -distance : distance;
}

float typhon_analytic_coverage(vec2 p) {
    if (u_effect_coverage_enabled == 0) return 1.0;
    float distance = 1.0e20;
    if (u_effect_coverage_has_rounded_rect != 0) {
        distance = min(distance, typhon_rounded_rect_distance(p));
    }
    if (u_effect_coverage_has_triangle != 0) {
        distance = min(distance, typhon_triangle_distance(p));
    }
    if (isnan(distance) || isinf(distance)) return 0.0;
    float aa_width = max(fwidth(distance), 1.0e-4);
    return clamp(1.0 - smoothstep(-aa_width, aa_width, distance), 0.0, 1.0);
}
"#,
    canonical_effect_coordinates_glsl!(),
    rgba16f_rtz_glsl!(),
    virtual_rgba16f_rtz_glsl!(),
    r#"
void main() {
    vec2 output_position = v_uv * u_effect_output_size;
    vec2 logical_input_uv = (output_position - u_effect_input_domain.xy) /
        u_effect_input_domain.zw;
    if (any(lessThan(logical_input_uv, vec2(0.0))) || any(greaterThan(logical_input_uv, vec2(1.0)))) {
        discard;
    }

    vec2 canonical_local_uv = typhon_canonical_effect_local_uv();
    vec2 sample_uv = typhon_effect_sample_uv(canonical_local_uv);
    vec2 offset = u_effect_texel_size * u_effect_blur_radius;
    vec4 result = texture(u_effect_input, sample_uv) * 0.40;
    result += texture(u_effect_input, sample_uv + vec2(offset.x, 0.0)) * 0.15;
    result += texture(u_effect_input, sample_uv - vec2(offset.x, 0.0)) * 0.15;
    result += texture(u_effect_input, sample_uv + vec2(0.0, offset.y)) * 0.15;
    result += texture(u_effect_input, sample_uv - vec2(0.0, offset.y)) * 0.15;
    result = typhon_virtual_rgba16f_rtz(result);
    result = u_effect_color_matrix * result + u_effect_color_bias;
    result.rgb = mix(result.rgb, result.rgb * u_effect_tint_color.rgb, clamp(u_effect_tint_amount, 0.0, 1.0));
    float noise = typhon_canonical_effect_noise();
    result.rgb += noise * u_effect_noise_amount;
    result = typhon_sanitize_premultiplied(result);
    result = typhon_virtual_rgba16f_rtz(result);
    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);
    if (u_effect_force_opaque != 0) result.a = 1.0;
    result *= clamp(u_presentation_opacity, 0.0, 1.0);
    result *= typhon_analytic_coverage(output_position);
    out_color = typhon_sanitize_premultiplied(result);
}
"#
);

#[cfg(test)]
mod canonical_shader_contract_tests {
    use super::{
        CANONICAL_EFFECT_COORDINATES, FRAGMENT_STAGE_FRAGMENT_CANONICAL_SHADER,
        FRAGMENT_STAGE_FRAGMENT_SHADER, RGBA16F_RTZ_GLSL,
        TERMINAL_KAWASE_COMPOSITE_CANONICAL_FRAGMENT_SHADER,
    };

    #[test]
    fn canonical_fragment_declares_the_qualified_coordinate_uniforms() {
        for uniform in [
            "uniform highp int u_effect_target_flip_y;",
            "uniform vec2 u_effect_output_size;",
            "uniform vec4 u_effect_input_domain;",
            "uniform vec4 u_effect_output_domain;",
        ] {
            assert!(
                FRAGMENT_STAGE_FRAGMENT_CANONICAL_SHADER.contains(uniform),
                "canonical Fragment source is missing {uniform}"
            );
        }
        assert!(CANONICAL_EFFECT_COORDINATES.contains("floor(gl_FragCoord.xy)"));
        assert!(CANONICAL_EFFECT_COORDINATES.contains("u_effect_output_domain.xy"));
        assert!(CANONICAL_EFFECT_COORDINATES.contains("u_effect_input_domain.xy"));
        assert!(CANONICAL_EFFECT_COORDINATES.contains("u_effect_input_domain.zw"));
        assert!(CANONICAL_EFFECT_COORDINATES.contains("vec2(12.9898, 78.233)"));
        assert!(CANONICAL_EFFECT_COORDINATES.contains("43758.5453"));
    }

    #[test]
    fn canonical_fragment_changes_only_the_noise_coordinate_authority() {
        assert!(
            FRAGMENT_STAGE_FRAGMENT_CANONICAL_SHADER
                .contains("texture(u_effect_input, typhon_effect_sample_uv(v_uv))")
        );
        assert!(
            FRAGMENT_STAGE_FRAGMENT_CANONICAL_SHADER
                .contains("float noise = typhon_canonical_effect_noise();")
        );
        assert!(
            FRAGMENT_STAGE_FRAGMENT_SHADER
                .contains("float noise = fract(sin(dot(v_uv, vec2(12.9898, 78.233)))")
        );
        assert!(
            FRAGMENT_STAGE_FRAGMENT_CANONICAL_SHADER
                .contains("result.rgb += noise * u_effect_noise_amount;")
        );
    }

    #[test]
    fn canonical_terminal_shader_preserves_rtz_boundaries_and_canonical_coordinates() {
        let source = TERMINAL_KAWASE_COMPOSITE_CANONICAL_FRAGMENT_SHADER;
        let q1_positions = source
            .match_indices("result = typhon_virtual_rgba16f_rtz(result);")
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert_eq!(
            q1_positions.len(),
            2,
            "canonical terminal must have Q1 and Q2"
        );

        let matrix = source
            .find("result = u_effect_color_matrix * result + u_effect_color_bias;")
            .expect("ColorMatrix stage exists");
        let tint = source
            .find("result.rgb = mix(result.rgb, result.rgb * u_effect_tint_color.rgb")
            .expect("Tint stage exists");
        let noise = source
            .find("float noise = typhon_canonical_effect_noise();")
            .expect("canonical Noise helper is used");
        let sanitize = source
            .find("result = typhon_sanitize_premultiplied(result);")
            .expect("Fragment sanitize exists");
        let encode = source
            .find(
                "if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);",
            )
            .expect("output encoding exists");
        assert!(q1_positions[0] < matrix, "Q1 must precede ColorMatrix");
        assert!(
            matrix < tint && tint < noise,
            "material stage order changed"
        );
        assert!(noise < sanitize, "Fragment sanitize must follow Noise");
        assert!(
            sanitize < q1_positions[1],
            "Q2 must follow Fragment sanitize"
        );
        assert!(q1_positions[1] < encode, "Q2 must precede output encoding");

        assert!(source.contains("vec2 canonical_local_uv = typhon_canonical_effect_local_uv();"));
        assert!(source.contains("typhon_effect_sample_uv(canonical_local_uv)"));
        assert!(
            CANONICAL_EFFECT_COORDINATES
                .contains("vec2 canonical_noise_uv = typhon_canonical_effect_local_uv();")
        );
        assert!(RGBA16F_RTZ_GLSL.contains("typhon_float_to_binary16_rtz"));
        assert!(!source.contains("packHalf2x16"));
        for uniform in [
            "u_effect_input",
            "u_effect_input_flip_y",
            "u_effect_target_flip_y",
            "u_effect_texel_size",
            "u_effect_blur_radius",
            "u_effect_input_domain",
            "u_effect_output_domain",
            "u_effect_output_size",
            "u_effect_color_matrix",
            "u_effect_color_bias",
            "u_effect_tint_color",
            "u_effect_tint_amount",
            "u_effect_noise_amount",
            "u_effect_encode_srgb",
            "u_effect_force_opaque",
            "u_presentation_opacity",
            "u_effect_coverage_enabled",
            "u_effect_coverage_has_rounded_rect",
            "u_effect_coverage_rounded_rect",
            "u_effect_coverage_radius",
            "u_effect_coverage_has_triangle",
            "u_effect_coverage_triangle_a",
            "u_effect_coverage_triangle_b",
            "u_effect_coverage_triangle_c",
        ] {
            assert!(
                source.contains(uniform),
                "canonical terminal is missing {uniform}"
            );
        }
    }
}

#[cfg(test)]
pub(crate) const DIAGNOSTIC_KAWASE_ONLY_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform vec2 u_effect_texel_size;
uniform float u_effect_blur_radius;
uniform vec2 u_effect_output_size;
uniform vec4 u_effect_input_domain;
uniform int u_effect_input_flip_y;
uniform highp int u_effect_target_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

void main() {
    ivec2 storage_pixel = ivec2(floor(gl_FragCoord.xy));
    int logical_y = u_effect_target_flip_y != 0
        ? storage_pixel.y
        : int(u_effect_output_size.y) - 1 - storage_pixel.y;
    vec2 logical_output_pixel = vec2(float(storage_pixel.x), float(logical_y));
    vec2 local_pixel = logical_output_pixel - u_effect_input_domain.xy;
    vec2 local_uv = (local_pixel + vec2(0.5)) / u_effect_input_domain.zw;
    vec2 sample_uv = typhon_effect_sample_uv(local_uv);
    vec2 offset = u_effect_texel_size * u_effect_blur_radius;
    vec4 result = texture(u_effect_input, sample_uv) * 0.40;
    result += texture(u_effect_input, sample_uv + vec2(offset.x, 0.0)) * 0.15;
    result += texture(u_effect_input, sample_uv - vec2(offset.x, 0.0)) * 0.15;
    result += texture(u_effect_input, sample_uv + vec2(0.0, offset.y)) * 0.15;
    result += texture(u_effect_input, sample_uv - vec2(0.0, offset.y)) * 0.15;
    out_color = result;
}
"#;

#[cfg(test)]
pub(crate) const DIAGNOSTIC_LEGACY_UP2_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform vec2 u_effect_texel_size;
uniform float u_effect_blur_radius;
uniform vec2 u_effect_output_size;
uniform mat4 u_effect_color_matrix;
uniform vec4 u_effect_color_bias;
uniform vec4 u_effect_tint_color;
uniform float u_effect_tint_amount;
uniform float u_effect_noise_amount;
uniform int u_effect_input_flip_y;
uniform highp int u_effect_target_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

vec4 typhon_sanitize_premultiplied(vec4 value) {
    if (any(isnan(value)) || any(isinf(value))) return vec4(0.0);
    float alpha = clamp(value.a, 0.0, 1.0);
    return vec4(clamp(value.rgb, vec3(0.0), vec3(alpha)), alpha);
}

vec4 typhon_virtual_rgba16f_boundary(vec4 value) {
    return vec4(
        unpackHalf2x16(packHalf2x16(value.rg)),
        unpackHalf2x16(packHalf2x16(value.ba))
    );
}

void main() {
    ivec2 storage_pixel = ivec2(floor(gl_FragCoord.xy));
    int logical_y = u_effect_target_flip_y != 0
        ? storage_pixel.y
        : int(u_effect_output_size.y) - 1 - storage_pixel.y;
    vec2 local_pixel = vec2(float(storage_pixel.x), float(logical_y));
    vec2 local_uv = (local_pixel + vec2(0.5)) / u_effect_output_size;
    vec2 sample_uv = typhon_effect_sample_uv(local_uv);
    vec2 offset = u_effect_texel_size * u_effect_blur_radius;
    vec4 result = texture(u_effect_input, sample_uv) * 0.40;
    result += texture(u_effect_input, sample_uv + vec2(offset.x, 0.0)) * 0.15;
    result += texture(u_effect_input, sample_uv - vec2(offset.x, 0.0)) * 0.15;
    result += texture(u_effect_input, sample_uv + vec2(0.0, offset.y)) * 0.15;
    result += texture(u_effect_input, sample_uv - vec2(0.0, offset.y)) * 0.15;
    result = typhon_virtual_rgba16f_boundary(result);
    result = u_effect_color_matrix * result + u_effect_color_bias;
    result.rgb = mix(result.rgb, result.rgb * u_effect_tint_color.rgb, clamp(u_effect_tint_amount, 0.0, 1.0));
    float noise = fract(sin(dot(v_uv, vec2(12.9898, 78.233))) * 43758.5453) - 0.5;
    result.rgb += noise * u_effect_noise_amount;
    out_color = typhon_sanitize_premultiplied(result);
}
"#;

#[cfg(test)]
pub(crate) const DIAGNOSTIC_TERMINAL_UP2_FRAGMENT_STAGE_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform vec2 u_effect_texel_size;
uniform float u_effect_blur_radius;
uniform vec2 u_effect_output_size;
uniform vec4 u_effect_input_domain;
uniform int u_effect_input_flip_y;
uniform highp int u_effect_target_flip_y;
uniform mat4 u_effect_color_matrix;
uniform vec4 u_effect_color_bias;
uniform vec4 u_effect_tint_color;
uniform float u_effect_tint_amount;
uniform float u_effect_noise_amount;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

vec4 typhon_sanitize_premultiplied(vec4 value) {
    if (any(isnan(value)) || any(isinf(value))) return vec4(0.0);
    float alpha = clamp(value.a, 0.0, 1.0);
    return vec4(clamp(value.rgb, vec3(0.0), vec3(alpha)), alpha);
}

vec4 typhon_virtual_rgba16f_boundary(vec4 value) {
    return vec4(
        unpackHalf2x16(packHalf2x16(value.rg)),
        unpackHalf2x16(packHalf2x16(value.ba))
    );
}

void main() {
    vec2 output_position = v_uv * u_effect_output_size;
    vec2 local_pixel = floor(output_position - u_effect_input_domain.xy);
    vec2 local_uv = (local_pixel + vec2(0.5)) / u_effect_input_domain.zw;
    ivec2 storage_pixel = ivec2(floor(gl_FragCoord.xy));
    int logical_y = u_effect_target_flip_y != 0
        ? storage_pixel.y
        : int(u_effect_output_size.y) - 1 - storage_pixel.y;
    vec2 logical_output_pixel = vec2(float(storage_pixel.x), float(logical_y));
    vec2 canonical_local_pixel = logical_output_pixel - u_effect_input_domain.xy;
    vec2 canonical_uv =
        (canonical_local_pixel + vec2(0.5)) / u_effect_input_domain.zw;
    vec2 sample_uv = typhon_effect_sample_uv(canonical_uv);
    vec2 offset = u_effect_texel_size * u_effect_blur_radius;
    vec4 result = texture(u_effect_input, sample_uv) * 0.40;
    result += texture(u_effect_input, sample_uv + vec2(offset.x, 0.0)) * 0.15;
    result += texture(u_effect_input, sample_uv - vec2(offset.x, 0.0)) * 0.15;
    result += texture(u_effect_input, sample_uv + vec2(0.0, offset.y)) * 0.15;
    result += texture(u_effect_input, sample_uv - vec2(0.0, offset.y)) * 0.15;
    result = typhon_virtual_rgba16f_boundary(result);
    result = u_effect_color_matrix * result + u_effect_color_bias;
    result.rgb = mix(result.rgb, result.rgb * u_effect_tint_color.rgb, clamp(u_effect_tint_amount, 0.0, 1.0));
    float noise = fract(sin(dot(local_uv, vec2(12.9898, 78.233))) * 43758.5453) - 0.5;
    result.rgb += noise * u_effect_noise_amount;
    result = typhon_sanitize_premultiplied(result);
    result = typhon_virtual_rgba16f_boundary(result);
    out_color = typhon_sanitize_premultiplied(result);
}
"#;

#[cfg(test)]
pub(crate) const DIAGNOSTIC_MATERIAL_COMPOSITE_VERTEX_SHADER: &str = r#"#version 300 es
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_uv;
uniform int u_effect_target_flip_y;
uniform vec2 u_effect_output_size;
uniform vec4 u_effect_input_domain;
out vec2 v_uv;
out vec2 v_output_uv;
out vec2 v_legacy_noise_uv;

void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
    v_uv = a_uv;
    v_output_uv = u_effect_target_flip_y != 0
        ? vec2(a_uv.x, 1.0 - a_uv.y)
        : a_uv;
    vec2 output_position = v_output_uv * u_effect_output_size;
    vec2 local_uv = (output_position - u_effect_input_domain.xy) /
        u_effect_input_domain.zw;
    v_legacy_noise_uv = vec2(
        local_uv.x,
        u_effect_target_flip_y != 0 ? 1.0 - local_uv.y : local_uv.y
    );
}
"#;

#[cfg(test)]
pub(crate) const DIAGNOSTIC_MATERIAL_COMPOSITE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform float u_presentation_opacity;
uniform vec4 u_effect_input_domain;
uniform vec2 u_effect_output_size;
uniform int u_effect_encode_srgb;
uniform int u_effect_force_opaque;
uniform int u_effect_input_flip_y;
uniform int u_effect_coverage_enabled;
uniform int u_effect_coverage_has_rounded_rect;
uniform vec4 u_effect_coverage_rounded_rect;
uniform float u_effect_coverage_radius;
uniform int u_effect_coverage_has_triangle;
uniform vec2 u_effect_coverage_triangle_a;
uniform vec2 u_effect_coverage_triangle_b;
uniform vec2 u_effect_coverage_triangle_c;
uniform mat4 u_effect_color_matrix;
uniform vec4 u_effect_color_bias;
uniform vec4 u_effect_tint_color;
uniform float u_effect_tint_amount;
uniform float u_effect_noise_amount;
in vec2 v_uv;
in vec2 v_output_uv;
in vec2 v_legacy_noise_uv;
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
vec4 typhon_virtual_rgba16f_boundary(vec4 value) {
    return vec4(
        unpackHalf2x16(packHalf2x16(value.rg)),
        unpackHalf2x16(packHalf2x16(value.ba))
    );
}
vec4 typhon_encode_premultiplied_srgb(vec4 value) {
    value = typhon_sanitize_premultiplied(value);
    if (value.a <= 0.00001) return vec4(0.0);
    vec3 straight = clamp(value.rgb / value.a, vec3(0.0), vec3(1.0));
    vec3 encoded = vec3(typhon_encode_srgb_channel(straight.r), typhon_encode_srgb_channel(straight.g), typhon_encode_srgb_channel(straight.b));
    return typhon_sanitize_premultiplied(vec4(encoded * value.a, value.a));
}
float typhon_rounded_rect_distance(vec2 p) {
    vec2 size = u_effect_coverage_rounded_rect.zw;
    float radius = max(u_effect_coverage_radius, 0.0);
    vec2 half_size = size * 0.5;
    vec2 center = u_effect_coverage_rounded_rect.xy + half_size;
    vec2 q = abs(p - center) - (half_size - vec2(radius));
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}
float typhon_cross(vec2 a, vec2 b) { return a.x * b.y - a.y * b.x; }
float typhon_segment_distance(vec2 p, vec2 a, vec2 b) {
    vec2 edge = b - a;
    float length_squared = dot(edge, edge);
    float t = clamp(dot(p - a, edge) / max(length_squared, 1.0e-8), 0.0, 1.0);
    return length(p - (a + t * edge));
}
float typhon_triangle_distance(vec2 p) {
    vec2 a = u_effect_coverage_triangle_a;
    vec2 b = u_effect_coverage_triangle_b;
    vec2 c = u_effect_coverage_triangle_c;
    float area = typhon_cross(b - a, c - a);
    if (isnan(area) || isinf(area) || abs(area) <= 1.0e-6) return 1.0e20;
    vec3 side = vec3(typhon_cross(b - a, p - a), typhon_cross(c - b, p - b), typhon_cross(a - c, p - c));
    bool inside = all(greaterThanEqual(side, vec3(0.0))) || all(lessThanEqual(side, vec3(0.0)));
    float distance = min(typhon_segment_distance(p, a, b), min(typhon_segment_distance(p, b, c), typhon_segment_distance(p, c, a)));
    return inside ? -distance : distance;
}
float typhon_analytic_coverage(vec2 p) {
    if (u_effect_coverage_enabled == 0) return 1.0;
    float distance = 1.0e20;
    if (u_effect_coverage_has_rounded_rect != 0) distance = min(distance, typhon_rounded_rect_distance(p));
    if (u_effect_coverage_has_triangle != 0) distance = min(distance, typhon_triangle_distance(p));
    if (isnan(distance) || isinf(distance)) return 0.0;
    float aa_width = max(fwidth(distance), 1.0e-4);
    return clamp(1.0 - smoothstep(-aa_width, aa_width, distance), 0.0, 1.0);
}
void main() {
    vec2 output_position = v_output_uv * u_effect_output_size;
    vec2 logical_input_uv = (output_position - u_effect_input_domain.xy) / u_effect_input_domain.zw;
    if (any(lessThan(logical_input_uv, vec2(0.0))) || any(greaterThan(logical_input_uv, vec2(1.0)))) discard;
    vec4 result = texture(u_effect_input, typhon_effect_sample_uv(logical_input_uv));
    result = u_effect_color_matrix * result + u_effect_color_bias;
    result.rgb = mix(result.rgb, result.rgb * u_effect_tint_color.rgb, clamp(u_effect_tint_amount, 0.0, 1.0));
    float noise = fract(sin(dot(v_legacy_noise_uv, vec2(12.9898, 78.233))) * 43758.5453) - 0.5;
    result.rgb += noise * u_effect_noise_amount;
    result = typhon_sanitize_premultiplied(result);
    result = typhon_virtual_rgba16f_boundary(result);
    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);
    if (u_effect_force_opaque != 0) result.a = 1.0;
    result *= clamp(u_presentation_opacity, 0.0, 1.0);
    result *= typhon_analytic_coverage(output_position);
    out_color = typhon_sanitize_premultiplied(result);
}
"#;

#[cfg(test)]
fn replace_diagnostic_shader_once(source: &mut String, before: &str, after: &str) {
    assert_eq!(
        source.matches(before).count(),
        1,
        "diagnostic shader anchor must be unique"
    );
    *source = source.replacen(before, after, 1);
}

#[cfg(test)]
pub(crate) fn diagnostic_kawase_only_shader(q1: bool, encode_blue_bits: bool) -> String {
    diagnostic_kawase_only_channel_bits_shader(q1, encode_blue_bits.then_some("b"))
}

#[cfg(test)]
pub(crate) fn diagnostic_kawase_only_channel_bits_shader(
    q1: bool,
    encode_channel: Option<&str>,
) -> String {
    let mut source = DIAGNOSTIC_KAWASE_ONLY_FRAGMENT_SHADER.to_owned();
    if q1 {
        let main = format!("{RGBA16F_RTZ_GLSL}\n{VIRTUAL_RGBA16F_RTZ_GLSL}\n\nvoid main() {{");
        replace_diagnostic_shader_once(&mut source, "void main() {", &main);
        replace_diagnostic_shader_once(
            &mut source,
            "    result += texture(u_effect_input, sample_uv - vec2(0.0, offset.y)) * 0.15;\n    out_color = result;",
            "    result += texture(u_effect_input, sample_uv - vec2(0.0, offset.y)) * 0.15;\n    result = typhon_virtual_rgba16f_rtz(result);\n    out_color = result;",
        );
    }
    if let Some(channel) = encode_channel {
        let encode_statement = diagnostic_float_bits_output(channel);
        replace_diagnostic_shader_once(&mut source, "    out_color = result;", &encode_statement);
    }
    source
}

#[cfg(test)]
fn diagnostic_float_bits_output(channel: &str) -> String {
    assert!(matches!(channel, "r" | "g" | "b" | "a"));
    format!(
        "    uint bits = floatBitsToUint(result.{channel});\n    out_color = vec4(\n        float((bits >> 0u) & 255u),\n        float((bits >> 8u) & 255u),\n        float((bits >> 16u) & 255u),\n        float((bits >> 24u) & 255u)\n    ) / 255.0;"
    )
}

#[cfg(test)]
pub(crate) fn diagnostic_legacy_up2_canonical_shader() -> String {
    super::super::blur::DUAL_KAWASE_UPSAMPLE_CANONICAL_SHADER.to_owned()
}

#[cfg(test)]
pub(crate) fn diagnostic_fragment_stage_q2_shader() -> String {
    let mut source = FRAGMENT_STAGE_FRAGMENT_SHADER.to_owned();
    let main = format!("{RGBA16F_RTZ_GLSL}\n{VIRTUAL_RGBA16F_RTZ_GLSL}\n\nvoid main() {{");
    replace_diagnostic_shader_once(&mut source, "void main() {", &main);
    replace_diagnostic_shader_once(
        &mut source,
        "    result = typhon_sanitize_premultiplied(result);\n    if (u_effect_encode_srgb != 0)",
        "    result = typhon_sanitize_premultiplied(result);\n    result = typhon_virtual_rgba16f_rtz(result);\n    if (u_effect_encode_srgb != 0)",
    );
    source
}

#[cfg(test)]
pub(crate) fn diagnostic_canonical_material_shader() -> String {
    FRAGMENT_STAGE_FRAGMENT_CANONICAL_SHADER.to_owned()
}

#[cfg(test)]
pub(crate) fn diagnostic_canonical_noise_field_shader() -> String {
    format!(
        "#version 300 es\nprecision highp float;\nprecision highp int;\nuniform vec2 u_effect_output_size;\nuniform vec4 u_effect_input_domain;\nuniform vec4 u_effect_output_domain;\nuniform highp int u_effect_target_flip_y;\nout highp vec4 out_color;\n\n{CANONICAL_EFFECT_COORDINATES}\n\nvoid main() {{\n    float noise = typhon_canonical_effect_noise();\n    uint bits = floatBitsToUint(noise);\n    out_color = vec4(\n        float(bits & 255u),\n        float((bits >> 8u) & 255u),\n        float((bits >> 16u) & 255u),\n        float((bits >> 24u) & 255u)\n    );\n}}\n"
    )
}

#[cfg(test)]
pub(crate) fn diagnostic_fragment_stage_boundary_probe_shader(q2: bool, channel: &str) -> String {
    let mut source = FRAGMENT_STAGE_FRAGMENT_SHADER.to_owned();
    if q2 {
        let main = format!("{RGBA16F_RTZ_GLSL}\n{VIRTUAL_RGBA16F_RTZ_GLSL}\n\nvoid main() {{");
        replace_diagnostic_shader_once(&mut source, "void main() {", &main);
        replace_diagnostic_shader_once(
            &mut source,
            "    result = typhon_sanitize_premultiplied(result);\n    if (u_effect_encode_srgb != 0)",
            "    result = typhon_sanitize_premultiplied(result);\n    result = typhon_virtual_rgba16f_rtz(result);\n    if (u_effect_encode_srgb != 0)",
        );
    }
    replace_diagnostic_shader_once(
        &mut source,
        "    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);\n    out_color = typhon_sanitize_premultiplied(result);",
        &diagnostic_float_bits_output(channel),
    );
    source
}

#[cfg(test)]
pub(crate) fn diagnostic_terminal_shader_with_virtual_boundaries(q1: bool, q2: bool) -> String {
    let mut source = TERMINAL_KAWASE_COMPOSITE_FRAGMENT_SHADER.to_owned();
    replace_diagnostic_shader_once(
        &mut source,
        "uniform vec2 u_effect_output_size;",
        "uniform vec2 u_effect_output_size;\nuniform highp int u_effect_target_flip_y;\nuniform vec4 u_effect_output_domain;",
    );
    replace_diagnostic_shader_once(
        &mut source,
        "    vec2 effect_local_pixel = floor(output_position - u_effect_input_domain.xy);\n    vec2 effect_local_uv =\n        (effect_local_pixel + vec2(0.5)) / u_effect_input_domain.zw;",
        "    vec2 effect_local_pixel = floor(output_position - u_effect_input_domain.xy);\n    vec2 effect_local_uv =\n        (effect_local_pixel + vec2(0.5)) / u_effect_input_domain.zw;\n\n    ivec2 storage_pixel = ivec2(floor(gl_FragCoord.xy));\n    int logical_y = u_effect_target_flip_y != 0\n        ? storage_pixel.y\n        : int(u_effect_output_size.y) - 1 - storage_pixel.y;\n    vec2 logical_output_pixel = vec2(float(storage_pixel.x), float(logical_y));\n    vec2 canonical_local_pixel = logical_output_pixel - u_effect_input_domain.xy;\n    vec2 canonical_effect_local_uv =\n        (canonical_local_pixel + vec2(0.5)) / u_effect_input_domain.zw;",
    );
    replace_diagnostic_shader_once(
        &mut source,
        "vec2 sample_uv = typhon_effect_sample_uv(effect_local_uv);",
        "vec2 sample_uv = typhon_effect_sample_uv(canonical_effect_local_uv);",
    );
    if q1 || q2 {
        let main = format!("{RGBA16F_RTZ_GLSL}\n{VIRTUAL_RGBA16F_RTZ_GLSL}\n\nvoid main() {{");
        replace_diagnostic_shader_once(&mut source, "void main() {", &main);
    }
    if q1 {
        replace_diagnostic_shader_once(
            &mut source,
            "    result = u_effect_color_matrix * result + u_effect_color_bias;",
            "    result = typhon_virtual_rgba16f_rtz(result);\n    result = u_effect_color_matrix * result + u_effect_color_bias;",
        );
    }
    if q2 {
        replace_diagnostic_shader_once(
            &mut source,
            "    result = typhon_sanitize_premultiplied(result);\n    if (u_effect_encode_srgb != 0)",
            "    result = typhon_sanitize_premultiplied(result);\n    result = typhon_virtual_rgba16f_rtz(result);\n    if (u_effect_encode_srgb != 0)",
        );
    }
    source
}

#[cfg(test)]
pub(crate) fn diagnostic_canonical_terminal_shader_with_virtual_boundaries(
    q1: bool,
    q2: bool,
) -> String {
    let mut source = TERMINAL_KAWASE_COMPOSITE_CANONICAL_FRAGMENT_SHADER.to_owned();
    let boundary = "    result = typhon_virtual_rgba16f_rtz(result);";
    let positions = source
        .match_indices(boundary)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(
        positions.len(),
        2,
        "canonical terminal shader contains Q1 and Q2"
    );
    for (enabled, position) in [(q2, positions[1]), (q1, positions[0])] {
        if !enabled {
            source.replace_range(position..position + boundary.len(), "");
        }
    }
    source
}

#[cfg(test)]
pub(crate) fn diagnostic_canonical_terminal_q1_capture_shader() -> String {
    let mut source = diagnostic_canonical_terminal_shader_with_virtual_boundaries(true, true);
    replace_diagnostic_shader_once(
        &mut source,
        "    result = u_effect_color_matrix * result + u_effect_color_bias;",
        "    out_color = result;\n    return;\n    result = u_effect_color_matrix * result + u_effect_color_bias;",
    );
    source
}

#[cfg(test)]
pub(crate) fn diagnostic_canonical_terminal_q2_capture_shader() -> String {
    let mut source = diagnostic_canonical_terminal_shader_with_virtual_boundaries(true, true);
    replace_diagnostic_shader_once(
        &mut source,
        "    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);",
        "    out_color = result;\n    return;\n    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);",
    );
    source
}

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
