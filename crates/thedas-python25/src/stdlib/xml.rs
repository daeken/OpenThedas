use std::{collections::BTreeMap, path::Path, rc::Rc};

use crate::{Error, NativeObject, Result, Runtime, Value};

use super::module;

pub(super) fn install(runtime: &mut Runtime) {
    module(
        runtime,
        "xml.dom.minidom",
        [
            (
                "parse",
                Value::function(|runtime, args| {
                    let [path] = args.take()?;
                    let resolved = runtime.resolve_read_path(Path::new(path.as_text()?));
                    let source = std::fs::read_to_string(resolved)
                        .map_err(|e| Error::new("IOError", e.to_string()))?;
                    parse(&source)
                }),
            ),
            (
                "parseString",
                Value::function(|_, args| {
                    let [source] = args.take()?;
                    parse(source.as_text()?)
                }),
            ),
        ],
    );
}

struct Node {
    kind: roxmltree::NodeType,
    name: String,
    text: Option<String>,
    attrs: BTreeMap<String, String>,
    children: Vec<Rc<Node>>,
}

fn parse(source: &str) -> Result<Value> {
    let doc = roxmltree::Document::parse(source)
        .map_err(|error| Error::new("ExpatError", error.to_string()))?;
    Ok(Value::object(Dom(convert(doc.root(), 0)?)))
}

fn convert(node: roxmltree::Node<'_, '_>, depth: usize) -> Result<Rc<Node>> {
    if depth > 128 {
        return Err(Error::unsupported("XML nesting exceeds 128 levels"));
    }
    let kind = node.node_type();
    let name = match kind {
        roxmltree::NodeType::Root => "#document".to_owned(),
        roxmltree::NodeType::Element => {
            qualified(node, node.tag_name().namespace(), node.tag_name().name())
        }
        roxmltree::NodeType::Text => "#text".to_owned(),
        roxmltree::NodeType::Comment => "#comment".to_owned(),
        roxmltree::NodeType::PI => node.pi().map_or("", |pi| pi.target).to_owned(),
    };
    Ok(Rc::new(Node {
        kind,
        name,
        text: if node.is_text() || node.is_comment() {
            node.text().map(str::to_owned)
        } else {
            node.pi().and_then(|pi| pi.value).map(str::to_owned)
        },
        attrs: node
            .attributes()
            .map(|attr| {
                (
                    qualified(node, attr.namespace(), attr.name()),
                    attr.value().to_owned(),
                )
            })
            .collect(),
        children: node
            .children()
            .map(|child| convert(child, depth + 1))
            .collect::<Result<_>>()?,
    }))
}

fn qualified(node: roxmltree::Node<'_, '_>, ns: Option<&str>, name: &str) -> String {
    ns.and_then(|ns| node.lookup_prefix(ns))
        .filter(|prefix| !prefix.is_empty())
        .map_or_else(|| name.to_owned(), |prefix| format!("{prefix}:{name}"))
}

struct Dom(Rc<Node>);

impl NativeObject for Dom {
    fn get(&self, name: &str) -> Result<Value> {
        let node = &self.0;
        match name {
            "documentElement" => Ok(node
                .children
                .iter()
                .find(|child| child.kind == roxmltree::NodeType::Element)
                .map_or(Value::NONE, |child| Value::object(Dom(child.clone())))),
            "nodeName" | "tagName" => Ok(Value::unicode(&node.name)),
            "nodeValue" | "data" => Ok(node.text.as_ref().map_or(Value::NONE, Value::unicode)),
            "childNodes" => Ok(Value::list(
                node.children
                    .iter()
                    .map(|child| Value::object(Dom(child.clone())))
                    .collect(),
            )),
            "firstChild" => Ok(node
                .children
                .first()
                .map_or(Value::NONE, |child| Value::object(Dom(child.clone())))),
            "getAttribute" => {
                let node = node.clone();
                Ok(Value::function(move |_, args| {
                    let [name] = args.take()?;
                    Ok(Value::unicode(
                        node.attrs.get(name.as_text()?).map_or("", String::as_str),
                    ))
                }))
            }
            "hasAttribute" => {
                let node = node.clone();
                Ok(Value::function(move |_, args| {
                    let [name] = args.take()?;
                    Ok(Value::bool(node.attrs.contains_key(name.as_text()?)))
                }))
            }
            "getElementsByTagName" => {
                let node = node.clone();
                Ok(Value::function(move |_, args| {
                    let [name] = args.take()?;
                    let name = name.as_text()?;
                    let mut result = Vec::new();
                    collect(&node, name, &mut result);
                    Ok(Value::list(result))
                }))
            }
            _ => Err(Error::new("AttributeError", name)),
        }
    }
}

fn collect(node: &Node, name: &str, result: &mut Vec<Value>) {
    for child in &node.children {
        if child.kind == roxmltree::NodeType::Element && (name == "*" || child.name == name) {
            result.push(Value::object(Dom(child.clone())));
        }
        collect(child, name, result);
    }
}
