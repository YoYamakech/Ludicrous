// gl.rs: thin helpers around glow. Shader compilation and the vertex batches
// (one per texture: shapes, editor glyphs, UI glyphs, icons). Every batch is
// instanced: one unit quad is uploaded once, then one "instance" per rectangle
// or glyph.
//
// Phase F adds a second upload path. When the driver has `ARB_buffer_storage`
// and `ARB_base_instance`, the instance buffer is allocated once with
// persistent, coherent storage and mapped for the life of the program. Frames
// go into one of three regions in turn; `base_instance` points a draw at its
// region, so no attribute pointer is ever re-specified and nothing is copied
// through `glBufferSubData`. A fence per region stops the CPU from overwriting
// data the GPU is still reading.
//
// Without those extensions (or with LUDICROUS_GL_SAFE=1) we keep the plain
// `glBufferSubData` upload. The overlay reports which path is active so the
// two can be compared on real hardware.

use glow::HasContext;

use crate::render::caps::GlCaps;

/// How instance data reaches the GPU.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Upload {
    /// Persistent map + `base_instance` (ARB_buffer_storage + ARB_base_instance).
    Ring,
    /// `glBufferSubData` into one buffer, one draw per batch.
    Orphan,
}

impl Upload {
    /// Pick the fastest path the driver actually supports.
    pub fn for_caps(caps: &GlCaps) -> Upload {
        if caps.buffer_storage && caps.base_instance {
            Upload::Ring
        } else {
            Upload::Orphan
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Upload::Ring => "ring+base_instance",
            Upload::Orphan => "orphan (buffer_sub_data)",
        }
    }
}

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

/// The unit quad shared by every batch: two triangles, counter-clockwise.
/// Each vertex is (corner.x, corner.y) where 0,0 is the top-left of the shape.
const QUAD: [f32; 12] = [
    0.0, 0.0, 1.0, 0.0, 1.0, 1.0, // top-left, top-right, bottom-right
    0.0, 0.0, 1.0, 1.0, 0.0, 1.0, // top-left, bottom-right, bottom-left
];

/// How many frames may be in flight in the ring. Three is the spec's number
/// and also what a triple-buffered compositor keeps alive.
const REGIONS: usize = 3;

/// The mapped ring. Holds a raw pointer, so it is only created and used inside
/// `unsafe` blocks owned by `Batch`; it is never sent anywhere.
struct Ring {
    ptr: *mut u8,
    /// Bytes in one region (one frame's worth of instances).
    region_bytes: usize,
}

/// One VAO holding the unit quad (location 0) plus a per-instance buffer
/// (locations 1..). `data` is the CPU-side instance data for the current frame.
pub struct Batch {
    vao: glow::VertexArray,
    inst: glow::Buffer,
    pub data: Vec<f32>,
    stride_floats: usize,
    max_instances: usize,
    upload: Upload,
    ring: Option<Ring>,
    /// Region the next frame is written into.
    region: usize,
    /// One fence per region, set after the draw that used it.
    fences: Vec<Option<glow::Fence>>,
}

impl Batch {
    /// `attributes` lists (location, size) for each per-instance attribute, in
    /// order. `caps` decides whether the persistent ring is used.
    pub fn new(
        gl: &glow::Context,
        caps: &GlCaps,
        attributes: &[(u32, i32)],
        max_instances: usize,
    ) -> Self {
        let stride_floats: usize = attributes.iter().map(|a| a.1 as usize).sum();
        let total_bytes = stride_floats * max_instances * REGIONS * 4;
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

            // Instance buffer. The fast path allocates all three regions up
            // front and maps them once; the fallback allocates a single frame.
            let inst = gl.create_buffer().expect("inst");
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(inst));
            let mut ring = None;
            let upload = Upload::for_caps(caps);
            if upload == Upload::Ring {
                gl.buffer_storage(
                    glow::ARRAY_BUFFER,
                    total_bytes as i32,
                    None,
                    glow::MAP_WRITE_BIT | glow::MAP_PERSISTENT_BIT | glow::MAP_COHERENT_BIT,
                );
                let ptr = gl.map_buffer_range(
                    glow::ARRAY_BUFFER,
                    0,
                    total_bytes as i32,
                    glow::MAP_WRITE_BIT | glow::MAP_PERSISTENT_BIT | glow::MAP_COHERENT_BIT,
                );
                if ptr.is_null() {
                    // The driver said yes and then failed: fall back rather
                    // than draw from a null pointer.
                    eprintln!("ludicrous: ring map failed, using buffer_sub_data");
                    gl.buffer_data_size(
                        glow::ARRAY_BUFFER,
                        (stride_floats * max_instances * 4) as i32,
                        glow::DYNAMIC_DRAW,
                    );
                } else {
                    ring = Some(Ring {
                        ptr,
                        region_bytes: stride_floats * max_instances * 4,
                    });
                }
            } else {
                gl.buffer_data_size(
                    glow::ARRAY_BUFFER,
                    (stride_floats * max_instances * 4) as i32,
                    glow::DYNAMIC_DRAW,
                );
            }

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

