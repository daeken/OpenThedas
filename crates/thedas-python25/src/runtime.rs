use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{ErrorKind, Write},
    path::{Component, Path, PathBuf},
    rc::{Rc, Weak},
};

use crate::{
    Args, Error, Result, Value,
    ast::{self, Ast, Node},
    ops, stdlib,
    value::{Kind, Names},
};

type ScopeRef = Rc<Scope>;

struct Scope {
    locals: Names,
    enclosing: Option<ScopeRef>,
    bindings: Rc<Bindings>,
    kind: ScopeKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    Module,
    Class,
    Function,
}

#[derive(Default)]
struct Bindings {
    globals: BTreeSet<String>,
    locals: BTreeSet<String>,
}

impl Bindings {
    fn for_body(body: &Node) -> Rc<Self> {
        let mut names = Self::default();
        bindings(body, &mut names.globals, &mut names.locals);
        Rc::new(names)
    }
}

pub(crate) struct Function {
    name: String,
    params: Vec<(String, Option<Value>)>,
    rest: Option<String>,
    keywords: Option<String>,
    body: Ast,
    enclosing: ScopeRef,
    lambda: bool,
    bindings: Rc<Bindings>,
    future_division: bool,
}

#[derive(Debug)]
enum Flow {
    Next,
    Return(Value),
    Break,
    Continue,
}

/// Output produced by one invocation of the host's command runner.
pub struct ProcessOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Option<Vec<u8>>,
}

type ProcessRunner = dyn FnMut(&[String], bool) -> Result<ProcessOutput>;

/// A Python source module resolved by the embedding application.
pub struct ModuleSource {
    pub source: String,
    pub filename: String,
}

type ModuleLoader = dyn FnMut(&str) -> Result<Option<ModuleSource>>;

/// An independent interpreter instance. Install application modules before running a script.
pub struct Runtime {
    modules: BTreeMap<String, Value>,
    builtins: BTreeMap<String, Value>,
    scope: ScopeRef,
    active_exceptions: Vec<Error>,
    scopes: Vec<Weak<Scope>>,
    call_depth: usize,
    import_depth: usize,
    future_division: bool,
    stdout: Box<dyn Write>,
    process_runner: Option<Box<ProcessRunner>>,
    module_loader: Option<Box<ModuleLoader>>,
    case_insensitive_reads: bool,
    fuel: Option<u64>,
}

impl Runtime {
    /// Supply the exact values visible through sys.argv. EclipseRay omits the script filename.
    pub fn new(argv: impl IntoIterator<Item = Value>) -> Self {
        let scope = Rc::new(Scope {
            locals: Rc::default(),
            enclosing: None,
            bindings: Rc::default(),
            kind: ScopeKind::Module,
        });
        let mut runtime = Self {
            modules: BTreeMap::new(),
            builtins: BTreeMap::new(),
            scopes: vec![Rc::downgrade(&scope)],
            scope,
            active_exceptions: Vec::new(),
            call_depth: 0,
            import_depth: 0,
            future_division: false,
            stdout: Box::new(std::io::sink()),
            process_runner: None,
            module_loader: None,
            case_insensitive_reads: false,
            fuel: None,
        };
        runtime.set_global("__name__", Value::bytes("__main__"));
        stdlib::install(&mut runtime, argv.into_iter().collect());
        runtime
    }

    pub fn set_stdout(&mut self, writer: impl Write + 'static) {
        self.stdout = Box::new(writer);
    }

    /// Match the Windows Toolset's ASCII asset filenames on case-sensitive hosts.
    pub fn enable_case_insensitive_reads(&mut self) {
        self.case_insensitive_reads = true;
    }

