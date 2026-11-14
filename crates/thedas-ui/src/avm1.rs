//! Engine protocol and text helpers injected as extra frame-one AVM1 actions.
//!
//! Calls by path use registered callbacks. The value-returning fscommand wrappers fetch
//! the bridge's synchronous reply, and ability queries set the movie's expected variable.
//! Text helpers retain the current tooltip, inline-image and post-load layout behavior.
//! Mouse picking, button arguments and release ordering are provided by the player.

/// The callback name the injected code registers.
pub const INVOKE_CALLBACK: &str = "thedas_invoke";

/// Lays the tooltip tooltips.gfx showed last out again: `ResizeTooltip()`, then
/// `PositionTooltip` with the rectangle it was last placed by (`SharedLibrary.FloatingTooltip`'s
/// `PositionTooltip` is wrapped at install to remember it). The movie sizes the box from its
/// text clip's height straight after setting the text wrapped. The host
/// calls this for a few frames after a tooltip's text changes.
pub const TOOLTIP_RELAYOUT: &str = "_global.thedas_tooltip_relayout";

/// The function the host calls once, after starting a movie, to wrap [`PRE_CALLS`] (the
/// movies define those classes after the first frame's actions run).
pub const INSTALL: &str = "_global.thedas_install";

/// Scaleform's `Mouse.LEFT`, `Mouse.RIGHT`, `Mouse.MIDDLE`; a double click is the button + 4.
/// Provisional: Scaleform's values, as the movies' button tests imply.
pub const MOUSE_LEFT: f64 = 1.0;
pub const MOUSE_RIGHT: f64 = 2.0;
pub const MOUSE_MIDDLE: f64 = 3.0;
pub const DOUBLE_CLICK: f64 = 4.0;

/// Runs the post-load functions put off by `GUI.Element.RegisterPostLoadFunction`'s wrapper
/// (the host calls it after each frame and each mouse event). A player can run a new element's
/// `onLoad` as `attachMovie` makes it, so an element was already loaded (`m_bLoaded`, its
/// post-load functions called) when the movie registers functions for it right after making
/// it, and they never ran: an open dropdown's list background was never grown to the list
/// (`ExpandBackground`) nor its entries fitted (`FitButtons`), leaving the list see-through
/// over the controls below. Scaleform ran `onLoad` after the movie's code; the wrapper puts
/// such a function off until then. Provisional: the order is inferred from the movies.
pub const POST_LOAD_RUN: &str = "_global.thedas_run_after";

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

/// `DAExternalCommands` functions that send an fscommand and return what the game set in
/// `_root.fsc_retval` (and `fsc_retval2`) meanwhile (tactics.gfx's pickers). They are wrapped
/// like [`RETURNING_FSCOMMANDS`] (once the movie defines them, see [`INSTALL`]): the
/// wrapper returns the game's whole answer, which for `ListTacticTargets` and
/// `ListTacticConditions` is the `{Targets, IDs}` / `{Conditions, IDs}` object the originals
/// build from the two variables.
pub const DA_RETURNING_FSCOMMANDS: &[&str] = &[
    "ListTacticTargets",
    "ListTacticBaseConditions",
    "ListTacticConditions",
    "ListTacticActions",
    "ListTacticAbilities",
    "ListTacticModalAbilities",
    "ListTacticItems",
    "GetObjectNameByID",
    "CreatureCanUseItem",
    "HasAbility",
];

/// `DAExternalCommands` functions that ask through `ExternalInterface.call` and use what it
/// returns (answered by the game's query handler, as [`DA_RETURNING_FSCOMMANDS`]).
pub const DA_EXTERNAL_QUERIES: &[&str] = &["GetGender"];

/// Functions the game answers by setting a movie variable while the function runs (Scaleform's
/// `GFxMovie::SetVariable` from inside an `ExternalInterface` handler, which our bridge cannot do while the VM is borrowed). Each is wrapped to set the variable from `ExternalInterface.call(call name, first
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
const EQUALS2: u8 = 0x49;
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
const NEW_OBJECT: u8 = 0x40;
const ADD2: u8 = 0x47;
const INIT_ARRAY: u8 = 0x42;

enum Push<'a> {
    Str(&'a str),
    Int(i32),
    Null,
    Undefined,
    Bool(bool),
    Register(u8),
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
    inner.push(CALL_METHOD);
    inner.push(RETURN);
    let mut outer = function(&[], &inner);
    outer.push(RETURN);
    function(&["o"], &outer)
}

/// `function (o) { return function (f) {
/// if (_global.GUI.Element.cm_mcLastCreatedElement.m_bLoaded) { _global.thedas_after.push(f);
/// return; } return o.apply(this, arguments); }; }`: see [`POST_LOAD_RUN`].
fn post_load_wrap_maker() -> Vec<u8> {
    let mut then = Vec::new();
    get_path(&mut then, &["f"]);
    push(&mut then, &[Push::Int(1)]);
    get_path(&mut then, &["_global", "thedas_after"]);
    push(&mut then, &[Push::Str("push")]);
    then.extend_from_slice(&[CALL_METHOD, POP]);
    push(&mut then, &[Push::Undefined]);
    then.push(RETURN);
    let mut inner = Vec::new();
    get_path(
        &mut inner,
        &[
            "_global",
            "GUI",
            "Element",
            "cm_mcLastCreatedElement",
            "m_bLoaded",
        ],
    );
    inner.push(NOT);
    branch(&mut inner, IF, then.len(), false);
    inner.extend(then);
    get_path(&mut inner, &["arguments"]);
    get_path(&mut inner, &["this"]);
    push(&mut inner, &[Push::Int(2)]);
    get_path(&mut inner, &["o"]);
    push(&mut inner, &[Push::Str("apply")]);
    inner.extend_from_slice(&[CALL_METHOD, RETURN]);
    let mut outer = function(&["f"], &inner);
    outer.push(RETURN);
    function(&["o"], &outer)
}

