//! Parses every movie given and reports what the parser kept, skipped and failed on.
//! Usage: `cargo run -p thedas-gfx --example gfxscan -- DIR/*.gfx`

use thedas_gfx::swf::{Character, Movie};

fn main() {
    let mut bad = 0;
    for path in std::env::args().skip(1) {
        let Ok(data) = std::fs::read(&path) else {
            eprintln!("{path}: unreadable");
            continue;
        };
        match Movie::parse(&data) {
            Ok(m) => {
                let mut kinds = [0usize; 6];
                for c in m.characters.values() {
                    kinds[match c {
                        Character::Shape(_) => 0,
                        Character::Morph(_) => 1,
                        Character::Sprite(_) => 2,
                        Character::EditText(_) => 3,
                        Character::Font(_) => 4,
                        Character::Image(_) => 5,
                    }] += 1;
                }
                if !m.damaged.is_empty() {
                    bad += 1;
                }
                println!(
                    "{:28} frames {:4} shapes {:4} morph {} sprites {:4} text {:3} fonts {} images {:4} exports {:4} skipped {:?} damaged {:?}",
                    path.rsplit('/').next().unwrap_or(&path),
                    m.root.frames.len(),
                    kinds[0],
                    kinds[1],
                    kinds[2],
                    kinds[3],
                    kinds[4],
                    kinds[5],
                    m.exports.len(),
                    m.skipped,
                    m.damaged
                );
            }
            Err(e) => {
                bad += 1;
                println!("{path}: {e}");
            }
        }
    }
    if bad > 0 {
        std::process::exit(1);
    }
}