    /// Prefer the path as supplied. An ambiguous case-insensitive match leaves it unresolved.
    pub fn resolve_read_path<'a>(&self, path: &'a Path) -> Cow<'a, Path> {
        if self.case_insensitive_reads && missing_path(path) {
            resolve_ascii_case(path).map_or(Cow::Borrowed(path), Cow::Owned)
        } else {
            Cow::Borrowed(path)
        }
    }

    /// Install an argument-vector command runner. The boolean requests stderr
    /// merged into stdout. Without a runner, subprocess execution is unavailable.
    pub fn set_process_runner(
        &mut self,
        runner: impl FnMut(&[String], bool) -> Result<ProcessOutput> + 'static,
    ) {
        self.process_runner = Some(Box::new(runner));
    }

    /// Resolve imports that were not registered by the host or built-in library.
    /// The loader receives a dotted name and chooses where to look for its source.
    pub fn set_module_loader(
        &mut self,
        loader: impl FnMut(&str) -> Result<Option<ModuleSource>> + 'static,
    ) {
        self.module_loader = Some(Box::new(loader));
    }

    pub(crate) fn run_process(&mut self, args: &[String], merged: bool) -> Result<ProcessOutput> {
        if args.is_empty() {
            return Err(Error::new("ValueError", "empty process command"));
        }
        self.process_runner
            .as_mut()
            .ok_or_else(|| Error::unsupported("no host process runner installed"))?(
            args, merged
        )
    }

    /// Optionally cap AST visits for an invocation. Exhaustion is a host error.
    pub fn set_fuel(&mut self, visits: Option<u64>) {
        self.fuel = visits;
    }

    pub fn set_global(&mut self, name: impl Into<String>, value: Value) {
        self.global_scope()
            .locals
            .borrow_mut()
            .insert(name.into(), value);
    }

    pub fn global(&self, name: &str) -> Option<Value> {
        self.global_scope().locals.borrow().get(name).cloned()
    }

    pub fn register_module(&mut self, name: &str, value: Value) -> Result<()> {
        if !matches!(value.0, Kind::Module(_)) || name.split('.').any(str::is_empty) {
            return Err(Error::type_error(
                "expected a module and a dotted module name",
            ));
        }
        if let Some((parent, child)) = name.rsplit_once('.') {
            if !self.modules.contains_key(parent) {
                self.register_module(parent, Value::module(std::iter::empty::<(&str, Value)>()))?;
            }
            if let Some(Value(Kind::Module(attrs))) = self.modules.get(parent) {
                attrs.borrow_mut().insert(child.to_string(), value.clone());
            }
        }
        if let Some(Value(Kind::Module(old))) = self.modules.get(name) {
            if let Kind::Module(new) = &value.0 {
                if !Rc::ptr_eq(old, new) {
                    new.borrow_mut().extend(
                        old.borrow()
                            .iter()
                            .filter(|(key, _)| self.modules.contains_key(&format!("{name}.{key}")))
                            .map(|(k, v)| (k.clone(), v.clone())),
                    );
                }
            }
        }
        self.modules.insert(name.to_owned(), value);
        Ok(())
    }

    pub(crate) fn register_builtin(&mut self, name: &str, value: Value) {
        self.builtins.insert(name.to_owned(), value);
    }

    /// Parse and execute source in __main__. A Python SystemExit is returned as an error
    /// with class "SystemExit" and the requested status in the message.
    pub fn run(&mut self, source: &str) -> Result<()> {
        let ast = ast::parse(source)?;
        self.run_module(&ast)
    }

    fn run_module(&mut self, ast: &Node) -> Result<()> {
        let future_division = ast.named().any(|item| {
            item.kind == "future_import_statement"
                && item.named().any(|name| name.text() == "division")
        });
        let previous_division = std::mem::replace(&mut self.future_division, future_division);
        let result = self.block(ast);
        self.future_division = previous_division;
        match result? {
            Flow::Next => Ok(()),
            _ => Err(Error::new(
                "SyntaxError",
                "return, break or continue outside its scope",
            )),
        }
    }

    pub fn call(&mut self, callable: &Value, args: Args) -> Result<Value> {
        if self.call_depth >= 64 {
            return Err(Error::new("RuntimeError", "maximum call depth exceeded"));
        }
        self.call_depth += 1;
        let result = self.invoke(callable, args);
        self.call_depth -= 1;
        result
    }

    pub fn attribute(&self, object: &Value, name: &str) -> Result<Value> {
        self.get_attr(object, name)
    }

    fn spend(&mut self) -> Result<()> {
        if let Some(fuel) = &mut self.fuel {
            *fuel = fuel
                .checked_sub(1)
                .ok_or_else(|| Error::unsupported("AST visit budget exhausted"))?;
        }
        Ok(())
    }

    fn global_scope(&self) -> &Scope {
        let mut scope = self.scope.as_ref();
        while let Some(parent) = &scope.enclosing {
            scope = parent;
        }
        scope
    }

    fn lookup(&self, name: &str) -> Result<Value> {
        let mut scope = Some(self.scope.as_ref());
        while let Some(current) = scope {
            if current.bindings.globals.contains(name) {
                return self
                    .global_scope()
                    .locals
                    .borrow()
                    .get(name)
                    .or_else(|| self.builtins.get(name))
                    .cloned()
                    .ok_or_else(|| Error::new("NameError", name));
            }
            if let Some(value) = current.locals.borrow().get(name) {
                return Ok(value.clone());
            }
            if current.kind == ScopeKind::Function && current.bindings.locals.contains(name) {
                return Err(Error::new(
                    if std::ptr::eq(current, self.scope.as_ref()) {
                        "UnboundLocalError"
                    } else {
                        "NameError"
                    },
                    name,
                ));
            }
            scope = current.enclosing.as_deref();
        }
        self.builtins
            .get(name)
            .cloned()
            .ok_or_else(|| Error::new("NameError", name))
    }

    fn bind(&self, name: &str, value: Value) {
        let scope = if self.scope.bindings.globals.contains(name) {
            self.global_scope()
        } else {
            &self.scope
        };
        scope.locals.borrow_mut().insert(name.to_owned(), value);
    }

    fn new_scope(
        &mut self,
        kind: ScopeKind,
        enclosing: Option<ScopeRef>,
        bindings: Rc<Bindings>,
    ) -> ScopeRef {
        let scope = Rc::new(Scope {
            locals: Rc::default(),
            enclosing,
            bindings,
            kind,
        });
        if self.scopes.len() >= 1024 && self.scopes.len() % 1024 == 0 {
            self.scopes.retain(|scope| scope.strong_count() > 0);
        }
        self.scopes.push(Rc::downgrade(&scope));
        scope
    }

    fn block(&mut self, block: &Node) -> Result<Flow> {
        for statement in block.named() {
            let flow = self.statement(statement)?;
            if !matches!(flow, Flow::Next) {
                return Ok(flow);
            }
        }
        Ok(Flow::Next)
    }

    fn statement(&mut self, node: &Node) -> Result<Flow> {
        self.spend()?;
        self.execute(node).map_err(|error| error.at(node.line))
    }

    fn execute(&mut self, node: &Node) -> Result<Flow> {
        match node.kind {
            "expression_statement" => {
                let expression = node.first()?;
                if expression.kind == "identifier" && expression.text() == "print" {
                    self.write_stdout(b"\n")?;
                } else if expression.kind == "call"
                    && expression
                        .field("function")
                        .is_some_and(|name| name.kind == "identifier" && name.text() == "print")
                {
                    self.parenthesized_print(expression.need("arguments")?)?;
                } else {
                    self.expr(expression)?;
                }
            }
            "pass_statement" | "global_statement" => {}
            "return_statement" => {
                return Ok(Flow::Return(
                    node.named()
                        .next()
                        .map(|value| self.expr(value))
                        .transpose()?
                        .unwrap_or(Value::NONE),
                ));
            }
            "break_statement" => return Ok(Flow::Break),
            "continue_statement" => return Ok(Flow::Continue),
            "print_statement" => self.print_statement(node)?,
            "assert_statement" => {
                let mut expressions = node.named();
                let test = expressions.next().ok_or_else(|| node.unsupported())?;
                if !self.expr(test)?.truth() {
                    let message = expressions
                        .next()
                        .map(|expr| self.expr(expr))
                        .transpose()?
                        .map_or_else(String::new, |value| value.string());
                    return Err(Error::new("AssertionError", message));
                }
            }
            "if_statement" => {
                if self.expr(node.need("condition")?)?.truth() {
                    return self.block(node.need("consequence")?);
                }
                for clause in node.fields("alternative") {
                    if clause.kind == "else_clause" {
                        return self.block(clause.need("body")?);
                    }
                    if self.expr(clause.need("condition")?)?.truth() {
                        return self.block(clause.need("consequence")?);
                    }
                }
            }
            "while_statement" => {
                while self.expr(node.need("condition")?)?.truth() {
                    match self.block(node.need("body")?)? {
                        Flow::Next | Flow::Continue => {}
                        Flow::Break => return Ok(Flow::Next),
                        returned => return Ok(returned),
                    }
                }
                if let Some(otherwise) = node.field("alternative") {
                    return self.block(otherwise.need("body")?);
                }
            }
            "for_statement" => {
                if node.tokens().any(|child| child.kind == "async") {
                    return Err(node.unsupported());
                }
                let iterable = self.expr(node.need("right")?)?;
                for item in self.iterate(&iterable)? {
                    self.assign(node.need("left")?, item)?;
                    match self.block(node.need("body")?)? {
                        Flow::Next | Flow::Continue => {}
                        Flow::Break => return Ok(Flow::Next),
                        returned => return Ok(returned),
                    }
                }
                if let Some(otherwise) = node.field("alternative") {
                    return self.block(otherwise.need("body")?);
                }
            }
            "try_statement" => return self.try_block(node),
            "raise_statement" => return self.raise(node),
            "function_definition" => {
                let function = self.function(node, false)?;
                self.bind(node.need("name")?.text(), function);
            }
            "class_definition" => self.class(node)?,
            "import_statement" => {
                for item in node.fields("name") {
                    let (name, alias) = import_binding(item)?;
                    let imported = self.module(name)?;
                    if let Some(alias) = alias {
                        self.bind(alias, imported);
                    } else {
                        let root = name.split('.').next().unwrap_or(name);
                        let module = self.module(root)?;
                        self.bind(root, module);
                    }
                }
            }
            "import_from_statement" => {
                let module = self.module(node.need("module_name")?.text())?;
                if node.named().any(|item| item.kind == "wildcard_import") {
                    return Err(node.unsupported());
                }
                for item in node.fields("name") {
                    let (name, alias) = import_binding(item)?;
                    let item = match self.get_attr(&module, name) {
                        Err(error) if error.class == "AttributeError" => {
                            match self
                                .module(&format!("{}.{name}", node.need("module_name")?.text()))
                            {
                                Err(error) if error.class == "ImportError" => {
                                    return Err(Error::new(
                                        "ImportError",
                                        format!("cannot import name {name}"),
                                    ));
                                }
                                result => result?,
                            }
                        }
                        result => result?,
                    };
                    self.bind(alias.unwrap_or(name), item);
                }
            }
            "future_import_statement" => {
                if node.named().any(|name| {
                    !matches!(
                        name.text(),
                        "division"
                            | "with_statement"
                            | "absolute_import"
                            | "nested_scopes"
                            | "generators"
                    )
                }) {
                    return Err(node.unsupported());
                }
            }
            _ => return Err(node.unsupported()),
        }
        Ok(Flow::Next)
    }

    fn module(&mut self, name: &str) -> Result<Value> {
        if let Some(module) = self.modules.get(name) {
            return Ok(module.clone());
        }
        if let Some((parent, _)) = name.rsplit_once('.') {
            self.module(parent)?;
            if let Some(module) = self.modules.get(name) {
                return Ok(module.clone());
            }
        }
        let missing = || Error::new("ImportError", format!("No module named {name}"));
        let source = self.module_loader.as_mut().ok_or_else(missing)?(name)?.ok_or_else(missing)?;
        if self.import_depth >= 32 {
            return Err(Error::new(
                "ImportError",
                "maximum source import depth exceeded",
            ));
        }
        let ast = ast::parse(&source.source)?;
        let scope = self.new_scope(ScopeKind::Module, None, Rc::default());
        scope.locals.borrow_mut().extend([
            ("__name__".to_owned(), Value::bytes(name)),
            ("__file__".to_owned(), Value::bytes(source.filename)),
        ]);
        let module = Value(Kind::Module(scope.locals.clone()));
        // Use the in-progress module for circular imports.
        self.register_module(name, module.clone())?;
        let previous = std::mem::replace(&mut self.scope, scope);
        self.import_depth += 1;
        let result = self.run_module(&ast);
        self.import_depth -= 1;
        self.scope = previous;
        if let Err(error) = result {
            self.modules.remove(name);
            if let Some((parent, child)) = name.rsplit_once('.') {
                if let Some(Value(Kind::Module(attrs))) = self.modules.get(parent) {
                    attrs.borrow_mut().remove(child);
                }
            }
            return Err(error);
        }
        Ok(module)
    }

    fn class(&mut self, node: &Node) -> Result<()> {
        if node.field("type_parameters").is_some() {
            return Err(node.unsupported());
        }
        let name = node.need("name")?.text();
        let base = if let Some(args) = node.field("superclasses") {
            let mut bases = args.named();
            let base = bases.next().map(|base| self.expr(base)).transpose()?;
            if bases.next().is_some()
                || base
                    .as_ref()
                    .is_some_and(|v| !matches!(v.0, Kind::Class { .. }))
            {
                return Err(node.unsupported());
            }
            base.map(Rc::new)
        } else {
            None
        };
        let body = node.need("body")?;
        let scope = self.new_scope(
            ScopeKind::Class,
            Some(self.scope.clone()),
            Bindings::for_body(body),
        );
        let previous = std::mem::replace(&mut self.scope, scope.clone());
        let result = self.block(body);
        self.scope = previous;
        if !matches!(result?, Flow::Next) {
            return Err(node.unsupported());
        }
        scope
            .locals
            .borrow_mut()
            .insert("__name__".into(), Value::bytes(name));
        self.bind(
            name,
            Value(Kind::Class {
                attrs: scope.locals.clone(),
                base,
            }),
        );
        Ok(())
    }

    fn try_block(&mut self, node: &Node) -> Result<Flow> {
        let result = self.block(node.need("body")?);
        if result.as_ref().is_err_and(|error| error.fatal) {
            return result;
        }
        let mut result = match result {
            Ok(Flow::Next) => match node.named().find(|item| item.kind == "else_clause") {
                Some(otherwise) => self.block(otherwise.need("body")?),
                None => Ok(Flow::Next),
            },
            Err(error) => self.handle_exception(node, error),
            result => result,
        };
        if let Some(finally) = node.named().find(|item| item.kind == "finally_clause") {
            if result.as_ref().is_err_and(|error| error.fatal) {
                return result;
            }
            if let Err(error) = &result {
                self.active_exceptions.push(error.clone());
            }
            let cleanup = self.block(finally.first()?);
            if result.is_err() {
                self.active_exceptions.pop();
            }
            if !matches!(cleanup, Ok(Flow::Next)) {
                result = cleanup;
            }
        }
        result
    }

    fn handle_exception(&mut self, node: &Node, error: Error) -> Result<Flow> {
        for handler in node.named().filter(|node| node.kind == "except_clause") {
            if handler.tokens().any(|node| node.kind == "*") {
                return Err(handler.unsupported());
            }
            let mut values = handler.fields("value");
            if let Some(pattern) = values.next() {
                if !matches_exception(&self.expr(pattern)?, &error.class)? {
                    continue;
                }
            }
            if let Some(target) = handler.field("alias").or_else(|| values.next()) {
                self.assign(target, Value(Kind::Exception(Rc::new(error.clone()))))?;
            }
            if values.next().is_some() {
                return Err(handler.unsupported());
            }
            let body = handler
                .named()
                .find(|node| node.kind == "block")
                .ok_or_else(|| handler.unsupported())?;
            self.active_exceptions.push(error);
            let result = self.block(body);
            self.active_exceptions.pop();
            return result;
        }
        Err(error)
    }

    fn raise(&mut self, node: &Node) -> Result<Flow> {
        let Some(first) = node.named().next() else {
            return Err(self
                .active_exceptions
                .last()
                .cloned()
                .unwrap_or_else(|| Error::new("RuntimeError", "no active exception")));
        };
        let mut values = if first.kind == "expression_list" {
            first.named().collect::<Vec<_>>().into_iter()
        } else {
            node.named().collect::<Vec<_>>().into_iter()
        };
        let kind = self.expr(values.next().ok_or_else(|| node.unsupported())?)?;
        let message = values.next().map(|value| self.expr(value)).transpose()?;
        if values.next().is_some() || node.field("cause").is_some() {
            return Err(node.unsupported());
        }
        Err(match (kind.0, message) {
            (Kind::Exception(error), None) => (*error).clone(),
            (Kind::ExceptionType(class), message) => Error::new(
                class.as_ref(),
                message.map_or_else(String::new, |value| value.string()),
            ),
            _ => Error::type_error("exceptions must be classes or instances"),
        })
    }

    fn parenthesized_print(&mut self, arguments: &Node) -> Result<()> {
        if arguments.kind != "argument_list" {
            return Err(arguments.unsupported());
        }
        let args = self.arguments(arguments)?;
        if !args.named.is_empty()
            || arguments
                .named()
                .any(|item| item.kind == "list_splat" || item.kind == "dictionary_splat")
        {
            return Err(Error::new("SyntaxError", "invalid print statement"));
        }
        let value = match args.positional.as_slice() {
            [one] if !arguments.tokens().any(|item| item.kind == ",") => one.clone(),
            _ => Value::tuple(args.positional),
        };
        let mut output = printable(&value);
        output.push(b'\n');
        self.write_stdout(&output)
    }

    fn print_statement(&mut self, node: &Node) -> Result<()> {
        let mut target = None;
        let mut values = Vec::new();
        for item in node.named() {
            if item.kind == "chevron" {
                target = Some(self.expr(item.first()?)?);
            } else {
                let value = self.expr(item)?;
                values.push(printable(&value));
            }
        }
        let trailing = node.tokens().last().is_some_and(|item| item.kind == ",");
        let mut text = values.join(&b' ');
        text.push(if trailing { b' ' } else { b'\n' });
        if let Some(target) = target {
            let write = self.get_attr(&target, "write")?;
            self.call(
                &write,
                Args {
                    positional: vec![Value::bytes(text)],
                    ..Args::default()
                },
            )?;
        } else {
            self.write_stdout(&text)?;
        }
        Ok(())
    }

    pub(crate) fn write_stdout(&mut self, text: &[u8]) -> Result<()> {
        self.stdout
            .write_all(text)
            .map_err(|error| Error::new("IOError", error.to_string()))
    }

    pub(crate) fn flush_stdout(&mut self) -> Result<()> {
        self.stdout
            .flush()
            .map_err(|error| Error::new("IOError", error.to_string()))
    }

    pub(crate) fn pending_exception(&self) -> Option<Error> {
        self.active_exceptions.last().cloned()
    }

    fn iterate(&self, value: &Value) -> Result<Box<dyn Iterator<Item = Value>>> {
        match &value.0 {
            Kind::List(items) => {
                let items = items.clone();
                let mut index = 0;
                Ok(Box::new(std::iter::from_fn(move || {
                    let item = items.borrow().get(index).cloned()?;
                    index += 1;
                    Some(item)
                })))
            }
            Kind::XRange { start, step, len } => {
                let (start, step) = (i128::from(*start), i128::from(*step));
                Ok(Box::new((0..*len).map(move |index| {
                    Value::int(start + step * index as i128)
                })))
            }
            _ => Ok(Box::new(value.items()?.into_iter())),
        }
    }

    fn expr(&mut self, node: &Node) -> Result<Value> {
        self.spend()?;
        self.evaluate(node).map_err(|error| error.at(node.line))
    }

    fn evaluate(&mut self, node: &Node) -> Result<Value> {
        match node.kind {
            "identifier" => self.lookup(node.text()),
            "none" => Ok(Value::NONE),
            "true" => Ok(Value::bool(true)),
            "false" => Ok(Value::bool(false)),
            "integer" => ops::integer(node.text()),
            "float" => node
                .text()
                .parse::<f64>()
                .map(Value::float)
                .map_err(|_| Error::new("SyntaxError", "invalid float")),
            "string" => ops::literal(node.text()),
            "concatenated_string" => {
                let mut items = node.named();
                let mut value = self.expr(items.next().ok_or_else(|| node.unsupported())?)?;
                for item in items {
                    value = ops::binary("+", value, self.expr(item)?, self.future_division)?;
                }
                Ok(value)
            }
            "parenthesized_expression" => self.expr(node.first()?),
            "list" => Ok(Value::list(self.values(node.named())?)),
            "tuple" | "expression_list" => Ok(Value::tuple(self.values(node.named())?)),
            "dictionary" => {
                let value = Value(Kind::Dict(Rc::default()));
                for pair in node.named() {
                    let key = self.expr(pair.need("key")?)?;
                    let item = self.expr(pair.need("value")?)?;
                    self.set_item(&value, key, item)?;
                }
                Ok(value)
            }
            "attribute" => {
                let object = self.expr(node.need("object")?)?;
                self.get_attr(&object, node.need("attribute")?.text())
            }
            "subscript" => {
                let value = self.expr(node.need("value")?)?;
                let sub = node.need("subscript")?;
                if node.fields("subscript").count() != 1 {
                    return Err(node.unsupported());
                }
                if sub.kind == "slice" {
                    let (start, end, step) = self.slice_bounds(sub)?;
                    ops::slice(&value, start, end, step)
                } else {
                    ops::subscript(&value, &self.expr(sub)?)
                }
            }
            "call" => {
                let callee = self.expr(node.need("function")?)?;
                let args = self.arguments(node.need("arguments")?)?;
                self.call(&callee, args)
            }
            "lambda" => self.function(node, true),
            "list_comprehension" => {
                let body = node.need("body")?;
                let clauses = node
                    .named()
                    .filter(|item| matches!(item.kind, "for_in_clause" | "if_clause"))
                    .cloned()
                    .collect::<Vec<_>>();
                let mut output = Vec::new();
                self.comprehend(body, &clauses, &mut output)?;
                Ok(Value::list(output))
            }
            "conditional_expression" => {
                let nodes = node.named().collect::<Vec<_>>();
                match nodes.as_slice() {
                    [yes, condition, no] => {
                        if self.expr(condition)?.truth() {
                            self.expr(yes)
                        } else {
                            self.expr(no)
                        }
                    }
                    _ => Err(node.unsupported()),
                }
            }
            "unary_operator" => ops::unary(
                node.need("operator")?.text(),
                self.expr(node.need("argument")?)?,
            ),
            "not_operator" => ops::unary("not", self.expr(node.need("argument")?)?),
            "boolean_operator" => {
                let left = self.expr(node.need("left")?)?;
                let is_and = node.need("operator")?.kind == "and";
                if left.truth() == is_and {
                    self.expr(node.need("right")?)
                } else {
                    Ok(left)
                }
            }
            "binary_operator" => {
                let left = self.expr(node.need("left")?)?;
                let right = self.expr(node.need("right")?)?;
                ops::binary(
                    node.need("operator")?.text(),
                    left,
                    right,
                    self.future_division,
                )
            }
            "comparison_operator" => {
                let mut tokens = node.tokens();
                let mut left = self.expr(tokens.next().ok_or_else(|| node.unsupported())?)?;
                while let Some(op) = tokens.next() {
                    let right = self.expr(tokens.next().ok_or_else(|| node.unsupported())?)?;
                    let operator = op.text().split_whitespace().collect::<Vec<_>>().join(" ");
                    if !ops::compare(&operator, &left, &right)? {
                        return Ok(Value::bool(false));
                    }
                    left = right;
                }
                Ok(Value::bool(true))
            }
            "assignment" => self.assignment(node),
            "augmented_assignment" => self.augmented_assignment(node),
            _ => Err(node.unsupported()),
        }
    }

    fn values<'a>(&mut self, nodes: impl Iterator<Item = &'a Ast>) -> Result<Vec<Value>> {
        nodes.map(|node| self.expr(node)).collect()
    }

    fn slice_bounds(&mut self, node: &Node) -> Result<(Option<i64>, Option<i64>, Option<i64>)> {
        let mut parts = [None; 3];
        let mut index = 0;
        for item in node.tokens() {
            if item.kind == ":" {
                index += 1;
            } else {
                let value = self.expr(item)?;
                if !matches!(value.0, Kind::None) {
                    *parts.get_mut(index).ok_or_else(|| node.unsupported())? =
                        Some(value.as_i64()?);
                }
            }
        }
        Ok((parts[0], parts[1], parts[2]))
    }

    fn comprehend(&mut self, body: &Node, clauses: &[Ast], output: &mut Vec<Value>) -> Result<()> {
        let Some((clause, remaining)) = clauses.split_first() else {
            output.push(self.expr(body)?);
            return Ok(());
        };
        if clause.kind == "if_clause" {
            if self.expr(clause.first()?)?.truth() {
                self.comprehend(body, remaining, output)?;
            }
        } else {
            if clause.tokens().any(|child| child.kind == "async") {
                return Err(clause.unsupported());
            }
            let value = self.expr(clause.need("right")?)?;
            for item in self.iterate(&value)? {
                self.spend()?;
                self.assign(clause.need("left")?, item)?;
                self.comprehend(body, remaining, output)?;
            }
        }
        Ok(())
    }

    fn arguments(&mut self, node: &Node) -> Result<Args> {
        if node.kind != "argument_list" {
            return Err(node.unsupported());
        }
        let mut args = Args::default();
        for item in node.named() {
            match item.kind {
                "keyword_argument" => put_argument(
                    &mut args,
                    item.need("name")?.text(),
                    self.expr(item.need("value")?)?,
                )?,
                "list_splat" => {
                    let value = self.expr(item.first()?)?;
                    args.positional.extend(self.iterate(&value)?);
                }
                "dictionary_splat" => {
                    let mapping = self.expr(item.first()?)?;
                    let Kind::Dict(values) = mapping.0 else {
                        return Err(Error::type_error("expected dictionary for ** arguments"));
                    };
                    for (key, value) in values.borrow().iter() {
                        put_argument(&mut args, key.as_text()?, value.clone())?;
                    }
                }
                _ => args.positional.push(self.expr(item)?),
            }
        }
        Ok(args)
    }

    fn assignment(&mut self, node: &Node) -> Result<Value> {
        if node.field("type").is_some() {
            return Err(node.unsupported());
        }
        let mut targets = vec![node.need("left")?];
        let mut right = node.need("right")?;
        while right.kind == "assignment" {
            targets.push(right.need("left")?);
            right = right.need("right")?;
        }
        let value = self.expr(right)?;
        for target in targets {
            self.assign(target, value.clone())?;
        }
        Ok(value)
    }

    fn augmented_assignment(&mut self, node: &Node) -> Result<Value> {
        let target = self.location(node.need("left")?)?;
        let current = self.load(&target)?;
        let value = self.expr(node.need("right")?)?;
        let op = node.need("operator")?.text().trim_end_matches('=');
        let result = match (&current.0, op) {
            (Kind::List(items), "+") => {
                let addition = value.items()?;
                items.borrow_mut().extend(addition);
                current
            }
            (Kind::List(items), "*") => {
                let repeated =
                    ops::binary("*", current.clone(), value, self.future_division)?.items()?;
                *items.borrow_mut() = repeated;
                current
            }
            _ => ops::binary(op, current, value, self.future_division)?,
        };
        self.store(target, result.clone())?;
        Ok(result)
    }

    fn assign(&mut self, target: &Node, value: Value) -> Result<()> {
        match target.kind {
            "pattern_list" | "tuple_pattern" | "list_pattern" | "tuple" | "list" => {
                let targets = target.named().collect::<Vec<_>>();
                let values = value.items()?;
                if values.len() != targets.len() {
                    return Err(Error::new("ValueError", "unpacking length mismatch"));
                }
                for (target, value) in targets.into_iter().zip(values) {
                    self.assign(target, value)?;
                }
                Ok(())
            }
            _ => {
                let location = self.location(target)?;
                self.store(location, value)
            }
        }
    }

    fn location(&mut self, node: &Node) -> Result<Location> {
        match node.kind {
            "identifier" => Ok(Location::Name(node.text().to_owned())),
            "attribute" => Ok(Location::Attr(
                self.expr(node.need("object")?)?,
                node.need("attribute")?.text().to_owned(),
            )),
            "subscript" if node.fields("subscript").count() == 1 => Ok(Location::Item(
                self.expr(node.need("value")?)?,
                self.expr(node.need("subscript")?)?,
            )),
            "parenthesized_expression" => self.location(node.first()?),
            _ => Err(node.unsupported()),
        }
    }

    fn load(&self, location: &Location) -> Result<Value> {
        match location {
            Location::Name(name) => self.lookup(name),
            Location::Attr(value, name) => self.get_attr(value, name),
            Location::Item(value, key) => ops::subscript(value, key),
        }
    }

    fn store(&self, location: Location, value: Value) -> Result<()> {
        match location {
            Location::Name(name) => {
                self.bind(&name, value);
                Ok(())
            }
            Location::Attr(object, name) => match &object.0 {
                Kind::Module(attrs) | Kind::Class { attrs, .. } | Kind::Instance { attrs, .. } => {
                    attrs.borrow_mut().insert(name, value);
                    Ok(())
                }
                Kind::Host(object) => object.native.set(&name, value),
                _ => Err(Error::new("AttributeError", name)),
            },
            Location::Item(object, key) => self.set_item(&object, key, value),
        }
    }

    fn set_item(&self, object: &Value, key: Value, value: Value) -> Result<()> {
        match &object.0 {
            Kind::List(items) => {
                let mut items = items.borrow_mut();
                let index = ops::index(&key, items.len())?;
                items[index] = value;
            }
            Kind::Dict(items) => {
                let mut items = items.borrow_mut();
                if let Some(index) = ops::dict_index(&items, &key)? {
                    items[index].1 = value;
                } else {
                    items.push((key, value));
                }
            }
            _ => return Err(Error::type_error("object does not support item assignment")),
        }
        Ok(())
    }

    fn get_attr(&self, object: &Value, name: &str) -> Result<Value> {
        match &object.0 {
            Kind::Module(attrs) => attrs.borrow().get(name).cloned(),
            Kind::Class { attrs, base } => {
                if let Some(value) = attrs.borrow().get(name).cloned() {
                    Some(value)
                } else if let Some(base) = base {
                    return self.get_attr(base, name);
                } else {
                    None
                }
            }
            Kind::Instance { class, attrs } => {
                if let Some(value) = attrs.borrow().get(name).cloned() {
                    return Ok(value);
                }
                let item = self.get_attr(class, name)?;
                return if matches!(item.0, Kind::Function(_)) {
                    Ok(Value(Kind::Bound {
                        function: Rc::new(item),
                        receiver: Rc::new(object.clone()),
                    }))
                } else {
                    Ok(item)
                };
            }
            Kind::Host(object) => return object.native.get(name),
            _ => return stdlib::method(object, name),
        }
        .ok_or_else(|| Error::new("AttributeError", name))
    }

    fn function(&mut self, node: &Node, lambda: bool) -> Result<Value> {
        if node.field("return_type").is_some()
            || node.field("type_parameters").is_some()
            || node.tokens().any(|node| node.kind == "async")
        {
            return Err(node.unsupported());
        }
        let mut params = Vec::new();
        let mut rest = None;
        let mut keywords = None;
        if let Some(parameters) = node.field("parameters") {
            for param in parameters.named() {
                match param.kind {
                    "identifier" if rest.is_none() => params.push((param.text().to_string(), None)),
                    "default_parameter" if rest.is_none() => params.push((
                        param.need("name")?.text().to_string(),
                        Some(self.expr(param.need("value")?)?),
                    )),
                    "list_splat_pattern" => rest = Some(param.first()?.text().to_string()),
                    "dictionary_splat_pattern" => {
                        keywords = Some(param.first()?.text().to_string())
                    }
                    _ => return Err(param.unsupported()),
                }
            }
        }
        let enclosing = if self.scope.kind == ScopeKind::Class {
            self.scope
                .enclosing
                .clone()
                .ok_or_else(|| node.unsupported())?
        } else {
            self.scope.clone()
        };
        let body = node.need("body")?.clone();
        let function = Function {
            name: node
                .field("name")
                .map_or("<lambda>", |name| name.text())
                .to_owned(),
            params,
            rest,
            keywords,
            bindings: Bindings::for_body(&body),
            body,
            enclosing,
            lambda,
            future_division: self.future_division,
        };
        Ok(Value(Kind::Function(Rc::new(function))))
    }

    fn invoke(&mut self, callable: &Value, mut args: Args) -> Result<Value> {
        match &callable.0 {
            Kind::Native(callback) => callback(self, args),
            Kind::Bound { function, receiver } => {
                args.positional.insert(0, receiver.as_ref().clone());
                self.invoke(function, args)
            }
            Kind::Function(function) => self.call_function(function, args),
            Kind::ExceptionType(class) => {
                if !args.named.is_empty() {
                    return Err(Error::type_error("exception keyword arguments"));
                }
                let message = match args.positional.as_slice() {
                    [] => String::new(),
                    [one] => one.string(),
                    _ => return Err(Error::unsupported("multiple exception arguments")),
                };
                Ok(Value(Kind::Exception(Rc::new(Error::new(
                    class.as_ref(),
                    message,
                )))))
            }
            Kind::Class { .. } => {
                let instance = Value(Kind::Instance {
                    class: Rc::new(callable.clone()),
                    attrs: Rc::default(),
                });
                match self.get_attr(&instance, "__init__") {
                    Ok(init) => {
                        if !matches!(self.call(&init, args)?.0, Kind::None) {
                            return Err(Error::type_error("__init__ must return None"));
                        }
                    }
                    Err(err) if err.class == "AttributeError" => {
                        if !args.positional.is_empty() || !args.named.is_empty() {
                            return Err(Error::type_error("constructor takes no arguments"));
                        }
                    }
                    Err(err) => return Err(err),
                }
                Ok(instance)
            }
            _ => Err(Error::type_error(format!(
                "{} is not callable",
                callable.type_name()
            ))),
        }
    }

    fn call_function(&mut self, function: &Function, mut args: Args) -> Result<Value> {
        let scope = self.new_scope(
            ScopeKind::Function,
            Some(function.enclosing.clone()),
            function.bindings.clone(),
        );
        let mut positional = args.positional.into_iter();
        for (name, default) in &function.params {
            let pos = positional.next();
            let named = args.named.remove(name);
            if pos.is_some() && named.is_some() {
                return Err(Error::type_error(format!("multiple values for {name}")));
            }
            let value = pos.or(named).or_else(|| default.clone()).ok_or_else(|| {
                Error::type_error(format!("{} missing argument {name}", function.name))
            })?;
            scope.locals.borrow_mut().insert(name.to_owned(), value);
        }
        if let Some(rest) = &function.rest {
            scope
                .locals
                .borrow_mut()
                .insert(rest.clone(), Value::tuple(positional.collect()));
        } else if positional.next().is_some() {
            return Err(Error::type_error(format!(
                "{} received too many arguments",
                function.name
            )));
        }
        if let Some(keywords) = &function.keywords {
            let dict = args
                .named
                .into_iter()
                .map(|(key, value)| (Value::bytes(key), value))
                .collect();
            scope.locals.borrow_mut().insert(
                keywords.clone(),
                Value(Kind::Dict(Rc::new(RefCell::new(dict)))),
            );
        } else if let Some(name) = args.named.keys().next() {
            return Err(Error::type_error(format!(
                "{} received unknown keyword {name}",
                function.name
            )));
        }
        let previous = std::mem::replace(&mut self.scope, scope);
        let previous_division =
            std::mem::replace(&mut self.future_division, function.future_division);
        let result = if function.lambda {
            self.expr(&function.body)
        } else {
            match self.block(&function.body) {
                Ok(Flow::Return(value)) => Ok(value),
                Ok(Flow::Next) => Ok(Value::NONE),
                Ok(_) => Err(Error::new("SyntaxError", "break or continue outside loop")),
                Err(error) => Err(error),
            }
        };
        self.scope = previous;
        self.future_division = previous_division;
        result
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // Functions can close over their own definition scope.
        for scope in self.scopes.iter().filter_map(Weak::upgrade) {
            scope.locals.borrow_mut().clear();
        }
    }
}

