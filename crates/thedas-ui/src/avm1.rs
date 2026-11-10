//! A few hand-assembled AVM1 (ActionScript 2) actions injected into movies so the engine can
//! talk to them the way Scaleform let the game.
//!
//! - Scaleform games call movie functions by path (`GFxMovie::Invoke("_global.GUISystem.Start")`);
//!   Ruffle only lets the host call functions registered with `ExternalInterface.addCallback`.
//!   The injected code registers one such callback, [`INVOKE_CALLBACK`], taking an object path,
//!   a method name and an argument array, and applies the method to the object.
//! - The movies' `ExternalCommands` read engine answers by sending an `fscommand` and returning
//!   `ExternalCommands.fsc_retval`, which the game set from its fscommand handler. Ruffle's
//!   fscommand handler cannot touch the movie, so each such function is wrapped: the original
//!   sends the fscommand (the host computes and keeps the answer), then the wrapper returns
//!   `ExternalInterface.call(RETVAL_CALL)`.
//! - Flash Player (and Ruffle after it) hands an empty string passed through
//!   `ExternalInterface` to an SWF older than version 9 as `"null"`; Scaleform passed the
//!   game's values directly, and the movies test for empty text (conversation.gfx's
//!   continue-only reply). The host sends empty strings as [`EMPTY`], and every way a value
//!   comes in (`GetValue`, `GetString`, wrapped fscommands, [`INVOKE_CALLBACK`] arguments)
//!   turns it back into `""`.

/// The callback name the injected code registers.
pub const INVOKE_CALLBACK: &str = "thedas_invoke";

/// An empty string on its way into a movie (a zero-width space, which Flash's empty-string
/// conversion leaves alone); the injected code turns it back into `""`.
pub const EMPTY: &str = "\u{200B}";

/// `ExternalCommands` functions returning an engine value straight from
/// `ExternalInterface.call`; wrapped to turn [`EMPTY`] back into `""`.
pub const VALUE_GETTERS: &[&str] = &["GetValue", "GetString"];

/// The function the host calls once, after starting a movie, to wrap [`PRE_CALLS`] (the
/// movies define those classes after the first frame's actions run).
pub const INSTALL: &str = "_global.thedas_install";

/// The callback name for setting a movie variable (`object path`, `member`, `value`).
pub const SET_CALLBACK: &str = "thedas_set";

/// The `ExternalInterface.call` name wrappers use to fetch the last fscommand's answer.
pub const RETVAL_CALL: &str = "thedas_retval";

/// `ExternalCommands` functions that return `fsc_retval` (from the movies' bytecode).
pub const RETURNING_FSCOMMANDS: &[&str] = &[
    "GetNumMembers",
    "ListMemberNames",
    "ReplaceDesignerTags",
    "GetNum2DARows",
    "Get2DAValue",
    "Get2DAValueByColumnName",
    "Get2DAValueByIndex",
    "Get2DAValueByRowIndexColumnHash",
    "Get2DAValueByRowIndexColumnName",
    "GetNumM2DARows",
    "GetM2DAValue",
    "GetM2DAValueByIndex",
    "GetTlkString",
    "FormatFloat",
];

/// Functions the game answers by setting a movie variable while the function runs (Scaleform's
/// `GFxMovie::SetVariable` from inside an `ExternalInterface` handler, which Ruffle's handlers
/// cannot do). Each is wrapped to set the variable from `ExternalInterface.call(call name, first
/// argument)` before the original runs, so the original finds it set.
///
/// - `SharedLibrary.Ability.LoadFromToken(token)` calls `GetAbilityAttributes` and reads
///   `_root.ReturnedAbilityAttributes` (its register 1 is `_root`): id, use type, icon, GUI
///   type, prerequisite, display state.
pub const PRE_CALLS: &[PreCall] = &[PreCall {
    object: &["_global", "SharedLibrary", "Ability"],
    function: "LoadFromToken",
    target: &["_root"],
    variable: "ReturnedAbilityAttributes",
    call: "GetAbilityAttributes",
}];

/// One of [`PRE_CALLS`].
pub struct PreCall {
    /// The path of the object holding the function.
    pub object: &'static [&'static str],
    pub function: &'static str,
    /// The path of the object holding the variable.
    pub target: &'static [&'static str],
    pub variable: &'static str,
    /// The `ExternalInterface` call answering it.
    pub call: &'static str,
}

