//! Disassembles a movie's ActionScript: `gfxdis MOVIE.gfx [FILTER]` prints every action block
//! (init actions by sprite, frame actions, clip events), one action per line, constants
//! resolved. With FILTER, only blocks containing that text are printed.

use thedas_gfx::swf::read::Reader;
use thedas_gfx::swf::{Character, Command, Movie, Timeline};

fn name(op: u8) -> &'static str {
    match op {
        0x04 => "NextFrame",
        0x05 => "PrevFrame",
        0x06 => "Play",
        0x07 => "Stop",
        0x0A => "Add",
        0x0B => "Subtract",
        0x0C => "Multiply",
        0x0D => "Divide",
        0x0E => "Equals",
        0x0F => "Less",
        0x10 => "And",
        0x11 => "Or",
        0x12 => "Not",
        0x13 => "StringEquals",
        0x17 => "Pop",
        0x18 => "ToInteger",
        0x1C => "GetVariable",
        0x1D => "SetVariable",
        0x21 => "StringAdd",
        0x22 => "GetProperty",
        0x23 => "SetProperty",
        0x26 => "Trace",
        0x34 => "GetTime",
        0x3A => "Delete",
        0x3B => "Delete2",
        0x3C => "DefineLocal",
        0x3D => "CallFunction",
        0x3E => "Return",
        0x3F => "Modulo",
        0x40 => "NewObject",
        0x41 => "DefineLocal2",
        0x42 => "InitArray",
        0x43 => "InitObject",
        0x44 => "TypeOf",
        0x46 => "Enumerate",
        0x47 => "Add2",
        0x48 => "Less2",
        0x49 => "Equals2",
        0x4A => "ToNumber",
        0x4B => "ToString",
        0x4C => "PushDuplicate",
        0x4D => "StackSwap",
        0x4E => "GetMember",
        0x4F => "SetMember",
        0x50 => "Increment",
        0x51 => "Decrement",
        0x52 => "CallMethod",
        0x53 => "NewMethod",
        0x54 => "InstanceOf",
        0x55 => "Enumerate2",
        0x60 => "BitAnd",
        0x61 => "BitOr",
        0x62 => "BitXor",
        0x63 => "BitLShift",
        0x64 => "BitRShift",
        0x65 => "BitURShift",
        0x66 => "StrictEquals",
        0x67 => "Greater",
        0x69 => "Extends",
        0x81 => "GotoFrame",
        0x83 => "GetURL",
        0x87 => "StoreRegister",
        0x88 => "ConstantPool",
        0x8C => "GoToLabel",
        0x8E => "DefineFunction2",
        0x96 => "Push",
        0x99 => "Jump",
        0x9A => "GetURL2",
        0x9B => "DefineFunction",
        0x9D => "If",
        0x9F => "GotoFrame2",
        _ => "?",
    }
}

