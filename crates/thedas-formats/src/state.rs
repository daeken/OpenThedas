//! Render states (`<name>.sta`, stored as `<name>.sta.xml` in `states.erf`): the D3D9 device
//! and sampler states a material pass sets, written as XML:
//!
//! ```xml
//! <MaterialState Name="DAMshPnch1S">
//!   <DeviceState State="DS_ALPHATESTENABLE" Value="TRUE" />
//!   <DeviceState State="DS_ALPHAFUNC" Value="CMP_GREATER" />
//!   <DeviceState State="DS_ALPHAREF" Value="127" />
//!   <DeviceState State="DS_CULLMODE" Value="CULL_CW" />
//!   <SamplerState SamplerName="smpDecal" State="SAMP_ADDRESSU" Value="TEXADD_WRAP" />
//! </MaterialState>
//! ```
//!
//! State and value names mirror `D3DRENDERSTATETYPE` / `D3DSAMPLERSTATETYPE` with a `DS_` /
//! `SAMP_` prefix. [`MaterialState`] keeps every pair as written and offers typed accessors for
//! the ones the renderer needs. Values not set fall back to the D3D9 defaults (provisional:
//! the engine may establish its own defaults before applying a state file).

use crate::material::MaterialError;

#[derive(Clone, Debug, PartialEq)]
pub struct SamplerState {
    pub sampler: String,
    pub state: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct MaterialState {
    pub name: String,
    /// `(state, value)` in file order, e.g. `("DS_ZFUNC", "CMP_LESSEQUAL")`.
    pub device: Vec<(String, String)>,
    pub samplers: Vec<SamplerState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareFunc {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cull {
    None,
    /// Cull clockwise triangles.
    Cw,
    /// Cull counter-clockwise triangles (the D3D9 default).
    Ccw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendFactor {
    Zero,
    One,
    SrcColor,
    InvSrcColor,
    SrcAlpha,
    InvSrcAlpha,
    DestAlpha,
    InvDestAlpha,
    DestColor,
    InvDestColor,
    SrcAlphaSat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendOp {
    Add,
    Subtract,
    RevSubtract,
    Min,
    Max,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressMode {
    Wrap,
    Mirror,
    Clamp,
    Border,
    MirrorOnce,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    None,
    Point,
    Linear,
    Anisotropic,
}

/// The fixed-function settings for one pass, resolved with D3D9 defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PassState {
    pub z_enable: bool,
    pub z_write: bool,
    pub z_func: CompareFunc,
    pub cull: Cull,
    pub blend: Option<(BlendFactor, BlendFactor, BlendOp)>,
    /// Alpha test: function and reference in 0..=255.
    pub alpha_test: Option<(CompareFunc, u8)>,
    /// Which colour channels are written (bit 0 = red ... bit 3 = alpha).
    pub color_write: u8,
    pub srgb_write: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SamplerSettings {
    pub address_u: AddressMode,
    pub address_v: AddressMode,
    pub address_w: AddressMode,
    pub min: Filter,
    pub mag: Filter,
    pub mip: Filter,
    pub srgb: bool,
    /// `D3DCOLOR` (ARGB).
    pub border: u32,
}

impl Default for SamplerSettings {
    fn default() -> Self {
        Self {
            address_u: AddressMode::Wrap,
            address_v: AddressMode::Wrap,
            address_w: AddressMode::Wrap,
            min: Filter::Point,
            mag: Filter::Point,
            mip: Filter::None,
            srgb: false,
            border: 0,
        }
    }
}

fn cmp(v: &str) -> Option<CompareFunc> {
    Some(match v.trim_start_matches("CMP_") {
        "NEVER" => CompareFunc::Never,
        "LESS" => CompareFunc::Less,
        "EQUAL" => CompareFunc::Equal,
        "LESSEQUAL" => CompareFunc::LessEqual,
        "GREATER" => CompareFunc::Greater,
        "NOTEQUAL" => CompareFunc::NotEqual,
        "GREATEREQUAL" => CompareFunc::GreaterEqual,
        "ALWAYS" => CompareFunc::Always,
        _ => return None,
    })
}

fn blend_factor(v: &str) -> Option<BlendFactor> {
    use BlendFactor::*;
    Some(match v.trim_start_matches("BLEND_") {
        "ZERO" => Zero,
        "ONE" => One,
        "SRCCOLOR" => SrcColor,
        "INVSRCCOLOR" => InvSrcColor,
        "SRCALPHA" => SrcAlpha,
        "INVSRCALPHA" => InvSrcAlpha,
        "DESTALPHA" => DestAlpha,
        "INVDESTALPHA" => InvDestAlpha,
        "DESTCOLOR" => DestColor,
        "INVDESTCOLOR" => InvDestColor,
        "SRCALPHASAT" => SrcAlphaSat,
        _ => return None,
    })
}

fn truthy(v: &str) -> bool {
    matches!(v, "TRUE" | "1" | "ZBUFFERENABLE_TRUE" | "ZB_TRUE")
}

fn address(v: &str) -> Option<AddressMode> {
    Some(match v.trim_start_matches("TEXADD_") {
        "WRAP" => AddressMode::Wrap,
        "MIRROR" => AddressMode::Mirror,
        "CLAMP" => AddressMode::Clamp,
        "BORDER" => AddressMode::Border,
        "MIRRORONCE" => AddressMode::MirrorOnce,
        _ => return None,
    })
}

fn filter(v: &str) -> Option<Filter> {
    Some(match v.trim_start_matches("TEXFILT_") {
        "NONE" => Filter::None,
        "POINT" => Filter::Point,
        "LINEAR" => Filter::Linear,
        "ANISOTROPIC" => Filter::Anisotropic,
        _ => return None,
    })
}

fn bad(state: &str, value: &str) -> MaterialError {
    MaterialError::BadValue {
        element: "DeviceState",
        name: state.to_string(),
        value: value.to_string(),
    }
}

impl MaterialState {
    pub fn parse(data: &[u8]) -> Result<Self, MaterialError> {
        let text = String::from_utf8_lossy(data);
        let text = text.trim_start_matches('\u{feff}');
        let doc = roxmltree::Document::parse_with_options(
            text,
            roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )
        .map_err(|e| MaterialError::Xml(e.to_string()))?;
        let root = doc.root_element();
        if !root.tag_name().name().eq_ignore_ascii_case("MaterialState") {
            return Err(MaterialError::WrongRoot {
                expected: "MaterialState",
                found: root.tag_name().name().to_string(),
            });
        }
        let attr = |n: &roxmltree::Node, a: &str| {
            n.attributes()
                .find(|x| x.name().eq_ignore_ascii_case(a))
                .map(|x| x.value().trim().to_string())
        };
        let mut st = MaterialState {
            name: attr(&root, "Name").unwrap_or_default(),
            ..Default::default()
        };
        for n in root.children().filter(|n| n.is_element()) {
            let tag = n.tag_name().name();
            let state = attr(&n, "State").ok_or(MaterialError::MissingAttr {
                element: "DeviceState",
                attr: "State",
            })?;
            let value = attr(&n, "Value").ok_or(MaterialError::MissingAttr {
                element: "DeviceState",
                attr: "Value",
            })?;
            if tag.eq_ignore_ascii_case("DeviceState") {
                st.device.push((state, value));
            } else if tag.eq_ignore_ascii_case("SamplerState") {
                st.samplers.push(SamplerState {
                    sampler: attr(&n, "SamplerName").unwrap_or_default(),
                    state,
                    value,
                });
            }
        }
        // Validate the values the typed accessors interpret, so bad files fail at parse time.
        st.pass()?;
        for s in &st.samplers {
            st.sampler(&s.sampler)?;
        }
        Ok(st)
    }

    /// The last value set for a device state.
    pub fn device_state(&self, state: &str) -> Option<&str> {
        self.device
            .iter()
            .rev()
            .find(|(s, _)| s.eq_ignore_ascii_case(state))
            .map(|(_, v)| v.as_str())
    }

    /// Fixed-function pass settings, D3D9 defaults where the file is silent.
    pub fn pass(&self) -> Result<PassState, MaterialError> {
        let get = |s: &str| self.device_state(s);
        let parse_cmp = |s: &str, default| match get(s) {
            None => Ok(default),
            Some(v) => cmp(v).ok_or_else(|| bad(s, v)),
        };
        let z_enable = get("DS_ZENABLE").is_none_or(truthy);
        let z_write = get("DS_ZWRITEENABLE").is_none_or(truthy);
        let z_func = parse_cmp("DS_ZFUNC", CompareFunc::LessEqual)?;
        let cull = match get("DS_CULLMODE") {
            None => Cull::Ccw,
            Some("CULL_NONE") => Cull::None,
            Some("CULL_CW") => Cull::Cw,
            Some("CULL_CCW") => Cull::Ccw,
            Some(v) => return Err(bad("DS_CULLMODE", v)),
        };
        let blend = if get("DS_ALPHABLENDENABLE").is_some_and(truthy) {
            // An unknown factor (one file in the install says BLEND_ALPHA, which D3D9 does not
            // have) leaves the D3D9 default in place, as a rejected SetRenderState would.
            // Provisional.
            let f = |s: &str, d| -> Result<BlendFactor, MaterialError> {
                Ok(get(s).and_then(blend_factor).unwrap_or(d))
            };
            let op = match get("DS_BLENDOP") {
                None | Some("BLENDOP_ADD") => BlendOp::Add,
                Some("BLENDOP_SUBTRACT") => BlendOp::Subtract,
                Some("BLENDOP_REVSUBTRACT") => BlendOp::RevSubtract,
                Some("BLENDOP_MIN") => BlendOp::Min,
                Some("BLENDOP_MAX") => BlendOp::Max,
                Some(v) => return Err(bad("DS_BLENDOP", v)),
            };
            Some((
                f("DS_SRCBLEND", BlendFactor::One)?,
                f("DS_DESTBLEND", BlendFactor::Zero)?,
                op,
            ))
        } else {
            None
        };
        let alpha_test = if get("DS_ALPHATESTENABLE").is_some_and(truthy) {
            let func = parse_cmp("DS_ALPHAFUNC", CompareFunc::Always)?;
            let r = match get("DS_ALPHAREF") {
                None => 0,
                Some(v) => v
                    .parse::<u32>()
                    .map_err(|_| bad("DS_ALPHAREF", v))?
                    .min(255) as u8,
            };
            Some((func, r))
        } else {
            None
        };
        let color_write = match get("DS_COLORWRITEENABLE") {
            None => 0xf,
            Some(v) => match v.parse::<u32>() {
                Ok(n) => (n & 0xf) as u8,
                Err(_) => {
                    let mut m = 0;
                    for (name, bit) in [("RED", 1), ("GREEN", 2), ("BLUE", 4), ("ALPHA", 8)] {
                        if v.contains(name) {
                            m |= bit;
                        }
                    }
                    if m == 0 && !v.contains("NONE") && v != "0" {
                        return Err(bad("DS_COLORWRITEENABLE", v));
                    }
                    m
                }
            },
        };
        Ok(PassState {
            z_enable,
            z_write,
            z_func,
            cull,
            blend,
            alpha_test,
            color_write,
            srgb_write: get("DS_SRGBWRITEENABLE").is_some_and(truthy),
        })
    }

    /// Settings for one sampler (by its HLSL sampler name, e.g. `smpDecal`).
    pub fn sampler(&self, name: &str) -> Result<SamplerSettings, MaterialError> {
        let mut s = SamplerSettings::default();
        for x in self
            .samplers
            .iter()
            .filter(|x| x.sampler.eq_ignore_ascii_case(name))
        {
            let v = x.value.as_str();
            let berr = || MaterialError::BadValue {
                element: "SamplerState",
                name: x.state.clone(),
                value: v.to_string(),
            };
            match x.state.as_str() {
                "SAMP_ADDRESSU" => s.address_u = address(v).ok_or_else(berr)?,
                "SAMP_ADDRESSV" => s.address_v = address(v).ok_or_else(berr)?,
                "SAMP_ADDRESSW" => s.address_w = address(v).ok_or_else(berr)?,
                "SAMP_MINFILTER" => s.min = filter(v).ok_or_else(berr)?,
                "SAMP_MAGFILTER" => s.mag = filter(v).ok_or_else(berr)?,
                "SAMP_MIPFILTER" => s.mip = filter(v).ok_or_else(berr)?,
                "SAMP_SRGBTEXTURE" => s.srgb = truthy(v),
                "SAMP_BORDERCOLOR" => s.border = v.parse().map_err(|_| berr())?,
                _ => {}
            }
        }
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUNCH: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<!-- comment -->
<MaterialState Name="DAMshPnch1S">
  <DeviceState State="DS_ALPHABLENDENABLE" Value="FALSE" />
  <DeviceState State="DS_ALPHATESTENABLE" Value="TRUE" />
  <DeviceState State="DS_ALPHAFUNC" Value="CMP_GREATER" />
  <DeviceState State="DS_ALPHAREF" Value="127" />
  <DeviceState State="DS_ZENABLE" Value="ZBUFFERENABLE_TRUE" />
  <DeviceState State="DS_ZFUNC" Value="CMP_LESSEQUAL" />
  <DeviceState State="DS_CULLMODE" Value="CULL_CW" />
  <SamplerState SamplerName="smpDecal" State="SAMP_ADDRESSU" Value="TEXADD_WRAP" />
  <SamplerState SamplerName="smpLightmap" State="SAMP_ADDRESSU" Value="TEXADD_CLAMP" />
  <SamplerState SamplerName="smpLightmap" State="SAMP_MINFILTER" Value="TEXFILT_LINEAR" />
  <SamplerState SamplerName="smpShadow" State="SAMP_BORDERCOLOR" Value="4294967295" />
</MaterialState>"#;

    #[test]
    fn reads_pass_and_sampler_state() {
        let st = MaterialState::parse(PUNCH.as_bytes()).unwrap();
        let p = st.pass().unwrap();
        assert_eq!(p.alpha_test, Some((CompareFunc::Greater, 127)));
        assert_eq!(p.cull, Cull::Cw);
        assert!(p.z_enable && p.z_write);
        assert_eq!(p.blend, None);
        let lm = st.sampler("smpLightmap").unwrap();
        assert_eq!(lm.address_u, AddressMode::Clamp);
        assert_eq!(
            lm.address_v,
            AddressMode::Wrap,
            "unset states keep D3D9 defaults"
        );
        assert_eq!(lm.min, Filter::Linear);
        assert_eq!(st.sampler("smpShadow").unwrap().border, 0xFFFF_FFFF);
    }

    #[test]
    fn reads_blending_and_rejects_bad_values() {
        let x = br#"<MaterialState Name="b">
            <DeviceState State="DS_ALPHABLENDENABLE" Value="TRUE" />
            <DeviceState State="DS_SRCBLEND" Value="BLEND_SRCALPHA" />
            <DeviceState State="DS_DESTBLEND" Value="BLEND_INVSRCALPHA" />
            <DeviceState State="DS_ZWRITEENABLE" Value="FALSE" /></MaterialState>"#;
        let p = MaterialState::parse(x).unwrap().pass().unwrap();
        assert_eq!(
            p.blend,
            Some((
                BlendFactor::SrcAlpha,
                BlendFactor::InvSrcAlpha,
                BlendOp::Add
            ))
        );
        assert!(!p.z_write);
        let odd = br#"<MaterialState Name="b">
            <DeviceState State="DS_ALPHABLENDENABLE" Value="TRUE" />
            <DeviceState State="DS_SRCBLEND" Value="BLEND_ALPHA" /></MaterialState>"#;
        let p = MaterialState::parse(odd).unwrap().pass().unwrap();
        assert_eq!(
            p.blend,
            Some((BlendFactor::One, BlendFactor::Zero, BlendOp::Add))
        );
        let bad = br#"<MaterialState><DeviceState State="DS_ZFUNC" Value="CMP_SOMETIMES"/></MaterialState>"#;
        assert!(MaterialState::parse(bad).is_err());
        assert!(MaterialState::parse(b"<Material/>").is_err());
    }
}
