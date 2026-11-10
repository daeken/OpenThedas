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
//! - `screenshot`: the frame as the player sees it (world, reticle and interface), a PNG in
//!   base64 under `"png"`.
//! - `state`: the simulation's save (the JSON `thedas-mcp`'s `save_state` writes) under
//!   `"save"`, as a string.
//!
//! Every reply has `"ok"`; failures carry `"error"`. Input reaches the simulation only as
//! the player's own does (orders, interface clicks), so it stays deterministic.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value as J, json};

/// How long a connection waits for the game to answer one request.
const REPLY_TIMEOUT: Duration = Duration::from_secs(600);

/// The longest request line taken (bytes); a longer one is answered with an error and the
/// connection closed, so a stray client cannot grow the game's memory without bound.
pub const MAX_REQUEST: usize = 4 << 20;

/// A request from a client, with where its reply goes.
pub struct Request {
    pub body: J,
    pub reply: mpsc::Sender<J>,
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
        let listener = TcpListener::bind(addr).with_context(|| format!("listening on {addr}"))?;
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
        let reply = if too_long {
            failure(format!("request longer than {MAX_REQUEST} bytes"))
        } else {
            let text = String::from_utf8_lossy(&line);
            if text.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<J>(&text) {
                Ok(body) => ask(&tx, body),
                Err(e) => failure(format!("not JSON: {e}")),
            }
        };
        let mut text = reply.to_string();
        text.push('\n');
        if out.write_all(text.as_bytes()).is_err() || too_long {
            return;
        }
    }
}

/// Hands a request to the frame loop and waits for its answer.
fn ask(tx: &mpsc::Sender<Request>, body: J) -> J {
    let (reply, answer) = mpsc::channel();
    if tx.send(Request { body, reply }).is_err() {
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