const SET_MEMBER: u8 = 0x4f;
const SET_VARIABLE: u8 = 0x1d;
const DEFINE_LOCAL: u8 = 0x3c;
const STRICT_EQUALS: u8 = 0x66;
const NOT: u8 = 0x12;
const LESS2: u8 = 0x48;
const INCREMENT: u8 = 0x50;
const IF: u8 = 0x9d;
const JUMP: u8 = 0x99;

const GET_VARIABLE: u8 = 0x1c;
const GET_MEMBER: u8 = 0x4e;
const CALL_METHOD: u8 = 0x52;
const RETURN: u8 = 0x3e;
const POP: u8 = 0x17;
const PUSH: u8 = 0x96;
const DEFINE_FUNCTION: u8 = 0x9b;

enum Push<'a> {
    Str(&'a str),
    Int(i32),
    Null,
}

fn get_path(out: &mut Vec<u8>, path: &[&str]) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    push(out, &[Push::Str(first)]);
    out.push(GET_VARIABLE);
    for p in rest {
        push(out, &[Push::Str(p)]);
        out.push(GET_MEMBER);
    }
}

fn function(params: &[&str], body: &[u8]) -> Vec<u8> {
    let mut header = vec![0];
    header.extend_from_slice(&(params.len() as u16).to_le_bytes());
    for p in params {
        header.extend_from_slice(p.as_bytes());
        header.push(0);
    }
    header.extend_from_slice(&(body.len() as u16).to_le_bytes());
    let mut out = Vec::new();
    record(&mut out, DEFINE_FUNCTION, &header);
    out.extend_from_slice(body);
    out
}

/// A jump record (`If` or `Jump`) by `offset` bytes from its end.
fn branch(out: &mut Vec<u8>, code: u8, offset: usize, back: bool) {
    let o = offset as i16;
    record(out, code, &(if back { -o } else { o }).to_le_bytes());
}

/// Calls `_global.thedas_unempty` on the value on top of the stack.
fn unempty_top(out: &mut Vec<u8>) {
    push(out, &[Push::Int(1)]);
    get_path(out, &["_global"]);
    push(out, &[Push::Str("thedas_unempty")]);
    out.push(CALL_METHOD);
}

/// `function (v) { if (v === EMPTY) return ""; return v; }`
fn unempty_function() -> Vec<u8> {
    let mut then = Vec::new();
    push(&mut then, &[Push::Str("")]);
    then.push(RETURN);
    let mut body = Vec::new();
    get_path(&mut body, &["v"]);
    push(&mut body, &[Push::Str(EMPTY)]);
    body.extend_from_slice(&[STRICT_EQUALS, NOT]);
    branch(&mut body, IF, then.len(), false);
    body.extend(then);
    get_path(&mut body, &["v"]);
    body.push(RETURN);
    function(&["v"], &body)
}

/// `function (o) { return function () { o.apply(_global.ExternalCommands, arguments);
/// return _global.thedas_unempty(flash.external.ExternalInterface.call(RETVAL_CALL)); }; }`
fn wrap_maker() -> Vec<u8> {
    let mut inner = Vec::new();
    get_path(&mut inner, &["arguments"]);
    get_path(&mut inner, &["_global", "ExternalCommands"]);
    push(&mut inner, &[Push::Int(2)]);
    get_path(&mut inner, &["o"]);
    push(&mut inner, &[Push::Str("apply")]);
    inner.extend_from_slice(&[CALL_METHOD, POP]);
    push(&mut inner, &[Push::Str(RETVAL_CALL), Push::Int(1)]);
    get_path(&mut inner, &["flash", "external", "ExternalInterface"]);
    push(&mut inner, &[Push::Str("call")]);
    inner.push(CALL_METHOD);
    unempty_top(&mut inner);
    inner.push(RETURN);
    let mut outer = function(&[], &inner);
    outer.push(RETURN);
    function(&["o"], &outer)
}

/// `function (o) { return function () {
/// return _global.thedas_unempty(o.apply(_global.ExternalCommands, arguments)); }; }`
fn getter_wrap_maker() -> Vec<u8> {
    let mut inner = Vec::new();
    get_path(&mut inner, &["arguments"]);
    get_path(&mut inner, &["_global", "ExternalCommands"]);
    push(&mut inner, &[Push::Int(2)]);
    get_path(&mut inner, &["o"]);
    push(&mut inner, &[Push::Str("apply")]);
    inner.push(CALL_METHOD);
    unempty_top(&mut inner);
    inner.push(RETURN);
    let mut outer = function(&[], &inner);
    outer.push(RETURN);
    function(&["o"], &outer)
}

