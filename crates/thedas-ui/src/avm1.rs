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

/// The callback name the injected code registers.
pub const INVOKE_CALLBACK: &str = "thedas_invoke";

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

const SET_MEMBER: u8 = 0x4f;

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

/// `function (o) { return function () { o.apply(_global.ExternalCommands, arguments);
/// return flash.external.ExternalInterface.call(RETVAL_CALL); }; }`
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
    inner.extend_from_slice(&[CALL_METHOD, RETURN]);
    let mut outer = function(&[], &inner);
    outer.push(RETURN);
    function(&["o"], &outer)
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

/// `function (p, m, a) { var o = eval(p); return o[m].apply(o, a); }`
fn invoke_function() -> Vec<u8> {
    invoke_body()
}

fn invoke_body() -> Vec<u8> {
    let mut body = Vec::new();
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
    // _global.thedas_wrap = wrap_maker
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_wrap")]);
    out.extend(wrap_maker());
    out.push(SET_MEMBER);
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
    out.extend(invoke_function());
    push(
        &mut out,
        &[
            Push::Null,
            Push::Str(INVOKE_CALLBACK),
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
    out.extend_from_slice(&[CALL_METHOD, POP, 0]);
    out
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
