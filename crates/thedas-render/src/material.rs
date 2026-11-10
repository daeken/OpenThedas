//! Material programs: how a [`SceneMaterial`] is drawn.
//!
//! A [`ProgramSource`] maps a material to a [`MaterialProgram`] (a WGSL module plus fixed
//! function state). The renderer asks its sources in order and uses the first answer, so a
//! translated original shader can take over one material at a time by registering a source in
//! front of the stand-in: see [`ProgramRegistry::push_front`].
//!
//! Every program in this milestone uses the [`bindings`] interface. The second half of M1,
//! which translates the original D3D9 Static1 shaders into naga, can either adopt it or add
//! its own bind group layout per program.

use std::borrow::Cow;

use crate::scene::SceneMaterial;

pub mod bindings {
    //! The bind group interface shared by the stand-in programs.
    //!
    //! - group 0: frame uniforms (`FrameUniforms`)
    //! - group 1: material: diffuse texture at binding 0, sampler at binding 1
    //! - group 2: lightmap: atlas texture at binding 0, sampler at binding 1
    //! - group 3: draw uniforms (`DrawUniforms`), dynamic offset
    //!
    //! Vertex buffer 0 is [`crate::scene::Vertex`]: position, normal, uv0, uv1.

    pub const FRAME: u32 = 0;
    pub const MATERIAL: u32 = 1;
    pub const LIGHTMAP: u32 = 2;
    pub const DRAW: u32 = 3;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlendMode {
    Opaque,
    /// Discard fragments whose alpha is below the threshold; depth is written.
    AlphaTest(f32),
    /// Standard alpha blending, drawn after opaque geometry without depth writes.
    AlphaBlend,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialProgram {
    /// Pipeline cache key and debug label; programs with equal names must be identical.
    pub name: String,
    pub wgsl: Cow<'static, str>,
    pub vs_entry: &'static str,
    pub fs_entry: &'static str,
    pub blend: BlendMode,
    /// `false` draws both faces.
    pub cull_back: bool,
}

pub trait ProgramSource: Send + Sync {
    /// The program for a material, or `None` to let the next source decide.
    fn program(&self, material: &SceneMaterial) -> Option<MaterialProgram>;
}

/// What a material resolves to after asking every source.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolved {
    Draw(MaterialProgram),
    /// Not drawn (for example effects the stand-in does not cover).
    Skip,
}

pub struct ProgramRegistry {
    sources: Vec<Box<dyn ProgramSource>>,
}

impl Default for ProgramRegistry {
    fn default() -> Self {
        Self {
            sources: vec![Box::new(StandIn)],
        }
    }
}

impl ProgramRegistry {
    /// Adds a source that takes precedence over the existing ones.
    pub fn push_front(&mut self, source: Box<dyn ProgramSource>) {
        self.sources.insert(0, source);
    }

    pub fn resolve(&self, material: &SceneMaterial) -> Resolved {
        self.sources
            .iter()
            .find_map(|s| s.program(material))
            .map_or(Resolved::Skip, Resolved::Draw)
    }
}

/// Hand-written stand-in: diffuse texture times lightmap. Not the original look; it exists so
/// the area can be walked before the original shaders are translated.
pub struct StandIn;

pub const STAND_IN_WGSL: &str = include_str!("standin.wgsl");

impl ProgramSource for StandIn {
    fn program(&self, m: &SceneMaterial) -> Option<MaterialProgram> {
        let material = m.material_name();
        let semantic = m.semantic.to_ascii_lowercase();
        let class = m.render_class.as_deref().unwrap_or("").to_ascii_lowercase();
        // Effects (fire cards, haze, distortion) are out of scope for the stand-in.
        let effect = material.starts_with("vfx")
            || material.contains("distortion")
            || class.contains("distortion")
            || class.contains("vfx");
        if effect {
            return None;
        }
        let (fs, name) = if material == "black.mat" {
            ("fs_black", "standin-black")
        } else {
            ("fs_main", "standin")
        };
        let blend = if semantic.contains("punch") {
            BlendMode::AlphaTest(0.5)
        } else if semantic.contains("blend") || class.contains("transparent") {
            BlendMode::AlphaBlend
        } else {
            BlendMode::Opaque
        };
        let suffix = match blend {
            BlendMode::Opaque => "opaque",
            BlendMode::AlphaTest(_) => "punch",
            BlendMode::AlphaBlend => "blend",
        };
        Some(MaterialProgram {
            name: format!("{name}-{suffix}"),
            wgsl: Cow::Borrowed(STAND_IN_WGSL),
            vs_entry: "vs_main",
            fs_entry: if matches!(blend, BlendMode::AlphaTest(_)) && fs == "fs_main" {
                "fs_punch"
            } else {
                fs
            },
            blend,
            // Winding is not verified against the original yet (see the .sta render states),
            // so the stand-in draws both faces.
            cull_back: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thedas_formats::MaterialObject;

    fn material(mat: &str, semantic: &str, class: Option<&str>) -> SceneMaterial {
        let xml = format!(
            r#"<MaterialObject Name="t"><Material Name="{mat}"/><DefaultSemantic Name="{semantic}"/></MaterialObject>"#
        );
        SceneMaterial {
            mao: "t.mao".into(),
            object: MaterialObject::parse(xml.as_bytes()).unwrap(),
            definition: None,
            semantic: semantic.into(),
            render_class: class.map(str::to_string),
            technique: None,
        }
    }

    #[test]
    fn stand_in_classifies_materials() {
        let reg = ProgramRegistry::default();
        let prog = |m| match reg.resolve(&m) {
            Resolved::Draw(p) => Some((p.name, p.blend)),
            Resolved::Skip => None,
        };
        assert_eq!(
            prog(material("static.mat", "Default", Some("Opaque,Static1"))),
            Some(("standin-opaque".into(), BlendMode::Opaque))
        );
        assert_eq!(
            prog(material("static.mat", "LOD1Punch", None)).unwrap().1,
            BlendMode::AlphaTest(0.5)
        );
        assert_eq!(
            prog(material(
                "static.mat",
                "LOD1Blend",
                Some("Transparent,Static1")
            ))
            .unwrap()
            .1,
            BlendMode::AlphaBlend
        );
        assert_eq!(
            prog(material("black.mat", "Default", None)).unwrap().0,
            "standin-black-opaque"
        );
        assert_eq!(prog(material("vfx.mat", "Addv", None)), None);
    }

    #[test]
    fn earlier_sources_win() {
        struct Translated;
        impl ProgramSource for Translated {
            fn program(&self, m: &SceneMaterial) -> Option<MaterialProgram> {
                (m.material_name() == "static.mat").then(|| MaterialProgram {
                    name: "translated-static1".into(),
                    wgsl: Cow::Borrowed(""),
                    vs_entry: "vs",
                    fs_entry: "fs",
                    blend: BlendMode::Opaque,
                    cull_back: true,
                })
            }
        }
        let mut reg = ProgramRegistry::default();
        reg.push_front(Box::new(Translated));
        let name = |m| match reg.resolve(&m) {
            Resolved::Draw(p) => p.name,
            Resolved::Skip => "skip".into(),
        };
        assert_eq!(
            name(material("static.mat", "Default", None)),
            "translated-static1"
        );
        assert_eq!(
            name(material("prop.mat", "Default", None)),
            "standin-opaque"
        );
    }
}
