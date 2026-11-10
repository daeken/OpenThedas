//! Materials. Both kinds are XML, not GFF:
//!
//! - Material objects (`.mao`, in `materialobjects.erf`) pick a material, a default semantic,
//!   and bind textures and constants to the material's parameters.
//! - Materials (`<name>.mat`, stored as `<name>.mat.xml` in `materials.erf`) declare the
//!   parameters and, per semantic, a tree of `Logic` blocks that select a technique by shader
//!   model and detail level. Each technique's passes name a vertex shader, a pixel shader
//!   (compiled D3D9 bytecode in `shaders.erf` as `<name>.fxo`) and a render state (`.sta`).
//!
//! Element and attribute names are matched case-insensitively: a few material objects use
//! lowercase `texture` elements.
//!
//! Provisional: how `Logic` blocks are chosen ([`MaterialDef::select`]) is inferred from the
//! XML, not checked against the original executable.

#[derive(Debug, thiserror::Error)]
pub enum MaterialError {
    #[error("XML: {0}")]
    Xml(String),
    #[error("expected root element {expected}, found {found}")]
    WrongRoot {
        expected: &'static str,
        found: String,
    },
    #[error("{element}: missing attribute {attr}")]
    MissingAttr {
        element: &'static str,
        attr: &'static str,
    },
    #[error("{element} {name}: bad value {value:?}")]
    BadValue {
        element: &'static str,
        name: String,
        value: String,
    },
}

pub type Result<T> = std::result::Result<T, MaterialError>;

fn document(data: &[u8]) -> Result<String> {
    let text = String::from_utf8_lossy(data);
    Ok(text.trim_start_matches('\u{feff}').to_string())
}

fn parse_doc(text: &str) -> Result<roxmltree::Document<'_>> {
    roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| MaterialError::Xml(e.to_string()))
}

fn is(node: &roxmltree::Node, name: &str) -> bool {
    node.is_element() && node.tag_name().name().eq_ignore_ascii_case(name)
}

fn attr<'a>(node: &roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name().eq_ignore_ascii_case(name))
        .map(|a| a.value())
}

fn req<'a>(
    node: &roxmltree::Node<'a, '_>,
    element: &'static str,
    name: &'static str,
) -> Result<&'a str> {
    attr(node, name).ok_or(MaterialError::MissingAttr {
        element,
        attr: name,
    })
}

/// Parses one number, including the spellings MSVC's printf uses for non-finite values
/// (`1.#INF00`, `-1.#QNAN0`, `-1.#IND00`), which appear in some exported material objects.
fn parse_float(v: &str) -> Option<f32> {
    if let Ok(f) = v.parse::<f32>() {
        return Some(f);
    }
    let upper = v.to_ascii_uppercase();
    let negative = upper.starts_with('-');
    if upper.contains("#INF") {
        Some(if negative {
            f32::NEG_INFINITY
        } else {
            f32::INFINITY
        })
    } else if upper.contains("#QNAN") || upper.contains("#SNAN") || upper.contains("#IND") {
        Some(f32::NAN)
    } else {
        None
    }
}

fn floats(element: &'static str, name: &str, value: &str) -> Result<Vec<f32>> {
    value
        .split_whitespace()
        .map(|v| {
            parse_float(v).ok_or_else(|| MaterialError::BadValue {
                element,
                name: name.to_string(),
                value: value.to_string(),
            })
        })
        .collect()
}

/// A texture binding in a material object.
#[derive(Clone, Debug, PartialEq)]
pub struct TextureBinding {
    /// The material parameter, e.g. `mml_tDiffuse`.
    pub parameter: String,
    /// The texture resource as written, e.g. `fca_master_d.dds` (the extension is sometimes
    /// omitted; see [`TextureBinding::resource`]).
    pub res_name: String,
    /// Set on placeholders such as the lightmap (`LM.dds`) that the engine substitutes.
    pub requires_id: bool,
}

impl TextureBinding {
    /// The resource name with a `.dds` extension.
    pub fn resource(&self) -> String {
        if self.res_name.contains('.') {
            self.res_name.clone()
        } else {
            format!("{}.dds", self.res_name)
        }
    }
}

