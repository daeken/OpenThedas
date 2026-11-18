//! A control channel into the running game, for driving it from outside (`thedas-mcp`'s
//! `attach`): `--control 127.0.0.1:7878` listens on a loopback TCP port, and each line a
//! client sends is one JSON request, answered by one JSON line.
//!
//! Requests (`"cmd"`):
//! - `status`: where the game is (time, area, the leader and what it is doing, the party,
//!   the conversation, the screens shown, the camera).
//! - `input` with `"steps"`: input played into the game in order, in `--steps` syntax
//!   (`click X,Y; run MS; key i; ...`). The reply comes once every step has played, `run`
//!   steps taking that long in the game's own frames.
//!   `windowkey down|repeat|up KeyCode [JSON text]` uses the window's keyboard route,
//!   including App::text_key's typing gate, modifiers and game shortcuts. KeyCode
//!   is a winit physical key name (e.g. `F8`, `Quote`, `KeyA`, `Enter`). Supply the
//!   window event's character text as a JSON string, e.g. `windowkey down Quote "'"`
//!   or `windowkey down Space " "`. As with other steps, semicolons separate steps.
//!   Unlike `type` and `key enter`, this can test app-level keyboard routing.
//! - `screenshot`: the frame as the player sees it, read back from the window as it is
//!   presented (`"presented": true`; drawn again offscreen where the window's surface cannot
//!   be read, and `"presented": false` while the window has no frame to present, e.g.
//!   hidden or occluded), a PNG in base64 under `"png"`. `"scale": k` makes it `k` times
//!   smaller. The game keeps running while nothing is presented.
//! - `state`: the simulation's save (the JSON `thedas-mcp`'s `save_state` writes) under
//!   `"save"`, as a string.
//! - `motion_trace` with `"enabled": true` starts a bounded per-frame trace of the
//!   controlled creature's simulation/placement/skeletal roots, camera, target, dt and
//!   simulation Tick command count. `"enabled": false` stops it and returns `"frames"`.
//!
//! Every reply has `"ok"`; failures carry `"error"`. Input reaches the simulation only as
//! the player's own does (orders, interface clicks), so it stays deterministic.
//!
//! `--control-step-ms 20` opts a hidden, audio-off session into a fixed clock. Only
//! `run` advances that clock; loading workers consume none of a run's time. Send one
//! request at a time to observe input after its zero-time UI frame. Queued simulation
//! events wait for a positive frame. `input` accepts literal `text` or a physical `key`
//! with no printable text (optional boolean `down`, otherwise a tap), instead of `steps`.
//! `quit` exits gracefully. Normal interactive timing is unchanged.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value as J, json};

/// How long a connection waits for the game to answer one request.
const REPLY_TIMEOUT: Duration = Duration::from_secs(600);

/// How many times a busy address is tried again (200 ms apart) before giving up.
const BIND_TRIES: u32 = 25;

/// The longest request line taken (bytes); a longer one is answered with an error and the
/// connection closed, so a stray client cannot grow the game's memory without bound.
pub const MAX_REQUEST: usize = 4 << 20;

pub const MOTION_TRACE_LIMIT: usize = 4096;

pub fn record_motion(
    trace: &mut Option<Vec<J>>,
    live: &crate::live::Live,
    camera: &thedas_render::Camera,
    follow: Option<&crate::follow::Follow>,
    dt_ms: f64,
    sim_ticks: usize,
    rendered: bool,
) {
    let Some(frames) = trace.as_mut().filter(|f| f.len() < MOTION_TRACE_LIMIT) else {
        return;
    };
    frames.push(json!({
        "frame": frames.len(), "dt_ms": dt_ms, "sim_ticks": sim_ticks, "rendered": rendered,
        "hero": live.report_motion(), "camera": camera.position.to_array(),
        "target": follow.and_then(|f| f.target()).map(|p| p.to_array()),
    }));
}

/// A request from a client, with where its reply goes.
pub struct Request {
    pub body: J,
    pub reply: mpsc::Sender<J>,
    /// Only `quit` needs to know the reply reached the socket before the app exits.
    pub written: Option<mpsc::Receiver<()>>,
}

/// The listening channel: requests arrive here for the frame loop to serve.
pub struct Control {
    pub addr: SocketAddr,
    requests: mpsc::Receiver<Request>,
}

impl Control {
    /// Listens on `addr`, which must be a loopback address: the game takes orders from this
    /// machine only.
    pub fn listen(addr: &str) -> Result<Self> {
        let addr: SocketAddr = addr
            .parse()
            .with_context(|| format!("--control takes ip:port, not {addr:?}"))?;
        if !addr.ip().is_loopback() {
            bail!("--control must listen on a loopback address (127.0.0.1 or ::1), not {addr}");
        }
        // The address may still be held for a moment by the build that started this one
        // (an update hands its control address on, see `update::start_build`).
        let mut tries = 0;
        let listener = loop {
            match TcpListener::bind(addr) {
                Ok(l) => break l,
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && tries < BIND_TRIES => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(e) => return Err(e).with_context(|| format!("listening on {addr}")),
            }
        };
        let addr = listener.local_addr()?;
        let (tx, requests) = mpsc::channel();
        std::thread::Builder::new()
            .name("control".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let tx = tx.clone();
                    let _ = std::thread::Builder::new()
                        .name("control client".into())
                        .spawn(move || serve(stream, tx));
                }
            })?;
        Ok(Self { addr, requests })
    }

    /// The requests waiting to be served.
    pub fn poll(&self) -> Vec<Request> {
        self.requests.try_iter().collect()
    }
}

