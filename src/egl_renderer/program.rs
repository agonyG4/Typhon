#![allow(clippy::items_after_test_module)]

use std::io;

use glow::HasContext;

use super::{GlProgram, RendererResult};

pub(super) fn create_texture_program(gl: &glow::Context) -> RendererResult<GlProgram> {
    create_program_from_sources(gl, EGL_VERTEX_SHADER, EGL_FRAGMENT_SHADER)
}

pub(super) fn create_lamp_program(gl: &glow::Context) -> RendererResult<GlProgram> {
    create_program_from_sources(gl, LAMP_VERTEX_SHADER, LAMP_FRAGMENT_SHADER)
}

pub(super) fn create_capture_program(gl: &glow::Context) -> RendererResult<GlProgram> {
    create_program_from_sources(gl, CAPTURE_VERTEX_SHADER, EGL_FRAGMENT_SHADER)
}

pub(super) fn create_capture_copy_program(gl: &glow::Context) -> RendererResult<GlProgram> {
    create_program_from_sources(gl, EGL_VERTEX_SHADER, CAPTURE_COPY_FRAGMENT_SHADER)
}

pub(super) fn create_program_from_sources(
    gl: &glow::Context,
    vertex_source: &str,
    fragment_source: &str,
) -> RendererResult<GlProgram> {
    let vertex_shader = compile_shader(gl, glow::VERTEX_SHADER, vertex_source)?;
    let fragment_shader = compile_shader(gl, glow::FRAGMENT_SHADER, fragment_source)?;
    let program = unsafe { gl.create_program().map_err(io::Error::other)? };
    unsafe {
        gl.attach_shader(program, vertex_shader);
        gl.attach_shader(program, fragment_shader);
        gl.link_program(program);
        gl.detach_shader(program, vertex_shader);
        gl.detach_shader(program, fragment_shader);
        gl.delete_shader(vertex_shader);
        gl.delete_shader(fragment_shader);
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(io::Error::other(format!("EGL/GLES shader link failed: {log}")).into());
        }
    }
    Ok(program)
}

fn compile_shader(
    gl: &glow::Context,
    shader_type: u32,
    source: &str,
) -> RendererResult<<glow::Context as HasContext>::Shader> {
    let shader = unsafe { gl.create_shader(shader_type).map_err(io::Error::other)? };
    unsafe {
        gl.shader_source(shader, source);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            return Err(io::Error::other(format!("EGL/GLES shader compile failed: {log}")).into());
        }
    }
    Ok(shader)
}

const EGL_VERTEX_SHADER: &str = r#"#version 300 es
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_uv;
out vec2 v_uv;

void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
    v_uv = a_uv;
}
"#;

const EGL_FRAGMENT_SHADER: &str = r#"#version 300 es
precision mediump float;
uniform sampler2D u_texture;
uniform float u_opacity;
in vec2 v_uv;
out vec4 out_color;

void main() {
    out_color = texture(u_texture, v_uv) * clamp(u_opacity, 0.0, 1.0);
}
"#;

const CAPTURE_COPY_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
precision highp int;

uniform sampler2D u_output_texture;
uniform vec2 u_capture_output_size;
uniform vec4 u_capture_domain;
uniform vec2 u_capture_target_size;
uniform int u_capture_origin_bottom_left;

out vec4 out_color;

void main() {
    ivec2 destination = ivec2(gl_FragCoord.xy);
    int logical_y = int(u_capture_target_size.y) - 1 - destination.y;
    int source_x = int(u_capture_domain.x) + destination.x;
    int source_y = u_capture_origin_bottom_left != 0
        ? int(u_capture_output_size.y) - 1 - int(u_capture_domain.y) - logical_y
        : int(u_capture_domain.y) + logical_y;
    out_color = texelFetch(u_output_texture, ivec2(source_x, source_y), 0);
}
"#;

const LAMP_VERTEX_SHADER: &str = r#"#version 300 es
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_uv;
out vec2 v_uv;

uniform vec2 u_output_size;
uniform vec4 u_canonical_visual_rect;
uniform vec4 u_source_visual_rect;
uniform vec4 u_sink_rect;
uniform float u_progress;
uniform int u_direction;
uniform float u_shape_factor;
uniform float u_bump_distance;
uniform float u_contraction_progress;
uniform float u_translation_progress;
uniform float u_retreat_progress;
uniform int u_framebuffer_origin_bottom_left;

