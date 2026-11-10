//! `thedas sim`: run the headless simulation for a module and area.

use anyhow::{Context, Result};
use std::path::Path;
use thedas_sim::Sim;

pub struct Options<'a> {
    pub install: &'a Path,
    pub module: &'a str,
    pub area: Option<&'a str>,
    pub waypoint: Option<&'a str>,
    pub ticks: u64,
    pub tick_ms: u64,
    pub trace: Option<&'a Path>,
    pub seed: u64,
    /// Tag of a creature to talk to after loading; replies are picked by index in order.
    pub talk: Option<&'a str>,
    pub replies: &'a [usize],
}

/// Opens the install, loads the module and area.
pub fn start(o: &Options) -> Result<Sim> {
    thedas_sim::session::open(&thedas_sim::session::SessionOptions {
        install: o.install.to_path_buf(),
        module: o.module.to_string(),
        area: o.area.map(str::to_string),
        waypoint: o.waypoint.map(str::to_string),
        seed: o.seed,
        keep_trace: o.trace.is_some(),
    })
    .with_context(|| format!("opening module {} in {}", o.module, o.install.display()))
}

pub fn run(o: &Options) -> Result<()> {
    let mut sim = start(o)?;
    for _ in 0..o.ticks.max(1) {
        sim.tick(o.tick_ms);
    }
    if let Some(tag) = o.talk {
        let target = sim
            .world
            .by_tag(tag)
            .next()
            .map(|x| x.id)
            .with_context(|| format!("no object tagged {tag}"))?;
        let hero = sim.world.hero.unwrap_or(target);
        if sim.start_conversation(hero, target, "") {
            let mut picks = o.replies.iter();
            while let Some(st) = sim.dialog_state() {
                println!("{}: {}", st.speaker, st.line);
                for (i, (_, text)) in st.replies.iter().enumerate() {
                    println!("    [{i}] {text}");
                }
                if st.ended {
                    println!("(conversation ends)");
                    break;
                }
                let pick = picks.next().copied().unwrap_or(0);
                println!("  -> {pick}");
                sim.choose_reply(pick);
                sim.tick(0);
            }
        } else {
            println!("{tag} has no conversation");
        }
    }
    let tr = &sim.world.trace;
    println!(
        "time {} ms, {} objects, {} scripts run, {} events queued",
        sim.world.time_ms,
        sim.world.objects.len(),
        tr.scripts_run,
        sim.world.queue.len()
    );
    if !tr.script_errors.is_empty() {
        println!("script errors:");
        for (e, n) in &tr.script_errors {
            println!("  {n:6} {e}");
        }
    }
    let mut un: Vec<(&String, &u64)> = tr.unimplemented.iter().collect();
    un.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    println!(
        "{} engine functions implemented were called {} times; {} unimplemented ones were called {} times",
        tr.implemented_calls.len(),
        tr.implemented_calls.values().sum::<u64>(),
        un.len(),
        un.iter().map(|x| *x.1).sum::<u64>()
    );
    for (f, n) in un.iter().take(25) {
        println!("  {n:6} {f}");
    }
    if let Some(path) = o.trace {
        std::fs::write(path, tr.to_jsonl())?;
        eprintln!(
            "wrote {} trace entries to {}",
            tr.entries.len(),
            path.display()
        );
    }
    Ok(())
}
