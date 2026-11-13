//! The hand-written shaders against every backend's translation (see
//! [`thedas_shader::backends`]): each module with shaders of its own has a test that calls
//! this for them.

/// Panics unless `wgsl` (called `name`) can be drawn by every backend.
pub fn translates(name: &str, wgsl: &str) {
    if let Err(e) = thedas_shader::backends::check_wgsl(wgsl) {
        panic!("{name} cannot be drawn everywhere: {e}");
    }
}