// These are spatial constants. All temporal channels arrive from the CPU.
const float LAMP_SHAPE_MIN = 0.20;
const float LAMP_SHAPE_MAX = 0.80;
const float LAMP_STRETCH_POWER = 2.0;
const float LAMP_RAIL_C1_LOW_SHAPE = 0.14;
const float LAMP_RAIL_C1_HIGH_SHAPE = 0.04;
const float LAMP_RAIL_C2_LOW_SHAPE = 0.55;
const float LAMP_RAIL_C2_HIGH_SHAPE = 0.24;

float axis_position(vec4 rectangle, float normalized) {
    if (u_direction == 0) {
        return rectangle.y + rectangle.w * (1.0 - normalized);
    }
    if (u_direction == 1) {
        return rectangle.x + rectangle.z * normalized;
    }
    if (u_direction == 2) {
        return rectangle.y + rectangle.w * normalized;
    }
    return rectangle.x + rectangle.z * (1.0 - normalized);
}

float cross_position(vec4 rectangle, float normalized) {
    if (u_direction == 0 || u_direction == 2) {
        return rectangle.x + rectangle.z * normalized;
    }
    return rectangle.y + rectangle.w * normalized;
}

float movement_extent(vec4 rectangle) {
    return max((u_direction == 0 || u_direction == 2) ? rectangle.w : rectangle.z, 1.0);
}

float cubic_funnel_profile(float t, float shape_factor) {
    t = clamp(t, 0.0, 1.0);
    float normalized = (
        clamp(shape_factor, LAMP_SHAPE_MIN, LAMP_SHAPE_MAX) - LAMP_SHAPE_MIN
    ) / (LAMP_SHAPE_MAX - LAMP_SHAPE_MIN);
    float c1 = mix(LAMP_RAIL_C1_LOW_SHAPE, LAMP_RAIL_C1_HIGH_SHAPE, normalized);
    float c2 = mix(LAMP_RAIL_C2_LOW_SHAPE, LAMP_RAIL_C2_HIGH_SHAPE, normalized);
    float u = 1.0 - t;
    return clamp(
        3.0 * u * u * t * c1
        + 3.0 * u * t * t * c2
        + t * t * t,
        0.0,
        1.0
    );
}

void main() {
    vec2 source_point = u_source_visual_rect.xy
        + ((a_position - u_canonical_visual_rect.xy) / u_canonical_visual_rect.zw)
        * u_source_visual_rect.zw;
    float progress = clamp(u_progress, 0.0, 1.0);
    vec2 group_uv = clamp(
        (source_point - u_source_visual_rect.xy) / u_source_visual_rect.zw,
        0.0,
        1.0
    );
    vec2 target = u_sink_rect.xy + group_uv * u_sink_rect.zw;
    vec2 warped = source_point;
    if (progress >= 1.0) {
        warped = target;
    } else if (progress > 0.0) {
        float movement_normalized = (u_direction == 0)
            ? 1.0 - group_uv.y
            : ((u_direction == 1) ? group_uv.x
                : ((u_direction == 2) ? group_uv.y : 1.0 - group_uv.x));
        float cross_normalized = (u_direction == 0 || u_direction == 2)
            ? group_uv.x
            : group_uv.y;
        float funnel_weight = cubic_funnel_profile(
            movement_normalized,
            u_shape_factor
        );
        float early_contraction = clamp(
            clamp(u_contraction_progress, 0.0, 1.0) * funnel_weight,
            0.0,
            1.0
        );
        float stretch = LAMP_STRETCH_POWER
            * clamp(u_contraction_progress, 0.0, 1.0)
            * (1.0 - movement_normalized);
        float translation_progress = clamp(u_translation_progress, 0.0, 1.0);
        float row_translation = translation_progress >= 1.0
            ? 1.0
            : pow(translation_progress, 1.0 + stretch);
        float retreat_distance = clamp(
            u_bump_distance,
            0.0,
            movement_extent(u_source_visual_rect)
        ) * clamp(u_retreat_progress, 0.0, 1.0);
        float retreat_sign = (u_direction == 0 || u_direction == 3) ? 1.0 : -1.0;
        float source_axis = axis_position(u_source_visual_rect, movement_normalized)
            + retreat_sign * retreat_distance;
        float target_axis = axis_position(u_sink_rect, movement_normalized);
        float axis = mix(source_axis, target_axis, row_translation);
        float source_cross = cross_position(u_source_visual_rect, cross_normalized);
        float target_cross = cross_position(u_sink_rect, cross_normalized);
        float cross_completion = clamp(
            1.0 - (1.0 - early_contraction) * (1.0 - row_translation),
            0.0,
            1.0
        );
        float cross = mix(source_cross, target_cross, cross_completion);
        warped = (u_direction == 0 || u_direction == 2)
            ? vec2(cross, axis)
            : vec2(axis, cross);
    }
    vec2 clip = vec2(warped.x / u_output_size.x * 2.0 - 1.0,
        u_framebuffer_origin_bottom_left != 0
            ? 1.0 - warped.y / u_output_size.y * 2.0
            : warped.y / u_output_size.y * 2.0 - 1.0);
    gl_Position = vec4(clip, 0.0, 1.0);
    v_uv = a_uv;
}
"#;

