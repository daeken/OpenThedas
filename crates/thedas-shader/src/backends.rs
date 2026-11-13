//! Checks that a shader can be drawn by each of wgpu's backends, without their hardware.
//!
//! A backend translates a shader when a pipeline is made: to HLSL for Direct3D 12, SPIR-V
//! for Vulkan, GLSL for OpenGL (a machine with none of the others draws with OpenGL: a
//! Windows virtual machine, an old card), and refuses what its language cannot express.
//! A shader can work on the machine it was written on and stop the game on another. These
//! run the same translators here. They do not run the drivers' own compilers (Microsoft's
//! HLSL compiler, a GLSL driver), which can still refuse what the translators wrote.

use naga::back::{glsl, hlsl, spv};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Backend {
    Direct3D12,
    Vulkan,
    OpenGl,
}

/// What each backend refuses of a validated module: empty when all take it. OpenGL is
/// tried as desktop 3.3 and as ES 3.0, the oldest wgpu uses.
pub fn failures(module: &naga::Module, info: &naga::valid::ModuleInfo) -> Vec<(Backend, String)> {
    let mut out = Vec::new();
    let mut text = String::new();
    let options = hlsl::Options::default();
    let pipeline = hlsl::PipelineOptions { entry_point: None };
    if let Err(e) = hlsl::Writer::new(&mut text, &options, &pipeline).write(module, info, None) {
        out.push((Backend::Direct3D12, e.to_string()));
    }
    if let Err(e) = spv::write_vec(module, info, &spv::Options::default(), None) {
        out.push((Backend::Vulkan, e.to_string()));
    }
    let versions = [
        glsl::Version::Desktop(330),
        glsl::Version::Embedded {
            version: 300,
            is_webgl: false,
        },
    ];
    'gl: for version in versions {
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
            let mut text = String::new();
            let written = glsl::Writer::new(
                &mut text,
                module,
                info,
                &options,
                &pipeline,
                naga::proc::BoundsCheckPolicies::default(),
            )
            .and_then(|mut w| w.write().map(|_| ()));
            if let Err(e) = written {
                out.push((
                    Backend::OpenGl,
                    format!("{} for {version:?}: {e}", entry.name),
                ));
                break 'gl;
            }
        }
    }
    out
}

/// Checks WGSL text: an error naming what refused it (the parser, the validator, or the
/// backends).
pub fn check_wgsl(wgsl: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| e.to_string())?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| format!("{e:?}"))?;
    let refused = failures(&module, &info);
    if refused.is_empty() {
        Ok(())
    } else {
        Err(refused
            .iter()
            .map(|(b, e)| format!("{b:?}: {e}"))
            .collect::<Vec<_>>()
            .join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_a_texel_of_a_depth_texture_is_what_opengl_refuses() {
        // What the depth of field's shader did; it binds the depth buffer as plain numbers
        // now.
        let refused = "@group(0) @binding(0) var depth: texture_depth_2d;
            @fragment fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
                return vec4<f32>(textureLoad(depth, vec2<i32>(at.xy), 0));
            }";
        let error = check_wgsl(refused).unwrap_err();
        assert!(
            error.starts_with("OpenGl") && error.contains("depth"),
            "{error}"
        );
        let taken = refused
            .replace("texture_depth_2d", "texture_2d<f32>")
            .replace("0));", "0).x);");
        assert_eq!(check_wgsl(&taken), Ok(()));
    }
}
