use std::io;

use glow::HasContext;

use super::{GlProgram, RendererResult};

pub(super) fn create_texture_program(gl: &glow::Context) -> RendererResult<GlProgram> {
    let vertex_shader = compile_shader(gl, glow::VERTEX_SHADER, EGL_VERTEX_SHADER)?;
    let fragment_shader = compile_shader(gl, glow::FRAGMENT_SHADER, EGL_FRAGMENT_SHADER)?;
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