fn dis(code: &[u8], out: &mut String) {
    let mut pool: Vec<String> = Vec::new();
    let mut pc = 0;
    let mut depth_ends: Vec<usize> = Vec::new();
    while pc < code.len() {
        while depth_ends.last().is_some_and(|e| pc >= *e) {
            depth_ends.pop();
            out.push_str(&format!(
                "{:indent$}}}\n",
                "",
                indent = depth_ends.len() * 2
            ));
        }
        let at = pc;
        let op = code[pc];
        pc += 1;
        if op == 0 {
            break;
        }
        let mut body: &[u8] = &[];
        if op >= 0x80 {
            if pc + 2 > code.len() {
                break;
            }
            let len = u16::from_le_bytes([code[pc], code[pc + 1]]) as usize;
            pc += 2;
            body = code.get(pc..pc + len).unwrap_or(&[]);
            pc += len;
        }
        let ind = depth_ends.len() * 2;
        let mut line = format!("{:ind$}{at:5}: {}", "", name(op), ind = ind);
        let mut r = Reader::new(body);
        match op {
            0x88 => {
                let n = r.u16().unwrap_or(0);
                pool = (0..n).filter_map(|_| r.cstr().ok()).collect();
                line.push_str(&format!(" [{} strings]", pool.len()));
            }
            0x96 => {
                let mut vals = Vec::new();
                while !r.is_empty() {
                    let Ok(t) = r.u8() else { break };
                    vals.push(match t {
                        0 => format!("{:?}", r.cstr().unwrap_or_default()),
                        1 => format!("{}", r.f32().unwrap_or(0.0)),
                        2 => "null".into(),
                        3 => "undefined".into(),
                        4 => format!("r{}", r.u8().unwrap_or(0)),
                        5 => format!("{}", r.u8().unwrap_or(0) != 0),
                        6 => format!("{}", r.f64_swapped().unwrap_or(0.0)),
                        7 => format!("{}", r.i32().unwrap_or(0)),
                        8 => {
                            let i = r.u8().unwrap_or(0) as usize;
                            format!("{:?}", pool.get(i).cloned().unwrap_or_default())
                        }
                        9 => {
                            let i = r.u16().unwrap_or(0) as usize;
                            format!("{:?}", pool.get(i).cloned().unwrap_or_default())
                        }
                        _ => "?".into(),
                    });
                }
                line.push(' ');
                line.push_str(&vals.join(", "));
            }
            0x8E | 0x9B => {
                let n = r.cstr().unwrap_or_default();
                let np = r.u16().unwrap_or(0);
                let mut params = Vec::new();
                if op == 0x8E {
                    let regs = r.u8().unwrap_or(0);
                    let flags = r.u16().unwrap_or(0);
                    for _ in 0..np {
                        let reg = r.u8().unwrap_or(0);
                        params.push(format!("{}@r{reg}", r.cstr().unwrap_or_default()));
                    }
                    line.push_str(&format!(" regs={regs} flags={flags:#x}"));
                } else {
                    for _ in 0..np {
                        params.push(r.cstr().unwrap_or_default());
                    }
                }
                let size = r.u16().unwrap_or(0) as usize;
                line.push_str(&format!(" {n}({}) {{", params.join(", ")));
                depth_ends.push(pc + size);
            }
            0x99 | 0x9D => {
                let off = r.i16().unwrap_or(0) as isize;
                line.push_str(&format!(" -> {}", pc as isize + off));
            }
            0x87 => line.push_str(&format!(" r{}", r.u8().unwrap_or(0))),
            0x81 => line.push_str(&format!(" {}", r.u16().unwrap_or(0))),
            0x8C => line.push_str(&format!(" {:?}", r.cstr().unwrap_or_default())),
            0x9F => line.push_str(&format!(" flags={}", r.u8().unwrap_or(0))),
            0x83 => line.push_str(&format!(
                " {:?} {:?}",
                r.cstr().unwrap_or_default(),
                r.cstr().unwrap_or_default()
            )),
            _ => {}
        }
        out.push_str(&line);
        out.push('\n');
    }
}

fn walk(m: &Movie, tl: &Timeline, who: &str, filter: &str) {
    for (fi, f) in tl.frames.iter().enumerate() {
        for (sid, code) in &f.init_actions {
            let mut s = String::new();
            dis(code, &mut s);
            if s.contains(filter) {
                let export = m
                    .exports
                    .iter()
                    .find(|(_, id)| *id == sid)
                    .map(|(n, _)| n.clone())
                    .unwrap_or_default();
                println!(
                    "== init sprite {sid} ({export}) in {who} frame {}\n{s}",
                    fi + 1
                );
            }
        }
        for code in &f.actions {
            let mut s = String::new();
            dis(code, &mut s);
            if s.contains(filter) {
                println!("== frame {} of {who}\n{s}", fi + 1);
            }
        }
        for c in &f.commands {
            if let Command::Place(p) = c {
                for a in &p.clip_actions {
                    let mut s = String::new();
                    dis(&a.actions, &mut s);
                    if s.contains(filter) {
                        println!(
                            "== clip event {:#x} on {:?} (depth {}) in {who} frame {}\n{s}",
                            a.events,
                            p.name,
                            p.depth,
                            fi + 1
                        );
                    }
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&args[1]).expect("movie");
    let filter = args.get(2).cloned().unwrap_or_default();
    let m = Movie::parse(&data).expect("parse");
    walk(&m, &m.root, "root", &filter);
    let mut ids: Vec<&u16> = m.characters.keys().collect();
    ids.sort();
    for id in ids {
        if let Some(Character::Sprite(t)) = m.characters.get(id) {
            walk(&m, t, &format!("sprite {id}"), &filter);
        }
    }
}