/// A material object (`.mao`).
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialObject {
    pub name: String,
    /// The material file, e.g. `static.mat`.
    pub material: String,
    pub default_semantic: Option<String>,
    pub textures: Vec<TextureBinding>,
    pub floats: Vec<(String, f32)>,
    pub vectors: Vec<(String, [f32; 4])>,
    pub vector_arrays: Vec<(String, Vec<f32>)>,
}

impl MaterialObject {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let text = document(data)?;
        let doc = parse_doc(&text)?;
        let root = doc.root_element();
        if !is(&root, "MaterialObject") {
            return Err(MaterialError::WrongRoot {
                expected: "MaterialObject",
                found: root.tag_name().name().to_string(),
            });
        }
        let mut mao = MaterialObject {
            name: attr(&root, "Name").unwrap_or_default().to_string(),
            material: String::new(),
            default_semantic: None,
            textures: Vec::new(),
            floats: Vec::new(),
            vectors: Vec::new(),
            vector_arrays: Vec::new(),
        };
        for n in root.children().filter(|n| n.is_element()) {
            if is(&n, "Material") {
                mao.material = req(&n, "Material", "Name")?.to_string();
            } else if is(&n, "DefaultSemantic") {
                mao.default_semantic = attr(&n, "Name").map(str::to_string);
            } else if is(&n, "Texture") {
                mao.textures.push(TextureBinding {
                    parameter: req(&n, "Texture", "Name")?.to_string(),
                    res_name: req(&n, "Texture", "ResName")?.to_string(),
                    requires_id: attr(&n, "RequiresID")
                        .is_some_and(|v| v.eq_ignore_ascii_case("true")),
                });
            } else if is(&n, "Float") {
                let name = req(&n, "Float", "Name")?;
                let v = floats("Float", name, req(&n, "Float", "value")?)?;
                mao.floats
                    .push((name.to_string(), v.first().copied().unwrap_or(0.0)));
            } else if is(&n, "Vector4f") {
                let name = req(&n, "Vector4f", "Name")?;
                let v = floats("Vector4f", name, req(&n, "Vector4f", "value")?)?;
                let mut out = [0.0; 4];
                for (o, x) in out.iter_mut().zip(&v) {
                    *o = *x;
                }
                mao.vectors.push((name.to_string(), out));
            } else if is(&n, "Vector4fArray") {
                let name = req(&n, "Vector4fArray", "Name")?;
                let v = floats("Vector4fArray", name, req(&n, "Vector4fArray", "value")?)?;
                mao.vector_arrays.push((name.to_string(), v));
            }
        }
        if mao.material.is_empty() {
            return Err(MaterialError::MissingAttr {
                element: "Material",
                attr: "Name",
            });
        }
        Ok(mao)
    }

    /// The texture bound to a parameter, if any.
    pub fn texture(&self, parameter: &str) -> Option<&TextureBinding> {
        self.textures
            .iter()
            .find(|t| t.parameter.eq_ignore_ascii_case(parameter))
    }

    /// The resource name of the material definition: `static.mat` -> `static.mat.xml`.
    pub fn material_resource(&self) -> String {
        format!("{}.xml", self.material)
    }

    pub fn semantic(&self) -> &str {
        self.default_semantic.as_deref().unwrap_or("Default")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Parameter {
    /// `InternalParameter` (supplied by the engine) rather than `Parameter`.
    pub internal: bool,
    pub ty: String,
    pub name: String,
    pub sampler: Option<String>,
    pub stage: Option<u32>,
    pub usage: Option<String>,
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParameterSet {
    pub name: String,
    pub parameters: Vec<Parameter>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    pub name: Option<String>,
    pub vertex_shader: Option<String>,
    pub pixel_shader: Option<String>,
    pub state: Option<String>,
    pub parameter_sets: Vec<String>,
    pub streams: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Technique {
    pub name: Option<String>,
    pub passes: Vec<Pass>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LogicTest {
    pub shader_model: Option<u32>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Logic {
    pub note: Option<String>,
    pub test: Option<LogicTest>,
    pub techniques: Vec<Technique>,
    pub children: Vec<Logic>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Semantic {
    pub name: String,
    pub render_class: Option<String>,
    pub logic: Vec<Logic>,
}

/// A material definition (`.mat.xml`).
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialDef {
    pub name: String,
    pub parameter_sets: Vec<ParameterSet>,
    pub semantics: Vec<Semantic>,
}

fn read_params(n: roxmltree::Node) -> Result<Vec<Parameter>> {
    let mut out = Vec::new();
    for p in n.children() {
        let internal = is(&p, "InternalParameter");
        if !internal && !is(&p, "Parameter") {
            continue;
        }
        let stage = match attr(&p, "Stage") {
            Some(s) => Some(s.trim().parse().map_err(|_| MaterialError::BadValue {
                element: "Parameter",
                name: attr(&p, "Name").unwrap_or_default().to_string(),
                value: s.to_string(),
            })?),
            None => None,
        };
        out.push(Parameter {
            internal,
            ty: attr(&p, "Type").unwrap_or_default().to_string(),
            name: req(&p, "Parameter", "Name")?.to_string(),
            sampler: attr(&p, "SamplerName").map(str::to_string),
            stage,
            usage: attr(&p, "Usage").map(str::to_string),
            default: attr(&p, "Default").map(str::to_string),
        });
    }
    Ok(out)
}

fn read_logic(n: roxmltree::Node, depth: usize) -> Result<Logic> {
    if depth > 64 {
        return Err(MaterialError::Xml("Logic blocks nested too deeply".into()));
    }
    let mut logic = Logic {
        note: attr(&n, "Note").map(str::to_string),
        test: None,
        techniques: Vec::new(),
        children: Vec::new(),
    };
    for c in n.children() {
        if is(&c, "LogicTest") {
            let shader_model = match attr(&c, "ShaderModel") {
                Some(s) => Some(s.trim().parse().map_err(|_| MaterialError::BadValue {
                    element: "LogicTest",
                    name: "ShaderModel".into(),
                    value: s.to_string(),
                })?),
                None => None,
            };
            logic.test = Some(LogicTest {
                shader_model,
                detail: attr(&c, "Detail").map(str::to_string),
            });
        } else if is(&c, "Technique") {
            let mut t = Technique {
                name: attr(&c, "Name").map(str::to_string),
                passes: Vec::new(),
            };
            for p in c.children().filter(|p| is(p, "Pass")) {
                t.passes.push(Pass {
                    name: attr(&p, "Name").map(str::to_string),
                    vertex_shader: attr(&p, "VertexShader").map(str::to_string),
                    pixel_shader: attr(&p, "PixelShader").map(str::to_string),
                    state: attr(&p, "State").map(str::to_string),
                    parameter_sets: p
                        .children()
                        .filter(|r| is(r, "ParameterSetRef"))
                        .filter_map(|r| attr(&r, "Ref").map(str::to_string))
                        .collect(),
                    streams: p
                        .children()
                        .filter(|r| is(r, "RequiredStreamData"))
                        .filter_map(|r| attr(&r, "Name").map(str::to_string))
                        .collect(),
                });
            }
            logic.techniques.push(t);
        } else if is(&c, "Logic") {
            logic.children.push(read_logic(c, depth + 1)?);
        }
    }
    Ok(logic)
}

impl MaterialDef {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let text = document(data)?;
        let doc = parse_doc(&text)?;
        let root = doc.root_element();
        if !is(&root, "Material") {
            return Err(MaterialError::WrongRoot {
                expected: "Material",
                found: root.tag_name().name().to_string(),
            });
        }
        let mut def = MaterialDef {
            name: attr(&root, "Name").unwrap_or_default().to_string(),
            parameter_sets: Vec::new(),
            semantics: Vec::new(),
        };
        // Parameters outside any set are collected into an unnamed set.
        let loose = read_params(root)?;
        if !loose.is_empty() {
            def.parameter_sets.push(ParameterSet {
                name: String::new(),
                parameters: loose,
            });
        }
        for n in root.children() {
            if is(&n, "ParameterSet") {
                def.parameter_sets.push(ParameterSet {
                    name: attr(&n, "Name").unwrap_or_default().to_string(),
                    parameters: read_params(n)?,
                });
            } else if is(&n, "Semantic") {
                def.semantics.push(Semantic {
                    name: req(&n, "Semantic", "Name")?.to_string(),
                    render_class: attr(&n, "RenderClass").map(str::to_string),
                    logic: n
                        .children()
                        .filter(|c| is(c, "Logic"))
                        .map(|c| read_logic(c, 0))
                        .collect::<Result<_>>()?,
                });
            }
        }
        Ok(def)
    }

    pub fn semantic(&self, name: &str) -> Option<&Semantic> {
        self.semantics
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Picks the technique for a semantic on a device with the given shader model and detail
    /// level: the first `Logic` block in document order (depth first) whose test passes and
    /// that holds a technique. A test passes when its shader model is at most `shader_model`
    /// and its detail level, if any, is at most `detail` (LOW < MEDIUM < HIGH). Provisional:
    /// the ordering is inferred from `static.mat`, whose opaque semantic offers only MEDIUM
    /// and LOW SM3 techniques.
    pub fn select(&self, semantic: &str, shader_model: u32, detail: &str) -> Option<&Technique> {
        fn rank(d: &str) -> u32 {
            match d.to_ascii_uppercase().as_str() {
                "LOW" => 0,
                "MEDIUM" => 1,
                "HIGH" => 2,
                "VERYHIGH" | "VERY_HIGH" | "ULTRA" => 3,
                _ => 1,
            }
        }
        fn passes(l: &Logic, sm: u32, detail: &str) -> bool {
            match &l.test {
                None => true,
                Some(t) => {
                    t.shader_model.is_none_or(|m| m <= sm)
                        && t.detail.as_deref().is_none_or(|d| rank(d) <= rank(detail))
                }
            }
        }
        fn find<'a>(blocks: &'a [Logic], sm: u32, detail: &str) -> Option<&'a Technique> {
            for l in blocks {
                if !passes(l, sm, detail) {
                    continue;
                }
                if let Some(t) = l.techniques.first() {
                    return Some(t);
                }
                if let Some(t) = find(&l.children, sm, detail) {
                    return Some(t);
                }
            }
            None
        }
        find(&self.semantic(semantic)?.logic, shader_model, detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAO: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<!-- comment --><MaterialObject Name="Test">
	<Material Name="static.mat"></Material>
	<DefaultSemantic Name="LOD1Punch"></DefaultSemantic>
	<Texture Name="mml_tLightmap" ResName="LM.dds" RequiresID="true"></Texture>
	<Texture Name="mml_tDiffuse" ResName="test_d.dds"></Texture>
	<texture Name='mml_tNormalMap' ResName='test_n' />
	<Float Name="mml_fOpacityMultiplier" value="0.5"></Float>
	<Vector4f Name="mml_vTintColor" value="1 0.5 0.25 1"></Vector4f>
</MaterialObject>"#;

    const MAT: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<Material Name = "Static">
  <ParameterSet Name="Common">
    <InternalParameter Type = "Matrix4x4f" Name = "aur_mWorldViewProj" Usage = "ModelViewProjectionTrans"/>
    <Parameter Type = "Texture" Name = "mml_tDiffuse" SamplerName = "smpDecal" Stage="2" Default = "Default_Missing"/>
  </ParameterSet>
  <Semantic Name = "Default" RenderClass = "Opaque,Static1">
    <Logic Note = "Root">
      <Logic Note = "SM3 high">
        <LogicTest ShaderModel = "3" Detail="HIGH"/>
        <Technique Name = "LOD1">
          <Pass Name = "Pass 1" VertexShader = "Hi.vsh" PixelShader = "Hi.psh" State = "S.sta"/>
        </Technique>
      </Logic>
      <Logic Note = "SM3">
        <LogicTest ShaderModel = "3" Detail="MEDIUM"/>
        <Technique Name = "LOD1">
          <Pass Name = "Pass 1" VertexShader = "Static1.vsh" PixelShader = "Static1.psh" State = "DAMshOpqe1S.sta">
            <ParameterSetRef Ref="Common"/>
            <RequiredStreamData Name = "POSITION"/>
          </Pass>
        </Technique>
      </Logic>
      <Logic Note = "SM2">
        <LogicTest ShaderModel = "2"/>
        <Technique Name = "LOD1">
          <Pass Name = "Pass 1" VertexShader = "SM2.vsh" PixelShader = "SM2.psh" State = "DAMshOpqe1S.sta"/>
        </Technique>
      </Logic>
    </Logic>
  </Semantic>
</Material>"#;

    #[test]
    fn reads_material_object() {
        let mao = MaterialObject::parse(MAO.as_bytes()).unwrap();
        assert_eq!(mao.material, "static.mat");
        assert_eq!(mao.material_resource(), "static.mat.xml");
        assert_eq!(mao.semantic(), "LOD1Punch");
        assert_eq!(
            mao.texture("MML_TDIFFUSE").unwrap().resource(),
            "test_d.dds"
        );
        assert_eq!(
            mao.texture("mml_tNormalMap").unwrap().resource(),
            "test_n.dds"
        );
        assert!(mao.texture("mml_tLightmap").unwrap().requires_id);
        assert_eq!(mao.floats, vec![("mml_fOpacityMultiplier".into(), 0.5)]);
        assert_eq!(mao.vectors[0].1, [1.0, 0.5, 0.25, 1.0]);
    }

    #[test]
    fn reads_material_and_selects_techniques() {
        let mat = MaterialDef::parse(MAT.as_bytes()).unwrap();
        assert_eq!(mat.parameter_sets[0].parameters.len(), 2);
        assert_eq!(mat.parameter_sets[0].parameters[1].stage, Some(2));
        let pass = |sm, detail| {
            mat.select("default", sm, detail)
                .map(|t| t.passes[0].pixel_shader.clone().unwrap())
        };
        assert_eq!(pass(3, "MEDIUM").as_deref(), Some("Static1.psh"));
        assert_eq!(pass(3, "high").as_deref(), Some("Hi.psh"));
        assert_eq!(pass(3, "LOW").as_deref(), Some("SM2.psh"));
        assert_eq!(pass(3, "VERYHIGH").as_deref(), Some("Hi.psh"));
        assert_eq!(pass(2, "MEDIUM").as_deref(), Some("SM2.psh"));
        assert_eq!(pass(1, "MEDIUM"), None);
        let t = mat.select("Default", 3, "medium").unwrap();
        assert_eq!(t.passes[0].parameter_sets, vec!["Common"]);
        assert_eq!(t.passes[0].streams, vec!["POSITION"]);
        assert!(mat.select("Missing", 3, "MEDIUM").is_none());
    }

    #[test]
    fn rejects_bad_xml() {
        assert!(MaterialObject::parse(b"<MaterialObject><Material></MaterialObject>").is_err());
        assert!(MaterialObject::parse(b"<Material Name='x'/>").is_err());
        assert!(MaterialObject::parse(b"<MaterialObject Name='x'/>").is_err());
        assert!(MaterialDef::parse(b"\xff\xfe garbage").is_err());
        let bad = br#"<MaterialObject><Material Name="a.mat"/><Float Name="f" value="x"/></MaterialObject>"#;
        assert!(MaterialObject::parse(bad).is_err());
    }

    #[test]
    fn accepts_msvc_non_finite_spellings() {
        assert_eq!(parse_float("1.#INF00"), Some(f32::INFINITY));
        assert_eq!(parse_float("-1.#INF00"), Some(f32::NEG_INFINITY));
        assert!(parse_float("-1.#QNAN0").unwrap().is_nan());
        assert!(parse_float("-1.#IND00").unwrap().is_nan());
        assert_eq!(parse_float("x"), None);
        let xml = br#"<MaterialObject><Material Name="a.mat"/><Vector4fArray Name="v" value="0.5 -1.#QNAN0 1.#INF00 0"/></MaterialObject>"#;
        let mao = MaterialObject::parse(xml).unwrap();
        assert_eq!(mao.vector_arrays[0].1.len(), 4);
    }
}
