//! Opaque cutout materials whose surviving texels can share the G-buffer.

use crate::{
    d3d::D3dProgram,
    gpu::{GpuContext, TextureCache},
    scene::SceneMaterial,
};
use thedas_formats::state::{AddressMode, CompareFunc, Filter};
use thedas_shader::bytecode::TextureType;
use thedas_vfs::Vfs;

pub(crate) struct Material {
    pub diffuse: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    /// Comparison, 8-bit reference, opacity multiplier, shader clip threshold.
    pub test: [f32; 4],
}

pub(crate) fn supported(name: &str) -> bool {
    matches!(
        (name.split('+').next(), name.split('+').nth(1)),
        (Some("static1.vsh"), Some("static1_punch.psh"))
    ) || character(name)
}

pub(crate) fn character(name: &str) -> bool {
    matches!(
        (name.split('+').next(), name.split('+').nth(1)),
        (
            Some("char1.vsh"),
            Some(
                "ch1alph.psh"
                    | "ch1alphnotnt.psh"
                    | "ch1alphmultnt.psh"
                    | "ch1alpharmtnt.psh"
                    | "ch1alphem.psh"
                    | "ch1alphemnotnt.psh"
                    | "ch1alphemmultnt.psh"
                    | "ch1alphemarmtnt.psh"
            )
        )
    )
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

pub(crate) fn material(
    ctx: &GpuContext,
    vfs: &Vfs,
    cache: &mut TextureCache,
    m: &SceneMaterial,
    program: &D3dProgram,
) -> Option<Material> {
    let p = &program.pass;
    let character = character(&program.name);
    if !supported(&program.name)
        || program.instanced
        || p.blend.is_some()
        || !p.z_enable
        || !p.z_write
        || p.color_write != 15
        || (character
            && (p.alpha_test.is_none()
                || std::env::var("THEDAS_DEFERRED_CHARACTER_CUTOUTS").as_deref() == Ok("0")))
    {
        return None;
    }
    let opacity = crate::d3d::material_vector(m, "mml_fOpacityMultiplier").map_or(1.0, |v| v[0]);
    if !opacity.is_finite() {
        return None;
    }
    // A missing/checkerboard opacity map must retain its faithful fallback draw.
    let diffuse = [
        m.texture("mml_tDiffuse"),
        crate::d3d::texture_default(m, "mml_tDiffuse"),
    ]
    .into_iter()
    .flatten()
    .filter(|n| !n.eq_ignore_ascii_case("default_missing.dds"))
    .find_map(|n| cache.view(ctx, vfs, &n, TextureType::Tex2D))?;
    let s = program.state.sampler_at(Some("smpDecal"), Some(0)).ok()?;
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
        label: Some("deferred punch diffuse"),
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
        border_color: uses_border.then_some(
            match (s.border >> 24 >= 128, s.border & 0x00ff_ffff >= 0x0080_8080) {
                (false, _) => wgpu::SamplerBorderColor::TransparentBlack,
                (true, false) => wgpu::SamplerBorderColor::OpaqueBlack,
                (true, true) => wgpu::SamplerBorderColor::OpaqueWhite,
            },
        ),
        ..Default::default()
    });
    let (cmp, reference) = p.alpha_test.unwrap_or((CompareFunc::Always, 0));
    Some(Material {
        diffuse,
        sampler,
        test: [
            comparison(cmp),
            reference as f32,
            opacity,
            if character { -1.0 } else { 0.3 },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_represented_punch_equation_qualifies() {
        assert!(supported("static1.vsh+static1_punch.psh+damshpnch1s.sta"));
        for ps in [
            "ch1alph",
            "ch1alphnotnt",
            "ch1alphmultnt",
            "ch1alpharmtnt",
            "ch1alphem",
            "ch1alphemnotnt",
            "ch1alphemmultnt",
            "ch1alphemarmtnt",
        ] {
            let name = format!("char1.vsh+{ps}.psh+characterpt.sta");
            assert!(supported(&name));
            assert!(character(&name));
        }
        for name in [
            "char1.vsh+ch1.psh+character.sta",
            "char2.vsh+ch2.psh+characterpt.sta",
            "static1.vsh+static1_alpha.psh+blend.sta",
            "prop1.vsh+prop1_punch.psh+prop.sta",
            "hair0_pass1.vsh+hair0_pass1.psh+hair.sta",
            "spt_leafcards.vsh+spt_leafcards.psh+spt.sta",
        ] {
            assert!(!supported(name), "{name}");
        }
    }
}
