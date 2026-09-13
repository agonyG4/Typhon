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
in vec2 v_uv;
out vec4 out_color;

void main() {
    out_color = texture(u_texture, v_uv);
}
"#;

const LAMP_VERTEX_SHADER: &str = r#"#version 300 es
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_uv;
out vec2 v_uv;

uniform vec2 u_output_size;
uniform vec4 u_source_rect;
uniform vec4 u_source_visual_rect;
uniform vec4 u_full_window_rect;
uniform vec4 u_anchor_rect;
uniform float u_progress;
uniform int u_direction;
uniform float u_shape_factor;
uniform float u_bump_distance;
uniform float u_bump_progress;
uniform float u_stretch_progress;
uniform float u_squash_progress;
uniform int u_framebuffer_origin_bottom_left;

// Keep these values in lockstep with the documented Astrea Lamp constants in
// window_lifecycle_animation.rs; the shader mirrors the CPU reference math.
const float LAMP_BUMP_WEIGHT = 0.12;
const float LAMP_STRETCH_WEIGHT = 0.70;
const float LAMP_SQUASH_WEIGHT = 1.0;
const float LAMP_SHAPE_MIN = 0.20;
const float LAMP_SHAPE_MAX = 0.80;
const float LAMP_NEAR_EDGE_BIAS = 0.18;
const float LAMP_NECK_BASE = 0.20;
const float LAMP_NECK_RANGE = 0.80;

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

void main() {
    vec2 source_point = u_source_rect.xy
        + ((a_position - u_full_window_rect.xy) / u_full_window_rect.zw)
        * u_source_rect.zw;
    float progress = clamp(u_progress, 0.0, 1.0);
    vec2 group_uv = clamp(
        (source_point - u_source_visual_rect.xy) / u_source_visual_rect.zw,
        0.0,
        1.0
    );
    vec2 target = u_anchor_rect.xy + group_uv * u_anchor_rect.zw;
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
        float bump_fraction = u_bump_distance > 0.000001 ? LAMP_BUMP_WEIGHT / (
            LAMP_BUMP_WEIGHT + LAMP_STRETCH_WEIGHT
                * clamp(u_shape_factor, LAMP_SHAPE_MIN, LAMP_SHAPE_MAX)
                + LAMP_SQUASH_WEIGHT
        ) : 0.0;
        float stretch_weight = LAMP_STRETCH_WEIGHT
            * clamp(u_shape_factor, LAMP_SHAPE_MIN, LAMP_SHAPE_MAX);
        float total_weight = (u_bump_distance > 0.000001 ? LAMP_BUMP_WEIGHT : 0.0)
            + stretch_weight + LAMP_SQUASH_WEIGHT;
        float stretch_fraction = stretch_weight / total_weight;
        float base_motion = clamp(
            bump_fraction * u_bump_progress
                + stretch_fraction * u_stretch_progress,
            0.0,
            1.0
        );
        float bump_ratio = clamp(u_bump_distance / movement_extent(u_source_rect), 0.0, 1.0);
        float biased_motion = clamp(
            base_motion
                + (movement_normalized - 0.5)
                    * bump_ratio * LAMP_NEAR_EDGE_BIAS * u_bump_progress
                    * (1.0 - base_motion),
            0.0,
            1.0
        );
        float source_axis = axis_position(u_source_rect, movement_normalized);
        float target_axis = axis_position(u_anchor_rect, movement_normalized);
        float pre_squash_axis = mix(source_axis, target_axis, biased_motion);
        float axis = mix(pre_squash_axis, target_axis, clamp(u_squash_progress, 0.0, 1.0));
        float source_cross = cross_position(u_source_rect, cross_normalized);
        float target_cross = cross_position(u_anchor_rect, cross_normalized);
        float source_cross_center = cross_position(u_source_rect, 0.5);
        float neck_scale = clamp(
            1.0 - clamp(u_shape_factor, LAMP_SHAPE_MIN, LAMP_SHAPE_MAX)
                * clamp(u_stretch_progress, 0.0, 1.0)
                * (LAMP_NECK_BASE + LAMP_NECK_RANGE * movement_normalized),
            0.05,
            1.0
        );
        float neck_candidate = source_cross_center
            + (source_cross - source_cross_center) * neck_scale;
        float stretched_cross = abs(target_cross - neck_candidate)
                <= abs(target_cross - source_cross)
            ? neck_candidate
            : source_cross;
        float cross_motion = clamp(
            biased_motion + u_squash_progress,
            0.0,
            1.0
        );
        float cross = mix(stretched_cross, target_cross, cross_motion);
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
