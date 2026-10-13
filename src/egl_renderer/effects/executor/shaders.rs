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
const DIAGNOSTIC_VIRTUAL_RGBA16F_BOUNDARY: &str = r#"vec4 typhon_virtual_rgba16f_boundary(vec4 value) {
    return vec4(
        unpackHalf2x16(packHalf2x16(value.rg)),
        unpackHalf2x16(packHalf2x16(value.ba))
    );
}"#;

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
        let main = format!("{DIAGNOSTIC_VIRTUAL_RGBA16F_BOUNDARY}\n\nvoid main() {{");
        replace_diagnostic_shader_once(&mut source, "void main() {", &main);
        replace_diagnostic_shader_once(
            &mut source,
            "    result += texture(u_effect_input, sample_uv - vec2(0.0, offset.y)) * 0.15;\n    out_color = result;",
            "    result += texture(u_effect_input, sample_uv - vec2(0.0, offset.y)) * 0.15;\n    result = typhon_virtual_rgba16f_boundary(result);\n    out_color = result;",
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
    let mut source = DIAGNOSTIC_KAWASE_ONLY_FRAGMENT_SHADER.to_owned();
    replace_diagnostic_shader_once(
        &mut source,
        "    vec2 logical_output_pixel = vec2(float(storage_pixel.x), float(logical_y));\n    vec2 local_pixel = logical_output_pixel - u_effect_input_domain.xy;\n    vec2 local_uv = (local_pixel + vec2(0.5)) / u_effect_input_domain.zw;",
        "    vec2 local_pixel = vec2(float(storage_pixel.x), float(logical_y));\n    vec2 local_uv = (local_pixel + vec2(0.5)) / u_effect_output_size;",
    );
    source
}

#[cfg(test)]
pub(crate) fn diagnostic_fragment_stage_q2_shader() -> String {
    let mut source = FRAGMENT_STAGE_FRAGMENT_SHADER.to_owned();
    let main = format!("{DIAGNOSTIC_VIRTUAL_RGBA16F_BOUNDARY}\n\nvoid main() {{");
    replace_diagnostic_shader_once(&mut source, "void main() {", &main);
    replace_diagnostic_shader_once(
        &mut source,
        "    result = typhon_sanitize_premultiplied(result);\n    if (u_effect_encode_srgb != 0)",
        "    result = typhon_sanitize_premultiplied(result);\n    result = typhon_virtual_rgba16f_boundary(result);\n    if (u_effect_encode_srgb != 0)",
    );
    source
}

#[cfg(test)]
pub(crate) fn diagnostic_fragment_stage_boundary_probe_shader(q2: bool, channel: &str) -> String {
    let mut source = FRAGMENT_STAGE_FRAGMENT_SHADER.to_owned();
    if q2 {
        let main = format!("{DIAGNOSTIC_VIRTUAL_RGBA16F_BOUNDARY}\n\nvoid main() {{");
        replace_diagnostic_shader_once(&mut source, "void main() {", &main);
        replace_diagnostic_shader_once(
            &mut source,
            "    result = typhon_sanitize_premultiplied(result);\n    if (u_effect_encode_srgb != 0)",
            "    result = typhon_sanitize_premultiplied(result);\n    result = typhon_virtual_rgba16f_boundary(result);\n    if (u_effect_encode_srgb != 0)",
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
        "uniform vec2 u_effect_output_size;\nuniform highp int u_effect_target_flip_y;",
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
        let main = format!("{DIAGNOSTIC_VIRTUAL_RGBA16F_BOUNDARY}\n\nvoid main() {{");
        replace_diagnostic_shader_once(&mut source, "void main() {", &main);
    }
    if q1 {
        replace_diagnostic_shader_once(
            &mut source,
            "    result = u_effect_color_matrix * result + u_effect_color_bias;",
            "    result = typhon_virtual_rgba16f_boundary(result);\n    result = u_effect_color_matrix * result + u_effect_color_bias;",
        );
    }
    if q2 {
        replace_diagnostic_shader_once(
            &mut source,
            "    result = typhon_sanitize_premultiplied(result);\n    if (u_effect_encode_srgb != 0)",
            "    result = typhon_sanitize_premultiplied(result);\n    result = typhon_virtual_rgba16f_boundary(result);\n    if (u_effect_encode_srgb != 0)",
        );
    }
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