/// Retains the container's current selection when a preselected child finishes
/// loading after an early click. Observe the child after clearing its conflicting
/// initial state, before the container's watcher can feed back into another child.
fn selection_listener_wrap_maker() -> Vec<u8> {
    let mut inner = Asm::default();
    let c = &mut inner.code;
    get_path(c, &["property"]);
    push(c, &[Push::Str("SI_ItemSelected")]);
    c.extend_from_slice(&[EQUALS2, NOT]);
    inner.branch(IF, "original");
    let c = &mut inner.code;
    get_path(c, &["callback", "m_Function"]);
    c.push(0x44); // TypeOf
    push(c, &[Push::Str("function")]);
    c.extend_from_slice(&[EQUALS2, NOT]);
    inner.branch(IF, "original");
    let c = &mut inner.code;
    get_path(c, &["callback", "m_Function"]);
    get_path(
        c,
        &[
            "_global",
            "Behaviour",
            "SelectableItemContainer",
            "prototype",
            "SelectionChanged",
        ],
    );
    c.extend_from_slice(&[EQUALS2, NOT]);
    inner.branch(IF, "original");
    get_path(&mut inner.code, &["child", "SI_ItemSelected"]);
    inner.code.push(NOT);
    inner.branch(IF, "original");
    let c = &mut inner.code;
    push(c, &[Push::Str("current")]);
    get_path(c, &["callback", "m_TargetObject", "_currentObject"]);
    c.push(DEFINE_LOCAL);
    get_path(c, &["current"]);
    c.push(NOT);
    inner.branch(IF, "original");
    let c = &mut inner.code;
    get_path(c, &["current"]);
    get_path(c, &["child"]);
    c.push(EQUALS2);
    inner.branch(IF, "original");
    get_path(&mut inner.code, &["current", "SI_ItemSelected"]);
    inner.code.push(NOT);
    inner.branch(IF, "original");
    let c = &mut inner.code;
    push(c, &[Push::Int(0)]);
    get_path(c, &["child"]);
    push(c, &[Push::Str("Unselect")]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    inner.label("original");
    let c = &mut inner.code;
    get_path(c, &["arguments"]);
    get_path(c, &["this"]);
    push(c, &[Push::Int(2)]);
    get_path(c, &["o"]);
    push(c, &[Push::Str("apply")]);
    c.extend_from_slice(&[CALL_METHOD, RETURN]);
    let mut outer = function(&["child", "property", "callback", "data"], &inner.finish());
    outer.push(RETURN);
    function(&["o"], &outer)
}

/// `function () { var a = _global.thedas_after; while (a.length) { var f = a.shift(); f(); } }`
/// (see [`POST_LOAD_RUN`]).
fn run_after_function() -> Vec<u8> {
    let mut body = Vec::new();
    push(&mut body, &[Push::Str("a")]);
    get_path(&mut body, &["_global", "thedas_after"]);
    body.push(DEFINE_LOCAL);
    let mut test = Vec::new();
    get_path(&mut test, &["a", "length"]);
    test.push(NOT);
    let mut step = Vec::new();
    push(&mut step, &[Push::Str("f"), Push::Int(0)]);
    get_path(&mut step, &["a"]);
    push(&mut step, &[Push::Str("shift")]);
    step.extend_from_slice(&[CALL_METHOD, DEFINE_LOCAL]);
    push(&mut step, &[Push::Int(0)]);
    get_path(&mut step, &["f"]);
    push(&mut step, &[Push::Undefined]);
    step.extend_from_slice(&[CALL_METHOD, POP]);
    // Each branch record is 5 bytes.
    let back = test.len() + 5 + step.len() + 5;
    body.extend_from_slice(&test);
    branch(&mut body, IF, step.len() + 5, false);
    body.extend(step);
    branch(&mut body, JUMP, back, true);
    function(&[], &body)
}

/// Text fields showing inline images (`<img src='ico_gold' .../>` in HTML text, which
/// the engine helper places): the coins in money values, rune slots, the
/// character record's line breaks. `SharedLibrary.TextField.prototype.ProcessText`, which
/// every `SetText` passes its text through, is wrapped to note a field whose text has one
/// (its text as `thedas_html`); the host takes them ([`IMG_TAKE`]), lays the images out
/// over the field and leaves room for them in its text (see `UiMovie::layout_inline_images`).
pub const IMG_TAKE: &str = "_global.thedas_img_take";
/// Measures HTML in a field's format: (field, html, wrap) to `[width, height]`.
pub const IMG_MEASURE: &str = "_global.thedas_img_measure";
/// Sets a field's HTML text: (field, html).
pub const IMG_SET: &str = "_global.thedas_img_set";
/// A field's width and its text's alignment: (field) to `[width, align]`.
pub const IMG_FIELD: &str = "_global.thedas_img_field";
/// Places image `n` of a field: (field, n, linkage name or "" to clear, x, y) to the
/// image's `[width, height]`.
pub const IMG_PUT: &str = "_global.thedas_img_put";

/// `function (o) { return function (t) { var h = o.apply(this, arguments);
/// if (h.indexOf("<img") != -1) { this.thedas_html = h; if (_global.thedas_imgtf ==
/// undefined) _global.thedas_imgtf = new Array(); _global.thedas_imgtf.push(this); }
/// return h; }; }`: `SharedLibrary.TextField.prototype.ProcessText`'s wrapper (see
/// [`IMG_TAKE`]).
fn process_text_wrap_maker() -> Vec<u8> {
    let mut a = Asm::default();
    let c = &mut a.code;
    push(c, &[Push::Str("h")]);
    get_path(c, &["arguments"]);
    get_path(c, &["this"]);
    push(c, &[Push::Int(2)]);
    get_path(c, &["o"]);
    push(c, &[Push::Str("apply")]);
    c.extend_from_slice(&[CALL_METHOD, DEFINE_LOCAL]);
    push(c, &[Push::Str("<img"), Push::Int(1)]);
    get_path(c, &["h"]);
    push(c, &[Push::Str("indexOf")]);
    c.push(CALL_METHOD);
    push(c, &[Push::Int(-1)]);
    c.push(EQUALS2);
    a.branch(IF, "done");
    let c = &mut a.code;
    get_path(c, &["this"]);
    push(c, &[Push::Str("thedas_html")]);
    get_path(c, &["h"]);
    c.push(SET_MEMBER);
    get_path(c, &["_global", "thedas_imgtf"]);
    push(c, &[Push::Undefined]);
    c.extend_from_slice(&[EQUALS2, NOT]);
    a.branch(IF, "have");
    let c = &mut a.code;
    get_path(c, &["_global"]);
    push(
        c,
        &[Push::Str("thedas_imgtf"), Push::Int(0), Push::Str("Array")],
    );
    c.extend_from_slice(&[NEW_OBJECT, SET_MEMBER]);
    a.label("have");
    let c = &mut a.code;
    get_path(c, &["this"]);
    push(c, &[Push::Int(1)]);
    get_path(c, &["_global", "thedas_imgtf"]);
    push(c, &[Push::Str("push")]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    a.label("done");
    get_path(&mut a.code, &["h"]);
    a.code.push(RETURN);
    let mut outer = function(&["t"], &a.finish());
    outer.push(RETURN);
    function(&["o"], &outer)
}

/// `function () { var a = _global.thedas_imgtf; _global.thedas_imgtf = new Array();
/// if (_global.thedas_imgall == undefined) _global.thedas_imgall = new Array();
/// var o = new Array(); for (var i = 0; i < a.length; i++) { var c = a[i];
/// if (c.thedas_imgk == undefined) { c.thedas_imgk = _global.thedas_imgall.length;
/// _global.thedas_imgall.push(c); } o.push(c.thedas_imgk); o.push(c.thedas_html); }
/// return o; }` (see [`IMG_TAKE`]).
fn img_take_function() -> Vec<u8> {
    let mut a = Asm::default();
    let c = &mut a.code;
    push(c, &[Push::Str("a")]);
    get_path(c, &["_global", "thedas_imgtf"]);
    c.push(DEFINE_LOCAL);
    get_path(c, &["_global"]);
    push(
        c,
        &[Push::Str("thedas_imgtf"), Push::Int(0), Push::Str("Array")],
    );
    c.extend_from_slice(&[NEW_OBJECT, SET_MEMBER]);
    get_path(c, &["_global", "thedas_imgall"]);
    push(c, &[Push::Undefined]);
    c.extend_from_slice(&[EQUALS2, NOT]);
    a.branch(IF, "all");
    let c = &mut a.code;
    get_path(c, &["_global"]);
    push(
        c,
        &[Push::Str("thedas_imgall"), Push::Int(0), Push::Str("Array")],
    );
    c.extend_from_slice(&[NEW_OBJECT, SET_MEMBER]);
    a.label("all");
    let c = &mut a.code;
    push(c, &[Push::Str("o"), Push::Int(0), Push::Str("Array")]);
    c.extend_from_slice(&[NEW_OBJECT, DEFINE_LOCAL]);
    push(c, &[Push::Str("i"), Push::Int(0)]);
    c.push(DEFINE_LOCAL);
    a.label("loop");
    let c = &mut a.code;
    get_path(c, &["i"]);
    get_path(c, &["a", "length"]);
    c.extend_from_slice(&[LESS2, NOT]);
    a.branch(IF, "end");
    let c = &mut a.code;
    push(c, &[Push::Str("c")]);
    get_path(c, &["a"]);
    get_path(c, &["i"]);
    c.extend_from_slice(&[GET_MEMBER, DEFINE_LOCAL]);
    get_path(c, &["c", "thedas_imgk"]);
    push(c, &[Push::Undefined]);
    c.extend_from_slice(&[EQUALS2, NOT]);
    a.branch(IF, "known");
    let c = &mut a.code;
    get_path(c, &["c"]);
    push(c, &[Push::Str("thedas_imgk")]);
    get_path(c, &["_global", "thedas_imgall", "length"]);
    c.push(SET_MEMBER);
    get_path(c, &["c"]);
    push(c, &[Push::Int(1)]);
    get_path(c, &["_global", "thedas_imgall"]);
    push(c, &[Push::Str("push")]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    a.label("known");
    let c = &mut a.code;
    for m in ["thedas_imgk", "thedas_html"] {
        get_path(c, &["c", m]);
        push(c, &[Push::Int(1)]);
        get_path(c, &["o"]);
        push(c, &[Push::Str("push")]);
        c.extend_from_slice(&[CALL_METHOD, POP]);
    }
    push(c, &[Push::Str("i")]);
    get_path(c, &["i"]);
    c.extend_from_slice(&[INCREMENT, SET_VARIABLE]);
    a.branch(JUMP, "loop");
    a.label("end");
    get_path(&mut a.code, &["o"]);
    a.code.push(RETURN);
    function(&[], &a.finish())
}

/// `function (k, h, w) { var t = _global.thedas_imgall[k].TextField; var m =
/// _root.thedas_meas; if (m == undefined) { _root.createTextField("thedas_meas", 1048000,
/// -10000, -10000, 100, 100); m = _root.thedas_meas; m._visible = false; }
/// m.autoSize = "left"; m.embedFonts = t.embedFonts; m.multiline = t.multiline;
/// m.wordWrap = w && t.wordWrap; m._width = t._width; m.html = true;
/// m.setNewTextFormat(t.getNewTextFormat()); m.htmlText = h;
/// return [m.textWidth, m.textHeight]; }` (see [`IMG_MEASURE`]).
fn img_measure_function() -> Vec<u8> {
    let mut a = Asm::default();
    let c = &mut a.code;
    push(c, &[Push::Str("t")]);
    get_path(c, &["_global", "thedas_imgall"]);
    get_path(c, &["k"]);
    c.push(GET_MEMBER);
    push(c, &[Push::Str("TextField")]);
    c.extend_from_slice(&[GET_MEMBER, DEFINE_LOCAL]);
    push(c, &[Push::Str("m")]);
    get_path(c, &["_root", "thedas_meas"]);
    c.push(DEFINE_LOCAL);
    get_path(c, &["m"]);
    push(c, &[Push::Undefined]);
    c.extend_from_slice(&[EQUALS2, NOT]);
    a.branch(IF, "have");
    let c = &mut a.code;
    push(
        c,
        &[
            Push::Int(100),
            Push::Int(100),
            Push::Int(-10000),
            Push::Int(-10000),
            Push::Int(1048000),
            Push::Str("thedas_meas"),
            Push::Int(6),
        ],
    );
    get_path(c, &["_root"]);
    push(c, &[Push::Str("createTextField")]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    push(c, &[Push::Str("m")]);
    get_path(c, &["_root", "thedas_meas"]);
    c.push(SET_VARIABLE);
    get_path(c, &["m"]);
    push(c, &[Push::Str("_visible"), Push::Bool(false)]);
    c.push(SET_MEMBER);
    a.label("have");
    let c = &mut a.code;
    get_path(c, &["m"]);
    push(c, &[Push::Str("autoSize"), Push::Str("left")]);
    c.push(SET_MEMBER);
    for m in ["embedFonts", "multiline", "_width"] {
        get_path(c, &["m"]);
        push(c, &[Push::Str(m)]);
        get_path(c, &["t", m]);
        c.push(SET_MEMBER);
    }
    get_path(c, &["m"]);
    push(c, &[Push::Str("wordWrap")]);
    get_path(c, &["w"]);
    get_path(c, &["t", "wordWrap"]);
    c.push(0x10); // And
    c.push(SET_MEMBER);
    get_path(c, &["m"]);
    push(c, &[Push::Str("html"), Push::Bool(true)]);
    c.push(SET_MEMBER);
    push(c, &[Push::Int(0)]);
    get_path(c, &["t"]);
    push(c, &[Push::Str("getNewTextFormat")]);
    c.push(CALL_METHOD);
    push(c, &[Push::Int(1)]);
    get_path(c, &["m"]);
    push(c, &[Push::Str("setNewTextFormat")]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    get_path(c, &["m"]);
    push(c, &[Push::Str("htmlText")]);
    get_path(c, &["h"]);
    c.push(SET_MEMBER);
    get_path(c, &["m", "textHeight"]);
    get_path(c, &["m", "textWidth"]);
    push(c, &[Push::Int(2)]);
    c.extend_from_slice(&[INIT_ARRAY, RETURN]);
    function(&["k", "h", "w"], &a.finish())
}

/// `function (k, h) { _global.thedas_imgall[k].TextField.htmlText = h; }`
fn img_set_function() -> Vec<u8> {
    let mut c = Vec::new();
    get_path(&mut c, &["_global", "thedas_imgall"]);
    get_path(&mut c, &["k"]);
    c.push(GET_MEMBER);
    push(&mut c, &[Push::Str("TextField")]);
    c.push(GET_MEMBER);
    push(&mut c, &[Push::Str("htmlText")]);
    get_path(&mut c, &["h"]);
    c.push(SET_MEMBER);
    function(&["k", "h"], &c)
}

/// `function (k, n, s, x, y) { var c = _global.thedas_imgall[k]; var t = c.TextField;
/// var m = c.createEmptyMovieClip("thedas_img" + n, 10000 + n); if (s != "")
/// m.attachBitmap(flash.display.BitmapData.loadBitmap(s), 1); m._x = t._x + x;
/// m._y = t._y + y; return [m._width, m._height]; }` (see [`IMG_PUT`]).
fn img_put_function() -> Vec<u8> {
    let mut a = Asm::default();
    let c = &mut a.code;
    push(c, &[Push::Str("c")]);
    get_path(c, &["_global", "thedas_imgall"]);
    get_path(c, &["k"]);
    c.extend_from_slice(&[GET_MEMBER, DEFINE_LOCAL]);
    push(c, &[Push::Str("t")]);
    get_path(c, &["c", "TextField"]);
    c.push(DEFINE_LOCAL);
    push(c, &[Push::Str("m"), Push::Int(10000)]);
    get_path(c, &["n"]);
    c.push(ADD2);
    push(c, &[Push::Str("thedas_img")]);
    get_path(c, &["n"]);
    c.push(ADD2);
    push(c, &[Push::Int(2)]);
    get_path(c, &["c"]);
    push(c, &[Push::Str("createEmptyMovieClip")]);
    c.extend_from_slice(&[CALL_METHOD, DEFINE_LOCAL]);
    get_path(c, &["s"]);
    push(c, &[Push::Str("")]);
    c.push(EQUALS2);
    a.branch(IF, "place");
    let c = &mut a.code;
    push(c, &[Push::Int(1)]);
    get_path(c, &["s"]);
    push(c, &[Push::Int(1)]);
    get_path(c, &["flash", "display", "BitmapData"]);
    push(c, &[Push::Str("loadBitmap")]);
    c.push(CALL_METHOD);
    push(c, &[Push::Int(2)]);
    get_path(c, &["m"]);
    push(c, &[Push::Str("attachBitmap")]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    a.label("place");
    let c = &mut a.code;
    for (m, v) in [("_x", "x"), ("_y", "y")] {
        get_path(c, &["m"]);
        push(c, &[Push::Str(m)]);
        get_path(c, &["t", m]);
        get_path(c, &[v]);
        c.push(ADD2);
        c.push(SET_MEMBER);
    }
    get_path(c, &["m", "_height"]);
    get_path(c, &["m", "_width"]);
    push(c, &[Push::Int(2)]);
    c.extend_from_slice(&[INIT_ARRAY, RETURN]);
    function(&["k", "n", "s", "x", "y"], &a.finish())
}

/// `function (k) { var t = _global.thedas_imgall[k].TextField;
/// return [t._width, t.getNewTextFormat().align]; }` (see [`IMG_FIELD`]).
fn img_field_function() -> Vec<u8> {
    let mut c = Vec::new();
    push(&mut c, &[Push::Str("t")]);
    get_path(&mut c, &["_global", "thedas_imgall"]);
    get_path(&mut c, &["k"]);
    c.push(GET_MEMBER);
    push(&mut c, &[Push::Str("TextField")]);
    c.extend_from_slice(&[GET_MEMBER, DEFINE_LOCAL]);
    push(&mut c, &[Push::Int(0)]);
    get_path(&mut c, &["t"]);
    push(&mut c, &[Push::Str("getNewTextFormat")]);
    c.push(CALL_METHOD);
    push(&mut c, &[Push::Str("align")]);
    c.push(GET_MEMBER);
    get_path(&mut c, &["t", "_width"]);
    push(&mut c, &[Push::Int(2)]);
    c.extend_from_slice(&[INIT_ARRAY, RETURN]);
    function(&["k"], &c)
}

/// Code with forward and backward branches by label.
#[derive(Default)]
struct Asm {
    code: Vec<u8>,
    labels: std::collections::HashMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}

impl Asm {
    fn label(&mut self, l: &'static str) {
        self.labels.insert(l, self.code.len());
    }

    /// An `If` or `Jump` to `l`.
    fn branch(&mut self, op: u8, l: &'static str) {
        record(&mut self.code, op, &[0, 0]);
        self.fixups.push((self.code.len(), l));
    }

    fn finish(mut self) -> Vec<u8> {
        for (end, l) in std::mem::take(&mut self.fixups) {
            let target = self.labels.get(l).copied().unwrap_or(end);
            let off = (target as i64 - end as i64) as i16;
            self.code[end - 2..end].copy_from_slice(&off.to_le_bytes());
        }
        self.code
    }
}

const ENUMERATE2: u8 = 0x55;
const TYPEOF: u8 = 0x44;
const STORE_REGISTER: u8 = 0x87;

/// The host's list of what a pad's focus can land on (see [`FOCUS_LIST`]).
pub const FOCUS_LIST: &str = "_global.thedas_focus_list";

/// `function (c, o)`: pushes onto `o` each clip below `c` that takes a click (`onRelease` or
/// `onPress`), visible all the way up and not disabled (`enabled === false`), as
/// `"path|xMin|yMin|xMax|yMax"` with its bounds in `_root`'s space; a clip that takes the
/// click is not searched further (its children's handlers never see the mouse).
/// Provisional: Scaleform's own focus search is not traced (this is the host's stand-in for
/// the console focus the PC movies leave half-built).
fn focus_function() -> Vec<u8> {
    walk_function("thedas_focus", false)
}

/// `function (c, o)`: as [`focus_function`], but every visible clip below `c` is listed
/// (with its `_alpha` after its bounds) and searched: a debugging view of a movie's
/// display tree (`_global.thedas_tree_list`, for the headless `call` step).
fn tree_function() -> Vec<u8> {
    walk_function("thedas_tree", true)
}

/// The walk behind [`focus_function`] and [`tree_function`]: `name` is the function's own
/// `_global` name (it recurses through it); `all` lists every visible clip, not only those
/// taking a click.
fn walk_function(name: &str, all: bool) -> Vec<u8> {
    let mut a = Asm::default();
    get_path(&mut a.code, &["c"]);
    a.code.push(ENUMERATE2);
    a.label("next");
    let c = &mut a.code;
    record(c, STORE_REGISTER, &[0]);
    c.push(POP);
    push(c, &[Push::Register(0), Push::Null]);
    c.push(EQUALS2);
    a.branch(IF, "done");
    let c = &mut a.code;
    push(c, &[Push::Str("m")]);
    get_path(c, &["c"]);
    push(c, &[Push::Register(0)]);
    c.extend_from_slice(&[GET_MEMBER, DEFINE_LOCAL]);
    get_path(c, &["m"]);
    c.push(TYPEOF);
    push(c, &[Push::Str("movieclip")]);
    c.extend_from_slice(&[STRICT_EQUALS, NOT]);
    a.branch(IF, "next");
    let c = &mut a.code;
    get_path(c, &["m", "_parent"]);
    get_path(c, &["c"]);
    c.extend_from_slice(&[EQUALS2, NOT]);
    a.branch(IF, "next");
    get_path(&mut a.code, &["m", "_visible"]);
    a.code.push(NOT);
    a.branch(IF, "next");
    if !all {
        let c = &mut a.code;
        get_path(c, &["m", "enabled"]);
        push(c, &[Push::Bool(false)]);
        c.push(STRICT_EQUALS);
        a.branch(IF, "next");
        // A drop target (an empty equipment slot: it takes a dragged item and has no press
        // of its own) counts too, so the pad's focus reaches it.
        for h in ["onRelease", "onPress", "AcceptDrop"] {
            get_path(&mut a.code, &["m", h]);
            a.branch(IF, "button");
        }
        a.branch(JUMP, "recurse");
    }
    a.label("button");
    // var b = m.getBounds(_root); o.push(m + "|" + b.xMin + "|" + ... + b.yMax);
    let c = &mut a.code;
    push(c, &[Push::Str("b")]);
    get_path(c, &["_root"]);
    push(c, &[Push::Int(1)]);
    get_path(c, &["m"]);
    push(c, &[Push::Str("getBounds")]);
    c.extend_from_slice(&[CALL_METHOD, DEFINE_LOCAL]);
    push(c, &[Push::Str("")]);
    get_path(c, &["m"]);
    c.push(ADD2);
    for f in ["xMin", "yMin", "xMax", "yMax"] {
        push(c, &[Push::Str("|")]);
        c.push(ADD2);
        get_path(c, &["b", f]);
        c.push(ADD2);
    }
    if all {
        push(c, &[Push::Str("|")]);
        c.push(ADD2);
        get_path(c, &["m", "_alpha"]);
        c.push(ADD2);
    }
    push(c, &[Push::Int(1)]);
    get_path(c, &["o"]);
    push(c, &[Push::Str("push")]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    if !all {
        // A clip that takes the click is not searched further.
        a.branch(JUMP, "next");
    }
    a.label("recurse");
    // _global[name](m, o);
    let c = &mut a.code;
    get_path(c, &["o"]);
    get_path(c, &["m"]);
    push(c, &[Push::Int(2)]);
    get_path(c, &["_global"]);
    push(c, &[Push::Str(name)]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    a.branch(JUMP, "next");
    a.label("done");
    push(&mut a.code, &[Push::Undefined]);
    a.code.push(RETURN);
    function(&["c", "o"], &a.finish())
}

/// `function () { var o = new Array(); _global.thedas_focus(_root, o); return o.join(";"); }`
fn focus_list_function() -> Vec<u8> {
    list_function("thedas_focus")
}

/// The list function of a walk named `walk` (see [`walk_function`]).
/// `function (p, msg) { eval(p).SetButtonAction(function () {
/// ExternalCommands.SendMessage(msg); }); }`: gives the menu button at path `p` (one the
/// interface added) the action of sending `msg` to the game, as the movies' own buttons
/// send theirs.
fn button_message_function() -> Vec<u8> {
    let mut inner = Vec::new();
    push(&mut inner, &[Push::Str("msg")]);
    inner.push(GET_VARIABLE);
    push(&mut inner, &[Push::Int(1)]);
    get_path(&mut inner, &["ExternalCommands"]);
    push(&mut inner, &[Push::Str("SendMessage")]);
    inner.extend_from_slice(&[CALL_METHOD, POP]);
    let mut c = function(&[], &inner);
    push(&mut c, &[Push::Int(1), Push::Str("p")]);
    // The path's value, then the clip it names.
    c.extend_from_slice(&[GET_VARIABLE, GET_VARIABLE]);
    push(&mut c, &[Push::Str("SetButtonAction")]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    function(&["p", "msg"], &c)
}

fn list_function(walk: &str) -> Vec<u8> {
    let mut c = Vec::new();
    push(&mut c, &[Push::Str("o"), Push::Int(0), Push::Str("Array")]);
    c.extend_from_slice(&[NEW_OBJECT, DEFINE_LOCAL]);
    get_path(&mut c, &["o"]);
    get_path(&mut c, &["_root"]);
    push(&mut c, &[Push::Int(2)]);
    get_path(&mut c, &["_global"]);
    push(&mut c, &[Push::Str(walk)]);
    c.extend_from_slice(&[CALL_METHOD, POP]);
    push(&mut c, &[Push::Str(";"), Push::Int(1)]);
    get_path(&mut c, &["o"]);
    push(&mut c, &[Push::Str("join")]);
    c.extend_from_slice(&[CALL_METHOD, RETURN]);
    function(&[], &c)
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
            Push::Undefined => b.push(3),
            Push::Bool(v) => {
                b.push(5);
                b.push(*v as u8);
            }
            Push::Register(r) => {
                b.push(4);
                b.push(*r);
            }
        }
    }
    record(out, PUSH, &b);
}

/// `function () { var t = _global.thedas_last_tip; t.ResizeTooltip();
/// t.PositionTooltip(t.thedas_rect); }` (see [`TOOLTIP_RELAYOUT`]).
fn tooltip_relayout_function() -> Vec<u8> {
    let mut body = Vec::new();
    push(&mut body, &[Push::Int(0)]);
    get_path(&mut body, &["_global", "thedas_last_tip"]);
    push(&mut body, &[Push::Str("ResizeTooltip")]);
    body.extend_from_slice(&[CALL_METHOD, POP]);
    get_path(&mut body, &["_global", "thedas_last_tip", "thedas_rect"]);
    push(&mut body, &[Push::Int(1)]);
    get_path(&mut body, &["_global", "thedas_last_tip"]);
    push(&mut body, &[Push::Str("PositionTooltip")]);
    body.extend_from_slice(&[CALL_METHOD, POP]);
    function(&[], &body)
}

/// `function (o) { return function (r) { this.thedas_rect = r; _global.thedas_last_tip =
/// this; return o.apply(this, arguments); }; }`: `SharedLibrary.FloatingTooltip`'s
/// `PositionTooltip` remembering where it put the tooltip, for [`TOOLTIP_RELAYOUT`].
fn tooltip_position_wrap_maker() -> Vec<u8> {
    let mut inner = Vec::new();
    get_path(&mut inner, &["this"]);
    push(&mut inner, &[Push::Str("thedas_rect")]);
    get_path(&mut inner, &["r"]);
    inner.push(SET_MEMBER);
    get_path(&mut inner, &["_global"]);
    push(&mut inner, &[Push::Str("thedas_last_tip")]);
    get_path(&mut inner, &["this"]);
    inner.push(SET_MEMBER);
    get_path(&mut inner, &["arguments"]);
    get_path(&mut inner, &["this"]);
    push(&mut inner, &[Push::Int(2)]);
    get_path(&mut inner, &["o"]);
    push(&mut inner, &[Push::Str("apply")]);
    inner.extend_from_slice(&[CALL_METHOD, RETURN]);
    let mut outer = function(&["r"], &inner);
    outer.push(RETURN);
    function(&["o"], &outer)
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

/// `if (!_global.ExternalCommands[f].thedas_w) { _global.ExternalCommands[f] =
/// _global.thedas_wrap(_global.ExternalCommands[f]); _global.ExternalCommands[f].thedas_w =
/// true; }`: wraps a [`RETURNING_FSCOMMANDS`] function once.
fn wrap_returning(out: &mut Vec<u8>, f: &str) {
    let mut then = Vec::new();
    get_path(&mut then, &["_global", "ExternalCommands"]);
    push(&mut then, &[Push::Str(f)]);
    get_path(&mut then, &["_global", "ExternalCommands", f]);
    push(&mut then, &[Push::Int(1)]);
    get_path(&mut then, &["_global"]);
    push(&mut then, &[Push::Str("thedas_wrap")]);
    then.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    get_path(&mut then, &["_global", "ExternalCommands", f]);
    push(&mut then, &[Push::Str("thedas_w"), Push::Bool(true)]);
    then.push(SET_MEMBER);
    get_path(out, &["_global", "ExternalCommands", f, "thedas_w"]);
    branch(out, IF, then.len(), false);
    out.extend(then);
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
        wrap_returning(&mut out, f);
    }
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_tip_wrap")]);
    out.extend(tooltip_position_wrap_maker());
    out.push(SET_MEMBER);
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_tooltip_relayout")]);
    out.extend(tooltip_relayout_function());
    out.push(SET_MEMBER);
    // _global.thedas_prewrap = prewrap_maker; _global.thedas_install wraps the PRE_CALLS
    // functions (the host calls it once the movie's classes exist, see `INSTALL`).
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_prewrap")]);
    out.extend(prewrap_maker());
    out.push(SET_MEMBER);
    let mut install = Vec::new();
    // A movie whose own classes load after this tag (saveload.gfx) defines
    // `ExternalCommands` afresh, unwrapped: its answers then came back undefined (the save
    // name prompt read "undefined"). The install wraps whatever is not wrapped yet.
    for f in RETURNING_FSCOMMANDS {
        wrap_returning(&mut install, f);
    }
    for f in DA_RETURNING_FSCOMMANDS {
        // _global.DAExternalCommands[f] = _global.thedas_wrap(_global.DAExternalCommands[f])
        get_path(&mut install, &["_global", "DAExternalCommands"]);
        push(&mut install, &[Push::Str(f)]);
        get_path(&mut install, &["_global", "DAExternalCommands", f]);
        push(&mut install, &[Push::Int(1)]);
        get_path(&mut install, &["_global"]);
        push(&mut install, &[Push::Str("thedas_wrap")]);
        install.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    }
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
    // SharedLibrary.FloatingTooltip.prototype.PositionTooltip remembers its rectangle (see
    // TOOLTIP_RELAYOUT); a no-op in movies without tooltips.
    let tip = ["_global", "SharedLibrary", "FloatingTooltip", "prototype"];
    get_path(&mut install, &tip);
    push(&mut install, &[Push::Str("PositionTooltip")]);
    let mut position = tip.to_vec();
    position.push("PositionTooltip");
    get_path(&mut install, &position);
    push(&mut install, &[Push::Int(1)]);
    get_path(&mut install, &["_global"]);
    push(&mut install, &[Push::Str("thedas_tip_wrap")]);
    install.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    // _global.thedas_after = new Array(); GUI.Element.RegisterPostLoadFunction wrapped (see
    // POST_LOAD_RUN).
    get_path(&mut out, &["_global"]);
    push(
        &mut out,
        &[Push::Str("thedas_after"), Push::Int(0), Push::Str("Array")],
    );
    out.extend_from_slice(&[NEW_OBJECT, SET_MEMBER]);
    // The pad's focus targets (see `focus_function`).
    for (name, f) in [
        ("thedas_focus", focus_function()),
        ("thedas_focus_list", focus_list_function()),
        ("thedas_tree", tree_function()),
        ("thedas_tree_list", list_function("thedas_tree")),
        ("thedas_button_message", button_message_function()),
    ] {
        get_path(&mut out, &["_global"]);
        push(&mut out, &[Push::Str(name)]);
        out.extend(f);
        out.push(SET_MEMBER);
    }
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_run_after")]);
    out.extend(run_after_function());
    out.push(SET_MEMBER);
    // Inline images in HTML text (see IMG_TAKE).
    for (name, f) in [
        ("thedas_text_wrap", process_text_wrap_maker()),
        ("thedas_img_take", img_take_function()),
        ("thedas_img_measure", img_measure_function()),
        ("thedas_img_set", img_set_function()),
        ("thedas_img_put", img_put_function()),
        ("thedas_img_field", img_field_function()),
    ] {
        get_path(&mut out, &["_global"]);
        push(&mut out, &[Push::Str(name)]);
        out.extend(f);
        out.push(SET_MEMBER);
    }
    let text = ["_global", "SharedLibrary", "TextField", "prototype"];
    get_path(&mut install, &text);
    push(&mut install, &[Push::Str("ProcessText")]);
    get_path(
        &mut install,
        &[
            "_global",
            "SharedLibrary",
            "TextField",
            "prototype",
            "ProcessText",
        ],
    );
    push(&mut install, &[Push::Int(1)]);
    get_path(&mut install, &["_global"]);
    push(&mut install, &[Push::Str("thedas_text_wrap")]);
    install.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    // The listener registrar is shared by existing containers and children loaded later.
    let registrar = ["_global", "DataType", "Observable", "RegisterListener"];
    let mut selection = Vec::new();
    get_path(&mut selection, &["_global", "DataType", "Observable"]);
    push(&mut selection, &[Push::Str("RegisterListener")]);
    get_path(&mut selection, &registrar);
    push(&mut selection, &[Push::Int(1)]);
    selection.extend(selection_listener_wrap_maker());
    push(&mut selection, &[Push::Undefined]);
    selection.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
    get_path(&mut selection, &registrar);
    push(&mut selection, &[Push::Str("thedas_w"), Push::Bool(true)]);
    selection.push(SET_MEMBER);
    let mut selection_install = Vec::new();
    get_path(
        &mut selection_install,
        &[
            "_global",
            "DataType",
            "Observable",
            "RegisterListener",
            "thedas_w",
        ],
    );
    branch(&mut selection_install, IF, selection.len(), false);
    selection_install.extend(selection);
    get_path(&mut install, &registrar);
    install.push(NOT);
    branch(&mut install, IF, selection_install.len(), false);
    install.extend(selection_install);
    get_path(&mut out, &["_global"]);
    push(&mut out, &[Push::Str("thedas_post_load_wrap")]);
    out.extend(post_load_wrap_maker());
    out.push(SET_MEMBER);
    let element = ["_global", "GUI", "Element"];
    get_path(&mut install, &element);
    push(&mut install, &[Push::Str("RegisterPostLoadFunction")]);
    get_path(
        &mut install,
        &["_global", "GUI", "Element", "RegisterPostLoadFunction"],
    );
    push(&mut install, &[Push::Int(1)]);
    get_path(&mut install, &["_global"]);
    push(&mut install, &[Push::Str("thedas_post_load_wrap")]);
    install.extend_from_slice(&[CALL_METHOD, SET_MEMBER]);
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

    struct Empty;
    impl thedas_gfx::Host for Empty {
        fn fscommand(&mut self, _: &str, _: &str) {}
        fn external_call(&mut self, _: &str, _: &[thedas_gfx::HostValue]) -> thedas_gfx::HostValue {
            thedas_gfx::HostValue::Undefined
        }
    }
    impl thedas_gfx::text::Fonts for Empty {
        fn font(&self, _: &str, _: bool, _: bool) -> Option<std::sync::Arc<thedas_gfx::swf::Font>> {
            None
        }
    }

    #[test]
    fn native_mouse_arguments_and_empty_callbacks_need_no_compatibility_globals() {
        use thedas_gfx::HostValue;
        use thedas_gfx::avm1::value::Value;

        let movie = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
        let mut player = thedas_gfx::Player::new(
            movie,
            "bridge-test",
            Box::new(Empty),
            std::sync::Arc::new(Empty),
            Some(register_invoke()),
        )
        .unwrap();
        player.run_frame();
        let root = player.vm.display_object(player.vm.root);
        let echo = player
            .vm
            .new_native(|_, _, args| args.first().cloned().unwrap_or_default());
        player.vm.set(&root, "echo", Value::Object(echo));
        assert_eq!(
            player.call_callback(
                INVOKE_CALLBACK,
                &[
                    HostValue::String("_root".into()),
                    HostValue::String("echo".into()),
                    HostValue::List(vec![HostValue::String(String::new())]),
                ]
            ),
            HostValue::String(String::new()),
        );
        let button = player.vm.call_method(
            &root,
            "createEmptyMovieClip",
            &[Value::from("button"), Value::Number(1.0)],
        );
        player
            .vm
            .call_method(&button, "beginFill", &[Value::Number(0.0)]);
        for (method, x, y) in [
            ("moveTo", 0.0, 0.0),
            ("lineTo", 100.0, 0.0),
            ("lineTo", 100.0, 100.0),
            ("lineTo", 0.0, 100.0),
            ("lineTo", 0.0, 0.0),
        ] {
            player
                .vm
                .call_method(&button, method, &[Value::Number(x), Value::Number(y)]);
        }
        player.vm.call_method(&button, "endFill", &[]);
        let press = player.vm.new_native(|vm, _, args| {
            vm.log.push(format!("press {:?}", args.first()));
            Value::Undefined
        });
        let release = player.vm.new_native(|vm, _, args| {
            vm.log.push(format!("release {:?}", args.first()));
            Value::Undefined
        });
        player.vm.set(&button, "onPress", Value::Object(press));
        player.vm.set(&button, "onRelease", Value::Object(release));
        let listener = Value::Object(player.vm.new_object());
        let down = player.vm.new_native(|vm, _, args| {
            assert_eq!(args.len(), 2);
            vm.log.push(format!("down {:?}", args.first()));
            Value::Undefined
        });
        player.vm.set(&listener, "onMouseDown", Value::Object(down));
        let mouse = Value::Object(player.vm.mouse);
        player.vm.call_method(&mouse, "addListener", &[listener]);
        player.drain_log();
        for button in 1..=3 {
            for double in [false, true] {
                player.mouse_button(20.0, 20.0, button, true, double);
                player.mouse_button(20.0, 20.0, button, false, double);
                let code = f64::from(button + if double { 4 } else { 0 });
                assert_eq!(
                    player.drain_log(),
                    [
                        format!("down Some(Number({code:?}))"),
                        format!("press Some(Number({code:?}))"),
                        format!("release Some(Number({code:?}))"),
                    ]
                );
            }
        }
    }

    #[test]
    fn late_selected_children_preserve_the_containers_existing_selection() {
        use thedas_gfx::avm1::value::Value;

        let movie = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
        let mut player = thedas_gfx::Player::new(
            movie,
            "selection-registration-test",
            Box::new(Empty),
            std::sync::Arc::new(Empty),
            Some(register_invoke()),
        )
        .unwrap();
        player.run_frame();
        let data_type = Value::Object(player.vm.new_object());
        let observable = Value::Object(player.vm.new_object());
        player
            .vm
            .define(player.vm.global, "DataType", data_type.clone(), 0);
        player.vm.set(&data_type, "Observable", observable.clone());
        let original = player.vm.new_native(|vm, _, args| {
            let selected = vm.get(&args[0], "SI_ItemSelected");
            vm.set(&args[0], "selectedWhenRegistered", selected);
            Value::Bool(true)
        });
        player
            .vm
            .set(&observable, "RegisterListener", Value::Object(original));
        let behaviour = Value::Object(player.vm.new_object());
        let class = Value::Object(player.vm.new_object());
        let prototype = Value::Object(player.vm.new_object());
        let selection_changed = player.vm.new_native(|_, _, _| Value::Undefined);
        player
            .vm
            .define(player.vm.global, "Behaviour", behaviour.clone(), 0);
        player
            .vm
            .set(&behaviour, "SelectableItemContainer", class.clone());
        player.vm.set(&class, "prototype", prototype.clone());
        player.vm.set(
            &prototype,
            "SelectionChanged",
            Value::Object(selection_changed),
        );
        let current = Value::Object(player.vm.new_object());
        player
            .vm
            .set(&current, "SI_ItemSelected", Value::Bool(true));
        let container = Value::Object(player.vm.new_object());
        let child = Value::Object(player.vm.new_object());
        let unselect = player.vm.new_native(|vm, this, _| {
            vm.set(&this, "SI_ItemSelected", Value::Bool(false));
            Value::Undefined
        });
        player.vm.set(&child, "Unselect", Value::Object(unselect));
        let callback = Value::Object(player.vm.new_object());
        player
            .vm
            .set(&callback, "m_TargetObject", container.clone());
        player.invoke(INSTALL, &[]);
        let wrapped = player.vm.get(&observable, "RegisterListener");
        player.invoke(INSTALL, &[]);
        assert_eq!(player.vm.get(&observable, "RegisterListener"), wrapped);
        for (selected, function, property, expected) in [
            (current.clone(), selection_changed, "SI_ItemSelected", false),
            (current.clone(), original, "SI_ItemSelected", true),
            (Value::Null, selection_changed, "SI_ItemSelected", true),
            (child.clone(), selection_changed, "SI_ItemSelected", true),
            (current.clone(), selection_changed, "other", true),
        ] {
            player.vm.set(&container, "_currentObject", selected);
            player
                .vm
                .set(&callback, "m_Function", Value::Object(function));
            player.vm.set(&child, "SI_ItemSelected", Value::Bool(true));
            let result = player.vm.call_method(
                &observable,
                "RegisterListener",
                &[
                    child.clone(),
                    Value::from(property),
                    callback.clone(),
                    child.clone(),
                ],
            );
            assert_eq!(result, Value::Bool(true));
            assert_eq!(
                player.vm.get(&current, "SI_ItemSelected"),
                Value::Bool(true)
            );
            assert_eq!(
                player.vm.get(&child, "SI_ItemSelected"),
                Value::Bool(expected)
            );
            assert_eq!(
                player.vm.get(&child, "selectedWhenRegistered"),
                Value::Bool(expected)
            );
        }
        player
            .vm
            .set(&prototype, "SelectionChanged", Value::Undefined);
        player.vm.set(&callback, "m_Function", Value::Undefined);
        player.vm.set(&container, "_currentObject", current);
        player.vm.set(&child, "SI_ItemSelected", Value::Bool(true));
        player.vm.call_method(
            &observable,
            "RegisterListener",
            &[
                child.clone(),
                Value::from("SI_ItemSelected"),
                callback,
                child.clone(),
            ],
        );
        assert_eq!(
            player.vm.get(&child, "selectedWhenRegistered"),
            Value::Bool(true)
        );
    }

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