/// `function (o, t, m, c) { return function () {
/// t[m] = flash.external.ExternalInterface.call(c, arguments[0]);
/// return o.apply(this, arguments); }; }`
fn prewrap_maker() -> Vec<u8> {
    let mut inner = Vec::new();
    get_path(&mut inner, &["t"]);
    get_path(&mut inner, &["m"]);
    get_path(&mut inner, &["arguments"]);
    push(&mut inner, &[Push::Int(0)]);
    inner.push(GET_MEMBER);
    get_path(&mut inner, &["c"]);
    push(&mut inner, &[Push::Int(2)]);
    get_path(&mut inner, &["flash", "external", "ExternalInterface"]);
    push(&mut inner, &[Push::Str("call")]);
    inner.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    get_path(&mut inner, &["arguments"]);
    get_path(&mut inner, &["this"]);
    push(&mut inner, &[Push::Int(2)]);
    get_path(&mut inner, &["o"]);
    push(&mut inner, &[Push::Str("apply")]);
    inner.extend_from_slice(&[CALL_METHOD, RETURN]);
    let mut outer = function(&[], &inner);
    outer.push(RETURN);
    function(&["o", "t", "m", "c"], &outer)
}

fn record(out: &mut Vec<u8>, code: u8, body: &[u8]) {
    out.push(code);
    out.extend_from_slice(&(body.len() as u16).to_le_bytes());
    out.extend_from_slice(body);
}

fn push(out: &mut Vec<u8>, values: &[Push]) {
    let mut b = Vec::new();
    for v in values {
        match v {
            Push::Str(s) => {
                b.push(0);
                b.extend_from_slice(s.as_bytes());
                b.push(0);
            }
            Push::Int(i) => {
                b.push(7);
                b.extend_from_slice(&i.to_le_bytes());
            }
            Push::Null => b.push(2),
        }
    }
    record(out, PUSH, &b);
}

/// `function (p, m, a) { for (var i = 0; i < a.length; i++) a[i] = _global.thedas_unempty(a[i]);
/// var o = eval(p); return o[m].apply(o, a); }`
fn invoke_function() -> Vec<u8> {
    invoke_body()
}

fn invoke_body() -> Vec<u8> {
    let mut body = Vec::new();
    push(&mut body, &[Push::Str("i"), Push::Int(0)]);
    body.push(DEFINE_LOCAL);
    let mut test = Vec::new();
    get_path(&mut test, &["i"]);
    get_path(&mut test, &["a", "length"]);
    test.extend_from_slice(&[LESS2, NOT]);
    let mut step = Vec::new();
    get_path(&mut step, &["a"]);
    get_path(&mut step, &["i"]);
    get_path(&mut step, &["a"]);
    get_path(&mut step, &["i"]);
    step.push(GET_MEMBER);
    unempty_top(&mut step);
    step.push(SET_MEMBER);
    push(&mut step, &[Push::Str("i")]);
    get_path(&mut step, &["i"]);
    step.extend_from_slice(&[INCREMENT, SET_VARIABLE]);
    // Each branch record is 5 bytes.
    let back = test.len() + 5 + step.len() + 5;
    body.extend_from_slice(&test);
    branch(&mut body, IF, step.len() + 5, false);
    body.extend(step);
    branch(&mut body, JUMP, back, true);
    push(&mut body, &[Push::Str("a")]);
    body.push(GET_VARIABLE);
    push(&mut body, &[Push::Str("p")]);
    body.extend_from_slice(&[GET_VARIABLE, GET_VARIABLE]);
    push(&mut body, &[Push::Int(2), Push::Str("p")]);
    body.extend_from_slice(&[GET_VARIABLE, GET_VARIABLE]);
    push(&mut body, &[Push::Str("m")]);
    body.extend_from_slice(&[GET_VARIABLE, GET_MEMBER]);
    push(&mut body, &[Push::Str("apply")]);
    body.extend_from_slice(&[CALL_METHOD, RETURN]);
    function(&["p", "m", "a"], &body)
}

