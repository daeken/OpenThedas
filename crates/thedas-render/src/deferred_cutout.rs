//! Point-shadow coverage for known forward cutouts, independent of G-buffer coverage.

use crate::{
    d3d_gpu::D3dPass,
    deferred_mesh::DeferredVertex,
    gpu::{GpuContext, TextureCache},
    scene::AreaScene,
};
use glam::Vec3;
use std::collections::{HashMap, HashSet};
use thedas_formats::state::{AddressMode, BlendFactor, BlendOp, CompareFunc, Filter, PassState};
use thedas_shader::bytecode::TextureType;
use thedas_vfs::Vfs;

pub(crate) struct Material {
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

pub(crate) struct Draw {
    pub k: usize,
    pub draw: usize,
    pub mesh: usize,
    pub skin_slot: usize,
    pub sphere: (Vec3, f32),
    pub material: usize,
    pub alpha: [f32; 4],
    pub clip: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rule {
    texture: &'static str,
    sampler: &'static str,
    channel: u32,
    clip: f32,
    hair_core: bool,
    blended_cutout: bool,
}

fn rule(name: &str) -> Option<Rule> {
    let mut names = name.split('+');
    let (vs, ps) = (names.next()?, names.next()?);
    let (texture, sampler, channel, clip, hair_core) = match (vs, ps) {
        ("static1.vsh", "static1_punch.psh") | ("prop1.vsh", "prop1_punch.psh") => {
            ("mml_tDiffuse", "smpDecal", 3, 0.3, false)
        }
        ("hair0_pass1.vsh", "hair0_pass1.psh") => {
            ("mml_tPackedTexture", "smpHairTexture", 2, -1.0, true)
        }
        ("eye0.vsh", "eyelash0.psh") => ("mml_tDiffuse", "smpDecal", 3, -1.0, false),
        _ => return None,
    };
    Some(Rule {
        texture,
        sampler,
        channel,
        clip,
        hair_core,
        blended_cutout: matches!(
            (vs, ps),
            ("prop1.vsh", "prop1_punch.psh") | ("eye0.vsh", "eyelash0.psh")
        ),
    })
}

fn supported_pass(rule: Rule, pass: &PassState, enabled: bool) -> bool {
    pass.z_enable
        && pass.z_write
        && match pass.blend {
            None => true,
            Some((BlendFactor::SrcAlpha, BlendFactor::InvSrcAlpha, BlendOp::Add)) => {
                enabled && rule.blended_cutout && pass.alpha_test.is_some()
            }
            _ => false,
        }
}

pub(crate) fn comparison(c: CompareFunc) -> f32 {
    match c {
        CompareFunc::Never => 1.0,
        CompareFunc::Less => 2.0,
        CompareFunc::Equal => 3.0,
        CompareFunc::LessEqual => 4.0,
        CompareFunc::Greater => 5.0,
        CompareFunc::NotEqual => 6.0,
        CompareFunc::GreaterEqual => 7.0,
        CompareFunc::Always => 8.0,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn plan(
    ctx: &GpuContext,
    vfs: &Vfs,
    scene: &AreaScene,
    d3d: &D3dPass,
    cache: &mut TextureCache,
    vertices: &mut HashMap<usize, Vec<DeferredVertex>>,
    skin_count: &mut usize,
) -> (Vec<Draw>, Vec<Material>) {
    let mut draws = Vec::new();
    let mut materials = Vec::new();
    if std::env::var("THEDAS_DEFERRED_CUTOUT_SHADOWS").as_deref() == Ok("0") {
        return (draws, materials);
    }
    let mut seen = HashSet::new();
    let mut material_of = HashMap::new();
    let blended = std::env::var("THEDAS_DEFERRED_BLENDED_CUTOUTS").as_deref() != Ok("0");
    for (k, d) in d3d.draws.iter().enumerate() {
        let program = d3d.program(d);
        let Some(r) = rule(&program.name) else {
            continue;
        };
        let p = &program.pass;
        let Some(item) = scene.draws.get(d.draw) else {
            continue;
        };
        if program.instanced
            || !supported_pass(r, p, blended)
            || (d.multipass && !r.hair_core)
            || (p.alpha_test.is_none() && r.clip < 0.0)
            || (r.hair_core
                && (p.color_write != 0 || p.alpha_test != Some((CompareFunc::GreaterEqual, 255))))
            || item.cast_shadow == Some(false)
            || item.lod.is_some()
            || scene.cut_away.contains(&d.draw)
            || item
                .skin
                .as_ref()
                .is_some_and(|s| s.palette.len() > crate::deferred_mesh::BONES)
            || seen.contains(&d.draw)
        {
            continue;
        }
        let Some(m) = scene.materials.get(item.material) else {
            continue;
        };
        if let std::collections::hash_map::Entry::Vacant(entry) = vertices.entry(d.mesh) {
            let Some(stream) = scene
                .meshes
                .get(d.mesh)
                .and_then(|m| crate::deferred_mesh::vertices(m, item.skin.is_some()))
            else {
                continue;
            };
            entry.insert(stream);
        }
        let key = (item.material, program.name.clone());
        let material = if let Some(&i) = material_of.get(&key) {
            i
        } else {
            // Missing opacity maps must not create a solid rectangular caster.
            let Some(view) = [
                m.texture(r.texture),
                crate::d3d::texture_default(m, r.texture),
            ]
            .into_iter()
            .flatten()
            .filter(|n| !n.eq_ignore_ascii_case("default_missing.dds"))
            .find_map(|n| cache.view(ctx, vfs, &n, TextureType::Tex2D)) else {
                continue;
            };
            let Ok(s) = program.state.sampler_at(Some(r.sampler), Some(0)) else {
                continue;
            };
            let border = ctx
                .device
                .features()
                .contains(wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER);
            let address = |a| match a {
                AddressMode::Wrap => wgpu::AddressMode::Repeat,
                AddressMode::Mirror | AddressMode::MirrorOnce => wgpu::AddressMode::MirrorRepeat,
                AddressMode::Border if border => wgpu::AddressMode::ClampToBorder,
                _ => wgpu::AddressMode::ClampToEdge,
            };
            let filter = |f| match f {
                Filter::Linear | Filter::Anisotropic => wgpu::FilterMode::Linear,
                _ => wgpu::FilterMode::Nearest,
            };
            let uses_border =
                border && [s.address_u, s.address_v, s.address_w].contains(&AddressMode::Border);
            let sampler = ctx.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("deferred cutout opacity"),
                address_mode_u: address(s.address_u),
                address_mode_v: address(s.address_v),
                address_mode_w: address(s.address_w),
                mag_filter: filter(s.mag),
                min_filter: filter(s.min),
                mipmap_filter: match s.mip {
                    Filter::Linear | Filter::Anisotropic => wgpu::MipmapFilterMode::Linear,
                    _ => wgpu::MipmapFilterMode::Nearest,
                },
                lod_max_clamp: if s.mip == Filter::None { 0.0 } else { 32.0 },
                border_color: uses_border.then_some({
                    match (s.border >> 24 >= 128, s.border & 0x00ff_ffff >= 0x0080_8080) {
                        (false, _) => wgpu::SamplerBorderColor::TransparentBlack,
                        (true, false) => wgpu::SamplerBorderColor::OpaqueBlack,
                        (true, true) => wgpu::SamplerBorderColor::OpaqueWhite,
                    }
                }),
                ..Default::default()
            });
            let i = materials.len();
            materials.push(Material { view, sampler });
            material_of.insert(key, i);
            i
        };
        let (cmp, reference) = p.alpha_test.unwrap_or((CompareFunc::Always, 0));
        let opacity =
            crate::d3d::material_vector(m, "mml_fOpacityMultiplier").map_or(1.0, |v| v[0]);
        if !opacity.is_finite() {
            continue;
        }
        let skin_slot = if item.skin.is_some() {
            let i = *skin_count;
            *skin_count += 1;
            i
        } else {
            0
        };
        seen.insert(d.draw);
        draws.push(Draw {
            k,
            draw: d.draw,
            mesh: d.mesh,
            skin_slot,
            sphere: item.sphere,
            material,
            alpha: [comparison(cmp), reference as f32, r.channel as f32, opacity],
            clip: r.clip,
        });
    }
    log::info!(
        "deferred: {} forward cutout point-shadow casters ({} hair cores, {} skinned)",
        draws.len(),
        draws.iter().filter(|d| d.alpha[2] == 2.0).count(),
        draws.iter().filter(|d| d.skin_slot > 0).count(),
    );
    (draws, materials)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_tested_depth_writing_blends_supply_cutout_coverage() {
        let mut p =
            thedas_formats::state::MaterialState::parse(br#"<MaterialState Name="fixture"/>"#)
                .unwrap()
                .pass()
                .unwrap();
        p.alpha_test = Some((CompareFunc::Greater, 20));
        p.blend = Some((
            BlendFactor::SrcAlpha,
            BlendFactor::InvSrcAlpha,
            BlendOp::Add,
        ));
        for name in [
            "prop1.vsh+prop1_punch.psh+proppunch.sta",
            "eye0.vsh+eyelash0.psh+eyelashpnch.sta",
        ] {
            let r = rule(name).unwrap();
            assert!(supported_pass(r, &p, true), "{name}");
            assert!(!supported_pass(r, &p, false));
            let mut no_depth = p;
            no_depth.z_write = false;
            assert!(!supported_pass(r, &no_depth, true));
            no_depth.z_write = true;
            no_depth.z_enable = false;
            assert!(!supported_pass(r, &no_depth, true));
            let mut untested = p;
            untested.alpha_test = None;
            assert!(!supported_pass(r, &untested, true));
            let mut additive = p;
            additive.blend = Some((BlendFactor::One, BlendFactor::One, BlendOp::Add));
            assert!(!supported_pass(r, &additive, true));
        }
        for name in [
            "static1.vsh+static1_punch.psh+punch.sta",
            "hair0_pass1.vsh+hair0_pass1.psh+hair.sta",
        ] {
            assert!(!supported_pass(rule(name).unwrap(), &p, true));
        }
        assert!(rule("prop1.vsh+propalpha1.psh+blend.sta").is_none());
        assert_eq!(
            rule("prop1.vsh+prop1_punch.psh+proppunch.sta")
                .unwrap()
                .clip,
            0.3
        );
    }

    #[test]
    fn only_known_cutouts_and_the_colour_disabled_hair_core_qualify() {
        let hair = rule("hair0_pass1.vsh+hair0_pass1.psh+hairalphapass1.sta").unwrap();
        assert!(hair.hair_core);
        assert_eq!(hair.channel, 2);
        assert_eq!(hair.texture, "mml_tPackedTexture");
        assert!(rule("hair0.vsh+hair0.psh+hairalphapass2.sta").is_none());
        assert!(rule("spt_leafcards.vsh+spt_leafcards.psh+spt.sta").is_none());
        let punch = rule("static1.vsh+static1_punch.psh+damshpnch1s.sta").unwrap();
        assert_eq!(punch.channel, 3);
        assert_eq!(punch.clip, 0.3);
        assert_eq!(
            rule("eye0.vsh+eyelash0.psh+eyelashpnch.sta")
                .unwrap()
                .channel,
            3
        );
    }
}
