//! Checks that a hand-written WGSL shader can be drawn by wgpu's OpenGL backend, which
//! translates it to GLSL when a pipeline is made and refuses what GLSL cannot express (a
//! machine with neither Direct3D 12, Vulkan nor Metal draws with OpenGL: a Windows
//! virtual machine, for one). The backends for the others take all of WGSL, so a shader
//! can work everywhere else and still stop the game there.

use naga::back::glsl;

/// Translates every entry point of `wgsl` for desktop OpenGL 3.3 and OpenGL ES 3.0, the
/// oldest the backend uses.
pub fn translate(wgsl: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| e.to_string())?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| format!("{e:?}"))?;
    let versions = [
        glsl::Version::Desktop(330),
        glsl::Version::Embedded {
            version: 300,
            is_webgl: false,
        },
    ];
    for version in versions {
        for entry in &module.entry_points {
            let options = glsl::Options {
                version,
                ..Default::default()
            };
            let pipeline = glsl::PipelineOptions {
                shader_stage: entry.stage,
                entry_point: entry.name.clone(),
                multiview: None,
            };
            let mut out = String::new();
            let mut writer = glsl::Writer::new(
                &mut out,
                &module,
                &info,
                &options,
                &pipeline,
                naga::proc::BoundsCheckPolicies::default(),
            )
            .map_err(|e| format!("{} for {version:?}: {e}", entry.name))?;
            writer
                .write()
                .map_err(|e| format!("{} for {version:?}: {e}", entry.name))?;
        }
    }
    Ok(())
}

/// Panics unless `wgsl` (called `name`) translates.
pub fn translates(name: &str, wgsl: &str) {
    if let Err(e) = translate(wgsl) {
        panic!("{name} cannot be drawn with OpenGL: {e}");
    }
}

#[test]
fn reading_a_texel_of_a_depth_texture_is_what_opengl_refuses() {
    // What the depth of field's shader did; it now binds the depth buffer as plain numbers.
    let refused = "@group(0) @binding(0) var depth: texture_depth_2d;
        @fragment fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
            return vec4<f32>(textureLoad(depth, vec2<i32>(at.xy), 0));
        }";
    let error = translate(refused).unwrap_err();
    assert!(error.contains("depth"), "{error}");
    let taken = refused
        .replace("texture_depth_2d", "texture_2d<f32>")
        .replace("0));", "0).x);");
    assert_eq!(translate(&taken), Ok(()));
}