/// Answers one client's requests, a line at a time, until it hangs up.
fn serve(stream: TcpStream, tx: mpsc::Sender<Request>) {
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = Vec::new();
        let read = (&mut reader)
            .take(MAX_REQUEST as u64 + 1)
            .read_until(b'\n', &mut line);
        match read {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let too_long = line.len() > MAX_REQUEST;
        let mut written = None;
        let reply = if too_long {
            failure(format!("request longer than {MAX_REQUEST} bytes"))
        } else {
            let text = String::from_utf8_lossy(&line);
            if text.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<J>(&text) {
                Ok(body) => ask(&tx, body, &mut written),
                Err(e) => failure(format!("not JSON: {e}")),
            }
        };
        let mut text = reply.to_string();
        text.push('\n');
        if out.write_all(text.as_bytes()).is_err() || too_long {
            return;
        }
        if let Some(written) = written {
            let _ = written.send(());
        }
    }
}

/// Hands a request to the frame loop and waits for its answer.
fn ask(tx: &mpsc::Sender<Request>, body: J, written_tx: &mut Option<mpsc::Sender<()>>) -> J {
    let (reply, answer) = mpsc::channel();
    let written = (body["cmd"] == "quit").then(|| {
        let (tx, rx) = mpsc::channel();
        *written_tx = Some(tx);
        rx
    });
    if tx
        .send(Request {
            body,
            reply,
            written,
        })
        .is_err()
    {
        return failure("the game has stopped");
    }
    answer
        .recv_timeout(REPLY_TIMEOUT)
        .unwrap_or_else(|_| failure("the game did not answer"))
}

/// A failed reply.
pub fn failure(msg: impl std::fmt::Display) -> J {
    json!({ "ok": false, "error": msg.to_string() })
}

/// Names of physical keys accepted by the control protocol (case insensitive).
pub fn key_code(name: &str) -> Result<winit::keyboard::KeyCode> {
    use winit::keyboard::KeyCode::*;
    let name = name.to_ascii_lowercase();
    Ok(match name.as_str() {
        "a" => KeyA,
        "b" => KeyB,
        "c" => KeyC,
        "d" => KeyD,
        "e" => KeyE,
        "f" => KeyF,
        "g" => KeyG,
        "h" => KeyH,
        "i" => KeyI,
        "j" => KeyJ,
        "k" => KeyK,
        "l" => KeyL,
        "m" => KeyM,
        "n" => KeyN,
        "o" => KeyO,
        "p" => KeyP,
        "q" => KeyQ,
        "r" => KeyR,
        "s" => KeyS,
        "t" => KeyT,
        "u" => KeyU,
        "v" => KeyV,
        "w" => KeyW,
        "x" => KeyX,
        "y" => KeyY,
        "z" => KeyZ,
        "0" => Digit0,
        "1" => Digit1,
        "2" => Digit2,
        "3" => Digit3,
        "4" => Digit4,
        "5" => Digit5,
        "6" => Digit6,
        "7" => Digit7,
        "8" => Digit8,
        "9" => Digit9,
        "f1" => F1,
        "f2" => F2,
        "f3" => F3,
        "f4" => F4,
        "f5" => F5,
        "f6" => F6,
        "f7" => F7,
        "f8" => F8,
        "f9" => F9,
        "f10" => F10,
        "f11" => F11,
        "f12" => F12,
        "space" => Space,
        "escape" | "esc" => Escape,
        "enter" | "return" => Enter,
        "tab" => Tab,
        "backspace" => Backspace,
        "delete" => Delete,
        "left" => ArrowLeft,
        "right" => ArrowRight,
        "up" => ArrowUp,
        "down" => ArrowDown,
        "home" => Home,
        "end" => End,
        "pageup" => PageUp,
        "pagedown" => PageDown,
        "shift" => ShiftLeft,
        "ctrl" | "control" => ControlLeft,
        "alt" => AltLeft,
        "meta" | "cmd" => SuperLeft,
        "/" => Slash,
        "\\" => Backslash,
        _ => bail!("unknown physical key {name:?}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_reach_the_loop_and_replies_come_back_a_line_each() {
        assert!(Control::listen("0.0.0.0:0").is_err(), "loopback only");
        assert!(Control::listen("nonsense").is_err());
        let control = Control::listen("127.0.0.1:0").unwrap();
        let addr = control.addr;
        let client = std::thread::spawn(move || {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(b"{\"cmd\":\"status\"}\nnot json\n").unwrap();
            let mut lines = BufReader::new(s).lines();
            let a: J = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
            let b: J = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
            (a, b)
        });
        // The frame loop: serve what has arrived.
        let mut served = 0;
        while served == 0 {
            for r in control.poll() {
                assert_eq!(r.body["cmd"], "status");
                r.reply.send(json!({ "ok": true, "time_ms": 5 })).unwrap();
                served += 1;
            }
            std::thread::yield_now();
        }
        let (a, b) = client.join().unwrap();
        assert_eq!(a["time_ms"], 5);
        assert_eq!(b["ok"], false);
        // A request past the limit is refused and the connection closed.
        let mut s = TcpStream::connect(addr).unwrap();
        let _ = s.write_all(&vec![b'x'; MAX_REQUEST + 10]);
        let mut lines = BufReader::new(s).lines();
        let refused: J = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
        assert!(refused["error"].as_str().unwrap().contains("longer than"));
        assert!(lines.next().is_none());
    }
}
