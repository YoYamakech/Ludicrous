// gl.rs: thin helpers around glow. Shader compilation and two vertex batches
// (one for rectangles, one for glyph quads). Both batches are instanced:
// one unit quad is uploaded once, then one "instance" per rectangle/glyph.

use glow::HasContext;

/// Compile and link a vertex + fragment shader pair.
pub fn program(gl: &glow::Context, vs_src: &str, fs_src: &str) -> glow::Program {
    unsafe {
        let vs = gl
            .create_shader(glow::VERTEX_SHADER)
            .expect("create shader");
        gl.shader_source(vs, vs_src);
        gl.compile_shader(vs);
        if !gl.get_shader_compile_status(vs) {
            panic!("vertex shader error:\n{}", gl.get_shader_info_log(vs));
        }
        let fs = gl
            .create_shader(glow::FRAGMENT_SHADER)
            .expect("create shader");
        gl.shader_source(fs, fs_src);
        gl.compile_shader(fs);
        if !gl.get_shader_compile_status(fs) {
            panic!("fragment shader error:\n{}", gl.get_shader_info_log(fs));
        }
        let p = gl.create_program().expect("create program");
        gl.attach_shader(p, vs);
        gl.attach_shader(p, fs);
        gl.link_program(p);
        if !gl.get_program_link_status(p) {
            panic!("link error:\n{}", gl.get_program_info_log(p));
        }
        gl.detach_shader(p, vs);
        gl.detach_shader(p, fs);
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        p
    }
}

/// The unit quad shared by both batches: two triangles, counter-clockwise.
/// Each vertex is (corner.x, corner.y) where 0,0 is the top-left of the shape.
const QUAD: [f32; 12] = [
    0.0, 0.0, 1.0, 0.0, 1.0, 1.0, // top-left, top-right, bottom-right
    0.0, 0.0, 1.0, 1.0, 0.0, 1.0, // top-left, bottom-right, bottom-left
];

/// One VAO holding the unit quad (location 0) plus a per-instance buffer
/// (locations 1..). `data` is the CPU-side instance data for the current frame.
pub struct Batch {
    vao: glow::VertexArray,
    inst: glow::Buffer,
    pub data: Vec<f32>,
    stride_floats: usize,
    max_instances: usize,
}

impl Batch {
    /// `attributes` lists (location, size) for each per-instance attribute, in order.
    pub fn new(gl: &glow::Context, attributes: &[(u32, i32)], max_instances: usize) -> Self {
        unsafe {
            let vao = gl.create_vertex_array().expect("vao");
            gl.bind_vertex_array(Some(vao));

            // Unit quad, static.
            let vbo = gl.create_buffer().expect("vbo");
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let bytes: &[u8] = std::slice::from_raw_parts(
                QUAD.as_ptr() as *const u8,
                std::mem::size_of_val(&QUAD),
            );
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);

            // Instance buffer, allocated once at the maximum size.
            let inst = gl.create_buffer().expect("inst");
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(inst));
            let stride_floats: usize = attributes.iter().map(|a| a.1 as usize).sum();
            gl.buffer_data_size(
                glow::ARRAY_BUFFER,
                (stride_floats * max_instances * 4) as i32,
                glow::DYNAMIC_DRAW,
            );
            let mut offset = 0;
            for (loc, size) in attributes {
                gl.enable_vertex_attrib_array(*loc);
                gl.vertex_attrib_pointer_f32(
                    *loc,
                    *size,
                    glow::FLOAT,
                    false,
                    (stride_floats * 4) as i32,
                    offset,
                );
                gl.vertex_attrib_divisor(*loc, 1);
                offset += size * 4;
            }
            gl.bind_vertex_array(None);
            Self {
                vao,
                inst,
                data: Vec::new(),
                stride_floats,
                max_instances,
            }
        }
    }

    /// Append one instance. Silently drops data past the buffer capacity so a
    /// pathological frame can never write out of bounds.
    pub fn push(&mut self, values: &[f32]) {
        if self.len() < self.max_instances {
            self.data.extend_from_slice(values);
        }
    }

    pub fn len(&self) -> usize {
        self.data.len() / self.stride_floats
    }

    /// Upload the CPU-side data and draw all instances in a single call.
    pub fn draw(&mut self, gl: &glow::Context) {
        if self.data.is_empty() {
            return;
        }
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.inst));
            let bytes: &[u8] =
                std::slice::from_raw_parts(self.data.as_ptr() as *const u8, self.data.len() * 4);
            gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytes);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, self.len() as i32);
            gl.bind_vertex_array(None);
        }
        self.data.clear();
    }
}
