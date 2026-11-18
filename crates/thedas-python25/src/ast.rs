use std::rc::Rc;

use crate::{Error, Result};

pub(crate) type Ast = Rc<Node>;

pub(crate) struct Node {
    pub kind: &'static str,
    pub line: usize,
    pub named: bool,
    source: Rc<str>,
    span: std::ops::Range<usize>,
    children: Vec<(Option<&'static str>, Ast)>,
}

impl Node {
    pub fn text(&self) -> &str {
        &self.source[self.span.clone()]
    }

    pub fn named(&self) -> impl Iterator<Item = &Ast> {
        self.children
            .iter()
            .map(|(_, node)| node)
            .filter(|node| node.named)
    }

    pub fn tokens(&self) -> impl Iterator<Item = &Ast> {
        self.children.iter().map(|(_, node)| node)
    }

    pub fn fields<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Ast> {
        self.children
            .iter()
            .filter_map(move |(field, node)| (*field == Some(name)).then_some(node))
    }

    pub fn field(&self, name: &str) -> Option<&Ast> {
        self.children
            .iter()
            .find_map(|(field, node)| (*field == Some(name)).then_some(node))
    }

    pub fn need(&self, name: &str) -> Result<&Ast> {
        self.field(name).ok_or_else(|| self.unsupported())
    }

    pub fn first(&self) -> Result<&Ast> {
        self.named().next().ok_or_else(|| self.unsupported())
    }

    pub fn unsupported(&self) -> Error {
        Error::unsupported(format!("{} on line {}", self.kind, self.line))
    }

    fn lower(node: tree_sitter::Node<'_>, source: &Rc<str>, depth: usize) -> Result<Ast> {
        if depth > 256 {
            return Err(Error::unsupported("AST nesting exceeds 256 levels"));
        }
        if node.is_error() || node.is_missing() {
            return Err(Error::new(
                "SyntaxError",
                format!(
                    "invalid syntax at line {}, column {}",
                    node.start_position().row + 1,
                    node.start_position().column + 1,
                ),
            ));
        }
        let mut cursor = node.walk();
        let children = node
            .children(&mut cursor)
            .enumerate()
            .filter(|(_, child)| !child.is_extra() || child.has_error())
            .map(|(i, child)| {
                Ok((
                    node.field_name_for_child(i as u32),
                    Self::lower(child, source, depth + 1)?,
                ))
            })
            .collect::<Result<_>>()?;
        Ok(Rc::new(Self {
            kind: node.kind(),
            named: node.is_named(),
            line: node.start_position().row + 1,
            source: source.clone(),
            span: node.byte_range(),
            children,
        }))
    }
}

pub(crate) fn parse(source: &str) -> Result<Ast> {
    let source: Rc<str> = source.replace("\r\n", "\n").replace('\r', "\n").into();
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .map_err(|err| Error::unsupported(err.to_string()))?;
    let tree = parser
        .parse(source.as_bytes(), None)
        .ok_or_else(|| Error::new("SyntaxError", "parser did not produce a tree"))?;
    Node::lower(tree.root_node(), &source, 0)
}
