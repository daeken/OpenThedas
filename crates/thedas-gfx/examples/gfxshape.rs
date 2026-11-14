//! Prints a character's definition: `gfxshape MOVIE.gfx ID...`
use thedas_gfx::swf::{Character, Movie};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let m = Movie::parse(&std::fs::read(&args[1]).expect("movie")).expect("parse");
    for id in &args[2..] {
        let id: u16 = id.parse().expect("id");
        match m.characters.get(&id) {
            Some(Character::Shape(s)) => println!(
                "{id}: shape bounds {:?} fills {:?} lines {}",
                s.bounds,
                s.fills,
                s.lines.len()
            ),
            Some(Character::Image(i)) => println!("{id}: image {:?}", i),
            Some(c) => println!("{id}: {:?}", std::mem::discriminant(c)),
            None => println!("{id}: none"),
        }
    }
}