enum Location {
    Name(String),
    Attr(Value, String),
    Item(Value, Value),
}

fn printable(value: &Value) -> Vec<u8> {
    match &value.0 {
        Kind::Bytes(bytes) => bytes.to_vec(),
        _ => value.string().into_bytes(),
    }
}

fn import_binding(node: &Node) -> Result<(&str, Option<&str>)> {
    match node.kind {
        "aliased_import" => Ok((node.need("name")?.text(), Some(node.need("alias")?.text()))),
        "dotted_name" | "identifier" => Ok((node.text(), None)),
        _ => Err(node.unsupported()),
    }
}

fn put_argument(args: &mut Args, key: &str, value: Value) -> Result<()> {
    if args.named.insert(key.to_owned(), value).is_some() {
        return Err(Error::type_error(format!("duplicate keyword {key}")));
    }
    Ok(())
}

fn matches_exception(pattern: &Value, class: &str) -> Result<bool> {
    if let Kind::Tuple(items) = &pattern.0 {
        for item in items.iter() {
            if matches_exception(item, class)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    let Kind::ExceptionType(expected) = &pattern.0 else {
        return Err(Error::type_error("expected an exception class"));
    };
    Ok(class == expected.as_ref()
        || expected.as_ref() == "BaseException"
        || (expected.as_ref() == "Exception"
            && !matches!(class, "SystemExit" | "KeyboardInterrupt" | "GeneratorExit"))
        || matches!(
            (expected.as_ref(), class),
            ("LookupError", "IndexError" | "KeyError")
                | ("NameError", "UnboundLocalError")
                | ("EnvironmentError", "IOError" | "OSError" | "WindowsError")
                | ("ArithmeticError", "ZeroDivisionError" | "OverflowError")
                | (
                    "ValueError",
                    "UnicodeError" | "UnicodeDecodeError" | "UnicodeEncodeError"
                )
        ))
}

fn bindings(node: &Node, globals: &mut BTreeSet<String>, locals: &mut BTreeSet<String>) {
    fn target(node: &Node, locals: &mut BTreeSet<String>) {
        match node.kind {
            "identifier" => {
                locals.insert(node.text().to_owned());
            }
            "pattern_list"
            | "tuple_pattern"
            | "list_pattern"
            | "tuple"
            | "list"
            | "parenthesized_expression" => {
                for child in node.named() {
                    target(child, locals);
                }
            }
            _ => {}
        }
    }
    match node.kind {
        "function_definition" | "class_definition" => {
            if let Some(name) = node.field("name") {
                target(name, locals);
            }
            return;
        }
        "lambda" => return,
        "global_statement" => {
            globals.extend(node.named().map(|item| item.text().to_owned()));
        }
        "assignment" | "augmented_assignment" | "for_statement" | "for_in_clause" => {
            if let Some(left) = node.field("left") {
                target(left, locals);
            }
        }
        "import_statement" | "import_from_statement" => {
            for item in node.fields("name") {
                if let Ok((name, alias)) = import_binding(item) {
                    locals.insert(
                        alias
                            .unwrap_or_else(|| name.split('.').next().unwrap_or(name))
                            .to_owned(),
                    );
                }
            }
        }
        "except_clause" => {
            if let Some(alias) = node.field("alias").or_else(|| node.fields("value").nth(1)) {
                target(alias, locals);
            }
        }
        _ => {}
    }
    for child in node.named() {
        bindings(child, globals, locals);
    }
}

fn missing_path(path: &Path) -> bool {
    fs::symlink_metadata(path).is_err_and(|error| error.kind() == ErrorKind::NotFound)
}

fn resolve_ascii_case(path: &Path) -> Option<PathBuf> {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        resolved.push(component);
        let Component::Normal(name) = component else {
            continue;
        };
        if !missing_path(&resolved) {
            continue;
        }
        resolved.pop();
        let directory = if resolved.as_os_str().is_empty() {
            Path::new(".")
        } else {
            &resolved
        };
        let mut found = None;
        for entry in fs::read_dir(directory).ok()? {
            let entry = entry.ok()?.file_name();
            if entry
                .as_encoded_bytes()
                .eq_ignore_ascii_case(name.as_encoded_bytes())
                && found.replace(entry).is_some()
            {
                return None;
            }
        }
        resolved.push(found?);
    }
    Some(resolved)
}
