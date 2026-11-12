// caps.rs: ask the driver what it supports, once, at startup.
//
// We never require an extension. Every optional path has a core-3.3 fallback,
// so the editor starts on any GL 3.3 driver. `LUDICROUS_GL_SAFE=1` turns all
// optional paths off, which is useful when a driver advertises something it
// cannot actually do.

use glow::HasContext;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, Default)]
pub struct GlCaps {
    pub buffer_storage: bool,
    pub base_instance: bool,
    pub texture_storage: bool,
    pub framebuffer_srgb: bool,
    pub direct_state_access: bool,
    pub invalidate_subdata: bool,
    pub context_flush_control: bool,
    pub timer_query: bool,
    pub debug_output: bool,
    /// True when LUDICROUS_GL_SAFE=1 disabled every optional path.
    #[allow(dead_code)] // read by the Phase F fast paths
    pub safe_mode: bool,
}

impl GlCaps {
    pub fn detect(gl: &glow::Context) -> Self {
        let safe_mode = std::env::var("LUDICROUS_GL_SAFE")
            .map(|v| v == "1")
            .unwrap_or(false);

        // `supported_extensions` is the driver's own list. Checking names is
        // more reliable than comparing version numbers, which say nothing
        // about which extensions a particular driver actually implements.
        let ext: &HashSet<String> = gl.supported_extensions();
        let has = |name: &str| !safe_mode && ext.contains(name);

        let caps = Self {
            buffer_storage: has("GL_ARB_buffer_storage"),
            base_instance: has("GL_ARB_base_instance"),
            texture_storage: has("GL_ARB_texture_storage"),
            framebuffer_srgb: has("GL_ARB_framebuffer_sRGB"),
            direct_state_access: has("GL_ARB_direct_state_access"),
            invalidate_subdata: has("GL_ARB_invalidate_subdata"),
            context_flush_control: has("GL_KHR_context_flush_control"),
            timer_query: has("GL_ARB_timer_query"),
            debug_output: has("GL_KHR_debug"),
            safe_mode,
        };

        unsafe {
            let version = gl.get_parameter_string(glow::VERSION);
            let renderer = gl.get_parameter_string(glow::RENDERER);
            eprintln!("ludicrous: GL {version} ({renderer})");
        }
        eprintln!(
            "ludicrous: optional paths{}: buffer_storage={} base_instance={} \
             texture_storage={} srgb={} dsa={} invalidate={} flush_control={} \
             timer_query={} debug={}",
            if safe_mode { " (safe mode)" } else { "" },
            caps.buffer_storage,
            caps.base_instance,
            caps.texture_storage,
            caps.framebuffer_srgb,
            caps.direct_state_access,
            caps.invalidate_subdata,
            caps.context_flush_control,
            caps.timer_query,
            caps.debug_output,
        );
        caps
    }
}
