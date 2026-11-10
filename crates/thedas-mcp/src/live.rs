//! The running game: a link to a `thedas-app` started with `--control ADDR` (see its
//! `control` module). Each request is one JSON line out and one JSON line back.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::Value as J;

/// How long a request may take (an `input` waits for its `run` steps to play).
const TIMEOUT: Duration = Duration::from_secs(610);

/// The address `thedas-app --control` is usually given.
pub const DEFAULT_ADDR: &str = "127.0.0.1:7878";

pub struct Link {
    pub addr: String,
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Link {
    pub fn connect(addr: &str) -> Result<Self, String> {
        let stream = TcpStream::connect(addr).map_err(|e| {
            format!("connecting to {addr}: {e} (start thedas-app with --control {addr})")
        })?;
        stream
            .set_read_timeout(Some(TIMEOUT))
            .map_err(|e| e.to_string())?;
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        Ok(Self {
            addr: addr.to_string(),
            reader: BufReader::new(stream),
            writer,
        })
    }

    /// Sends one request and returns the game's reply; a reply that is not `ok` is an error.
    pub fn ask(&mut self, body: J) -> Result<J, String> {
        let mut line = body.to_string();
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .map_err(|e| format!("the game hung up: {e}"))?;
        let mut answer = String::new();
        match self.reader.read_line(&mut answer) {
            Ok(0) => return Err("the game hung up".into()),
            Ok(_) => {}
            Err(e) => return Err(format!("no answer from the game: {e}")),
        }
        let reply: J = serde_json::from_str(&answer).map_err(|e| format!("bad reply: {e}"))?;
        if reply["ok"] == J::Bool(true) {
            Ok(reply)
        } else {
            Err(reply["error"]
                .as_str()
                .unwrap_or("the game refused")
                .to_string())
        }
    }
}

/// A PNG made at most `max_width` wide by averaging blocks of pixels; the factor it was
/// shrunk by (1 when it was small enough).
pub fn shrink_png(png: &[u8], max_width: u32) -> Result<(Vec<u8>, u32, u32, u32), String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err("not an RGBA8 PNG".into());
    }
    let (w, h) = (info.width, info.height);
    let k = w.div_ceil(max_width.max(1)).max(1);
    if k == 1 {
        return Ok((png.to_vec(), w, h, 1));
    }
    let (nw, nh) = (w / k, h / k);
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let mut sum = [0u32; 4];
            for dy in 0..k {
                for dx in 0..k {
                    let i = (((y * k + dy) * w + x * k + dx) * 4) as usize;
                    for c in 0..4 {
                        sum[c] += buf[i + c] as u32;
                    }
                }
            }
            let o = ((y * nw + x) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (sum[c] / (k * k)) as u8;
            }
        }
    }
    let mut bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut bytes, nw, nh);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(&out).map_err(|e| e.to_string())?;
    }
    Ok((bytes, nw, nh, k))
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    fn png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut enc = png::Encoder::new(&mut bytes, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .unwrap()
            .write_image_data(&rgba.repeat((w * h) as usize))
            .unwrap();
        bytes
    }

    #[test]
    fn asks_a_line_and_reads_a_line() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let game = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut out = s.try_clone().unwrap();
            let mut lines = BufReader::new(s).lines();
            let first: J = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
            assert_eq!(first["cmd"], "status");
            out.write_all(b"{\"ok\":true,\"time_ms\":7}\n").unwrap();
            lines.next().unwrap().unwrap();
            out.write_all(b"{\"ok\":false,\"error\":\"nope\"}\n")
                .unwrap();
        });
        let mut link = Link::connect(&addr).unwrap();
        assert_eq!(
            link.ask(serde_json::json!({"cmd": "status"})).unwrap()["time_ms"],
            7
        );
        assert_eq!(
            link.ask(serde_json::json!({"cmd": "x"})).unwrap_err(),
            "nope"
        );
        game.join().unwrap();
        assert!(link.ask(serde_json::json!({"cmd": "status"})).is_err());
        assert!(Link::connect("127.0.0.1:1").is_err());
    }

    #[test]
    fn shrinks_wide_screenshots_by_whole_factors() {
        let (small, w, h, k) = shrink_png(&png(2560, 1440, [10, 20, 30, 255]), 1280).unwrap();
        assert_eq!((w, h, k), (1280, 720, 2));
        let (same, _, _, k) = shrink_png(&small, 1280).unwrap();
        assert_eq!((k, same), (1, small.clone()));
        let (_, w, _, k) = shrink_png(&png(3000, 10, [0; 4]), 1280).unwrap();
        assert_eq!((w, k), (1000, 3));
        assert!(shrink_png(b"junk", 100).is_err());
    }
}