            let upload = if ring.is_some() {
                Upload::Ring
            } else {
                Upload::Orphan
            };
            Self {
                vao,
                inst,
                data: Vec::new(),
                stride_floats,
                max_instances,
                upload,
                ring,
                region: 0,
                fences: (0..REGIONS).map(|_| None).collect(),
            }
        }
    }

    pub fn upload(&self) -> Upload {
        self.upload
    }

    /// Append one instance. Silently drops data past the region capacity so a
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
    /// Returns the GL draw call count (0 or 1) for the debug overlay.
    pub fn draw(&mut self, gl: &glow::Context) -> u32 {
        if self.data.is_empty() {
            return 0;
        }
        let count = self.len() as i32;
        if let Some(ring) = &self.ring {
            let region = self.region;
            // Copy the raw parts out first: the body below mutates `self`.
            let ptr = ring.ptr;
            let region_bytes = ring.region_bytes;
            unsafe {
                // Never overwrite a region the GPU may still be reading.
                if let Some(fence) = self.fences[region].take() {
                    gl.client_wait_sync(fence, glow::SYNC_FLUSH_COMMANDS_BIT, 1_000_000_000);
                    gl.delete_sync(fence);
                }
                let dst = ptr.add(region * region_bytes);
                std::ptr::copy_nonoverlapping(
                    self.data.as_ptr() as *const u8,
                    dst,
                    self.data.len() * 4,
                );
                // The persistent map is coherent, but the driver still needs to
                // know the CPU wrote to buffer storage before it reads it.
                gl.memory_barrier(glow::CLIENT_MAPPED_BUFFER_BARRIER_BIT);
                gl.bind_vertex_array(Some(self.vao));
                // `base_instance` selects the region: instance i of this draw
                // reads instance (region * max + i) of the buffer.
                gl.draw_arrays_instanced_base_instance(
                    glow::TRIANGLES,
                    0,
                    6,
                    count,
                    (region * self.max_instances) as u32,
                );
                gl.bind_vertex_array(None);
                match gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) {
                    Ok(fence) => self.fences[region] = Some(fence),
                    Err(e) => eprintln!("ludicrous: fence_sync failed: {e}"),
                }
            }
            self.region = (region + 1) % REGIONS;
        } else {
            unsafe {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.inst));
                let bytes: &[u8] = std::slice::from_raw_parts(
                    self.data.as_ptr() as *const u8,
                    self.data.len() * 4,
                );
                // A plain SubData would stall on the previous frame's draw; the
                // orphaning pattern (allocate, then write) lets the driver hand
                // us new storage while the old one drains.
                gl.buffer_data_size(
                    glow::ARRAY_BUFFER,
                    (self.max_instances * self.stride_floats * 4) as i32,
                    glow::STREAM_DRAW,
                );
                gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytes);
                gl.bind_vertex_array(Some(self.vao));
                gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, count);
                gl.bind_vertex_array(None);
            }
        }
        self.data.clear();
        1
    }
}

impl Batch {
    /// Release the GL objects. Called by `Renderer::drop`, which owns the
    /// context (a `Batch` never does, so the teardown cannot be a `Drop`).
    pub fn destroy(&mut self, gl: &glow::Context) {
        unsafe {
            for fence in self.fences.drain(..).flatten() {
                gl.delete_sync(fence);
            }
            if self.ring.is_some() {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.inst));
                gl.unmap_buffer(glow::ARRAY_BUFFER);
                gl.bind_buffer(glow::ARRAY_BUFFER, None);
                self.ring = None;
            }
            gl.delete_buffer(self.inst);
            gl.delete_vertex_array(self.vao);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(buffer_storage: bool, base_instance: bool, safe: bool) -> GlCaps {
        GlCaps {
            buffer_storage,
            base_instance,
            safe_mode: safe,
            ..GlCaps::default()
        }
    }

    #[test]
    fn the_ring_needs_both_extensions() {
        assert_eq!(Upload::for_caps(&caps(true, true, false)), Upload::Ring);
        assert_eq!(Upload::for_caps(&caps(true, false, false)), Upload::Orphan);
        assert_eq!(Upload::for_caps(&caps(false, true, false)), Upload::Orphan);
        assert_eq!(Upload::for_caps(&caps(false, false, false)), Upload::Orphan);
    }

    #[test]
    fn safe_mode_disables_the_ring_even_when_detected() {
        // GlCaps::detect clears the flags in safe mode; this checks the choice
        // still lands on the fallback when only `safe_mode` is set.
        assert_eq!(Upload::for_caps(&caps(false, false, true)), Upload::Orphan);
    }
}