/// The body of a `DoAction` tag registering [`INVOKE_CALLBACK`]
/// (`flash.external.ExternalInterface.addCallback("thedas_invoke", null, function ...)`) and
/// wrapping [`RETURNING_FSCOMMANDS`].
pub fn register_invoke() -> Vec<u8> {
    let mut out = Vec::new();
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_unempty")]);
    out.extend(unempty_function());
    out.push(SET_MEMBER);
    // _global.thedas_wrap = wrap_maker
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_wrap")]);
    out.extend(wrap_maker());
    out.push(SET_MEMBER);
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_getter_wrap")]);
    out.extend(getter_wrap_maker());
    out.push(SET_MEMBER);
    for f in VALUE_GETTERS {
        get_path(&mut out, &["_global", "ExternalCommands"]);
        push(&mut out, &[Push::Str(f)]);
        get_path(&mut out, &["_global", "ExternalCommands", f]);
        push(&mut out, &[Push::Int(1)]);
        get_path(&mut out, &["_global"]);
        push(&mut out, &[Push::Str("thedas_getter_wrap")]);
        out.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    }
    for f in RETURNING_FSCOMMANDS {
        // _global.ExternalCommands[f] = _global.thedas_wrap(_global.ExternalCommands[f])
        get_path(&mut out, &["_global", "ExternalCommands"]);
        push(&mut out, &[Push::Str(f)]);
        get_path(&mut out, &["_global", "ExternalCommands", f]);
        push(&mut out, &[Push::Int(1)]);
        get_path(&mut out, &["_global"]);
        push(&mut out, &[Push::Str("thedas_wrap")]);
        out.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    }
    // _global.thedas_prewrap = prewrap_maker; _global.thedas_install wraps the PRE_CALLS
    // functions (the host calls it once the movie's classes exist, see `INSTALL`).
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_prewrap")]);
    out.extend(prewrap_maker());
    out.push(SET_MEMBER);
    let mut install = Vec::new();
    for p in PRE_CALLS {
        get_path(&mut install, p.object);
        push(
            &mut install,
            &[
                Push::Str(p.function),
                Push::Str(p.call),
                Push::Str(p.variable),
            ],
        );
        get_path(&mut install, p.target);
        let mut func = p.object.to_vec();
        func.push(p.function);
        get_path(&mut install, &func);
        push(&mut install, &[Push::Int(4)]);
        get_path(&mut install, &["_global"]);
        push(&mut install, &[Push::Str("thedas_prewrap")]);
        install.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    }
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_install")]);
    out.extend(function(&[], &install));
    out.push(SET_MEMBER);
    for (name, f) in [
        (INVOKE_CALLBACK, invoke_function()),
        (SET_CALLBACK, set_function()),
    ] {
        out.extend(f);
        push(
            &mut out,
            &[
                Push::Null,
                Push::Str(name),
                Push::Int(3),
                Push::Str("flash"),
            ],
        );
        out.push(GET_VARIABLE);
        push(&mut out, &[Push::Str("external")]);
        out.push(GET_MEMBER);
        push(&mut out, &[Push::Str("ExternalInterface")]);
        out.push(GET_MEMBER);
        push(&mut out, &[Push::Str("addCallback")]);
        out.extend_from_slice(&[CALL_METHOD, POP]);
    }
    out.push(0);
    out
}

/// `function (p, m, v) { eval(p)[m] = v; }`: how the engine sets movie variables
/// (Scaleform's `GFxMovie::SetVariable`).
fn set_function() -> Vec<u8> {
    let mut body = Vec::new();
    push(&mut body, &[Push::Str("p")]);
    body.extend_from_slice(&[GET_VARIABLE, GET_VARIABLE]);
    push(&mut body, &[Push::Str("m")]);
    body.push(GET_VARIABLE);
    push(&mut body, &[Push::Str("v")]);
    body.extend_from_slice(&[GET_VARIABLE, SET_MEMBER]);
    function(&["p", "m", "v"], &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_length_covers_body() {
        let f = invoke_function();
        assert_eq!(f[0], DEFINE_FUNCTION);
        let len = u16::from_le_bytes([f[1], f[2]]) as usize;
        let header = &f[3..3 + len];
        let code = u16::from_le_bytes([header[len - 2], header[len - 1]]) as usize;
        assert_eq!(3 + len + code, f.len());
        assert_eq!(*f.last().unwrap(), RETURN);
        assert_eq!(*register_invoke().last().unwrap(), 0);
    }
}