const LAMP_FRAGMENT_SHADER: &str = r#"#version 300 es
precision mediump float;
uniform sampler2D u_texture;
uniform float u_opacity;
in vec2 v_uv;
out vec4 out_color;

void main() {
    out_color = texture(u_texture, v_uv) * u_opacity;
}
"#;

#[cfg(test)]
mod tests {
    use super::LAMP_VERTEX_SHADER;
    use oblivion_one::presentation_animation::PresentationRect;
    use oblivion_one::window_lifecycle_animation::{
        LifecycleVisualGroup, cubic_funnel_profile, lamp_warp_visual_point,
    };

    fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
        PresentationRect::new(x, y, width, height).expect("valid test rectangle")
    }

    fn map_canonical_visual_point(
        canonical_visual: PresentationRect,
        source_visual: PresentationRect,
        point: [f64; 2],
    ) -> [f64; 2] {
        [
            source_visual.x()
                + (point[0] - canonical_visual.x()) / canonical_visual.width()
                    * source_visual.width(),
            source_visual.y()
                + (point[1] - canonical_visual.y()) / canonical_visual.height()
                    * source_visual.height(),
        ]
    }

    #[test]
    fn lamp_shader_and_cpu_reference_use_complete_visual_group_domain() {
        let canonical_client = rect(400.0, 100.0, 800.0, 600.0);
        let canonical_visual = rect(384.0, 60.0, 832.0, 640.0);
        let source_client = rect(200.0, 160.0, 960.0, 720.0);
        let anchor = rect(1500.0, 500.0, 64.0, 64.0);
        let group = LifecycleVisualGroup::from_bounds(
            canonical_client,
            canonical_visual,
            source_client,
            anchor,
            1920,
            1080,
        )
        .expect("valid visual group with SSD outside the client");
        let points = [
            [500.0, 60.0],   // SSD top edge
            [384.0, 300.0],  // SSD left edge
            [400.0, 100.0],  // client top-left
            [800.0, 400.0],  // client center
            [1200.0, 700.0], // client bottom-right
        ];

        assert!(group.presented_source_visual_rect.y() < group.presented_source_client_rect.y());
        assert!(group.presented_source_visual_rect.x() < group.presented_source_client_rect.x());
        for canonical_point in points {
            let source_point = map_canonical_visual_point(
                group.canonical_visual_rect,
                group.presented_source_visual_rect,
                canonical_point,
            );
            assert_eq!(
                lamp_warp_visual_point(group, source_point, 0.0),
                source_point
            );

            let near_start = lamp_warp_visual_point(group, source_point, 1.0e-9);
            assert!((near_start[0] - source_point[0]).abs() < 0.01);
            assert!((near_start[1] - source_point[1]).abs() < 0.01);

            let normalized = [
                (source_point[0] - group.presented_source_visual_rect.x())
                    / group.presented_source_visual_rect.width(),
                (source_point[1] - group.presented_source_visual_rect.y())
                    / group.presented_source_visual_rect.height(),
            ];
            let endpoint = lamp_warp_visual_point(group, source_point, 1.0);
            let expected_endpoint = [
                group.sink_rect.x() + normalized[0] * group.sink_rect.width(),
                group.sink_rect.y() + normalized[1] * group.sink_rect.height(),
            ];
            assert_eq!(endpoint, expected_endpoint);
        }

        assert!(LAMP_VERTEX_SHADER.contains(
            "u_source_visual_rect.xy\n        + ((a_position - u_canonical_visual_rect.xy)"
        ));
        assert!(LAMP_VERTEX_SHADER.contains("uniform vec4 u_canonical_visual_rect;"));
        assert!(LAMP_VERTEX_SHADER.contains("uniform vec4 u_sink_rect;"));
        assert!(LAMP_VERTEX_SHADER.contains("movement_extent(u_source_visual_rect)"));
        assert!(LAMP_VERTEX_SHADER.contains("axis_position(u_source_visual_rect"));
        assert!(LAMP_VERTEX_SHADER.contains("cross_position(u_source_visual_rect"));
        assert!(LAMP_VERTEX_SHADER.contains("axis_position(u_sink_rect"));
        assert!(LAMP_VERTEX_SHADER.contains("cross_position(u_sink_rect"));
        assert!(LAMP_VERTEX_SHADER.contains("float cubic_funnel_profile"));
        assert!(LAMP_VERTEX_SHADER.contains("3.0 * u * u * t * c1"));
        assert!(LAMP_VERTEX_SHADER.contains("LAMP_RAIL_C1_LOW_SHAPE = 0.14"));
        assert!(LAMP_VERTEX_SHADER.contains("LAMP_RAIL_C1_HIGH_SHAPE = 0.04"));
        assert!(LAMP_VERTEX_SHADER.contains("LAMP_RAIL_C2_LOW_SHAPE = 0.55"));
        assert!(LAMP_VERTEX_SHADER.contains("LAMP_RAIL_C2_HIGH_SHAPE = 0.24"));
        for uniform in [
            "uniform float u_progress;",
            "uniform float u_contraction_progress;",
            "uniform float u_translation_progress;",
            "uniform float u_retreat_progress;",
        ] {
            assert!(
                LAMP_VERTEX_SHADER.contains(uniform),
                "missing Lamp uniform {uniform}"
            );
        }
        for obsolete_uniform in ["u_bump_progress", "u_stretch_progress", "u_squash_progress"] {
            assert!(
                !LAMP_VERTEX_SHADER.contains(obsolete_uniform),
                "obsolete sequential uniform remains: {obsolete_uniform}"
            );
        }
        assert!(!LAMP_VERTEX_SHADER.contains("smoothstep"));
        assert!(!LAMP_VERTEX_SHADER.contains("in_out_cubic"));
        assert!(!LAMP_VERTEX_SHADER.contains("u_source_rect"));
        assert!(!LAMP_VERTEX_SHADER.contains("u_full_window_rect"));
        assert!(!LAMP_VERTEX_SHADER.contains("u_anchor_rect"));
        assert!(!LAMP_VERTEX_SHADER.contains("u_portal_rect"));
        assert!(!LAMP_VERTEX_SHADER.contains("spatial_funnel_exponent"));
        assert!(!LAMP_VERTEX_SHADER.contains("LAMP_SPATIAL_EXPONENT"));

        let expected_rails = [
            (0.00, 0.0, 0.0, 0.0),
            (0.50, 0.38375, 0.306875, 0.23),
            (1.00, 1.0, 1.0, 1.0),
        ];
        for (t, expected_min, expected_middle, expected_max) in expected_rails {
            assert!((cubic_funnel_profile(t, 0.20) - expected_min).abs() < 1.0e-12);
            assert!((cubic_funnel_profile(t, 0.50) - expected_middle).abs() < 1.0e-12);
            assert!((cubic_funnel_profile(t, 0.80) - expected_max).abs() < 1.0e-12);
        }
    }
}

const CAPTURE_VERTEX_SHADER: &str = r#"#version 300 es
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_uv;
uniform vec2 u_capture_output_size;
uniform vec4 u_capture_domain;
uniform int u_capture_origin_bottom_left;
out vec2 v_uv;

void main() {
    vec2 output_pixel = vec2(
        (a_position.x + 1.0) * 0.5 * u_capture_output_size.x,
        u_capture_origin_bottom_left != 0
            ? (1.0 - a_position.y) * 0.5 * u_capture_output_size.y
            : (a_position.y + 1.0) * 0.5 * u_capture_output_size.y);
    vec2 local = (output_pixel - u_capture_domain.xy) / u_capture_domain.zw;
    gl_Position = vec4(local.x * 2.0 - 1.0, 1.0 - local.y * 2.0, 0.0, 1.0);
    v_uv = a_uv;
}
"#;
