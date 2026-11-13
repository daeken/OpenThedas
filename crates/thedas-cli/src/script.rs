//! `thedas script`: disassemble and run compiled scripts, and rank engine functions by how
//! often the shipped scripts call them.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde_json::json;
use thedas_script::vm::{NullHost, Value};
use thedas_script::{EngineTable, Ncs, Vm};
use thedas_vfs::install::TextureQuality;
use thedas_vfs::{DaoInstall, Erf, Vfs};

/// Loads the engine table (`script.ldf`) from an install's resource stack.
pub fn engine_table(vfs: &Vfs) -> Result<EngineTable> {
    let (data, _) = vfs.read("script.ldf").context("reading script.ldf")?;
    Ok(EngineTable::parse(&data)?)
}

fn open(install: &Path) -> Result<(DaoInstall, Vfs)> {
    let install = DaoInstall::open(install)?;
    let vfs = install.global_vfs(TextureQuality::High)?;
    Ok((install, vfs))
}

/// Reads `name` from the stack, or from a file if `name` is a path.
fn load_script(vfs: &Vfs, name: &str) -> Result<Ncs> {
    let data = if Path::new(name).is_file() {
        std::fs::read(name)?
    } else {
        let res = if name.contains('.') {
            name.to_string()
        } else {
            format!("{name}.ncs")
        };
        vfs.read(&res).with_context(|| format!("reading {res}"))?.0
    };
    Ok(Ncs::parse(&data)?)
}

pub fn disasm(install: &Path, name: &str) -> Result<()> {
    let (_, vfs) = open(install)?;
    let table = engine_table(&vfs)?;
    let ncs = load_script(&vfs, name)?;
    print!("{}", ncs.disassemble(Some(&table)));
    Ok(())
}

fn show(v: &Value) -> String {
    match v {
        Value::String(s) | Value::Resource(s) => format!("{s:?}"),
        Value::Object(o) => format!("object {o:#x}"),
        other => format!("{other:?}"),
    }
}

pub fn run(install: &Path, name: &str, caller: u32) -> Result<()> {
    let (_, vfs) = open(install)?;
    let table = Arc::new(engine_table(&vfs)?);
    let ncs = Arc::new(load_script(&vfs, name)?);
    let mut host = NullHost::default();
    let result = Vm::new(ncs, table).run(&mut host, caller);
    for (f, args) in &host.calls {
        let a: Vec<String> = args.iter().map(show).collect();
        println!("{f}({})", a.join(", "));
    }
    match result {
        Ok(r) => {
            println!(
                "-- finished after {} instructions{}",
                r.steps,
                r.value
                    .map(|v| format!(", result {}", show(&v)))
                    .unwrap_or_default()
            );
            Ok(())
        }
        Err(e) => bail!("script stopped: {e}"),
    }
}

/// Ranks engine functions by static call count across every distinct compiled script in the
/// install, and runs each script once against a host that implements nothing, to exercise the
/// VM. Writes JSON and Markdown reports.
pub fn stats(install: &Path, report: Option<&Path>) -> Result<()> {
    let (inst, vfs) = open(install)?;
    let table = Arc::new(engine_table(&vfs)?);
    let mut seen: HashSet<(String, u64)> = HashSet::new();
    let mut calls: BTreeMap<u16, (u64, u64)> = BTreeMap::new();
    let (mut scripts, mut parse_failed) = (0u64, 0u64);
    let mut vm_ok = 0u64;
    let mut vm_fail: BTreeMap<String, (u64, String)> = BTreeMap::new();
    for path in inst.all_archives()? {
        let Ok(erf) = Erf::open(&path) else { continue };
        if erf.is_encrypted() && erf.installed_key().is_none() {
            continue;
        }
        for e in &erf.entries {
            let name = e.display_name().to_ascii_lowercase();
            if !name.ends_with(".ncs") {
                continue;
            }
            let Ok(data) = erf.read(e, None) else {
                continue;
            };
            let hash = data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
                (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
            });
            if !seen.insert((name.clone(), hash)) {
                continue;
            }
            scripts += 1;
            let ncs = match Ncs::parse(&data) {
                Ok(n) => Arc::new(n),
                Err(_) => {
                    parse_failed += 1;
                    continue;
                }
            };
            let mut in_script: HashSet<u16> = HashSet::new();
            for (id, _) in ncs.actions() {
                let c = calls.entry(id).or_default();
                c.0 += 1;
                if in_script.insert(id) {
                    c.1 += 1;
                }
            }
            let mut vm = Vm::new(ncs, table.clone());
            vm.budget = 2_000_000;
            match vm.run(&mut NullHost::default(), 0) {
                Ok(_) => vm_ok += 1,
                Err(e) => {
                    let key = e.to_string();
                    let key = key.split(" at ").next().unwrap_or(&key).to_string();
                    let entry = vm_fail.entry(key).or_insert((0, name.clone()));
                    entry.0 += 1;
                }
            }
        }
    }
    let mut ranked: Vec<(u16, u64, u64)> = calls.iter().map(|(&id, &(n, s))| (id, n, s)).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let name = |id: u16| {
        table
            .function(id)
            .map_or_else(|| format!("#{id}"), |f| f.name.clone())
    };
    println!(
        "{scripts} distinct scripts, {parse_failed} failed to parse; {} engine functions called, {} declared",
        ranked.len(),
        table.functions.len()
    );
    println!("VM smoke run with a do-nothing host: {vm_ok} finished");
    for (k, (n, ex)) in &vm_fail {
        println!("  {n:6} {k} (e.g. {ex})");
    }
    println!("\nTop 40 by static call count:");
    for (id, n, s) in ranked.iter().take(40) {
        println!("{n:8} {s:6}  {id:4} {}", name(*id));
    }
    if let Some(path) = report {
        let entries: Vec<serde_json::Value> = ranked
            .iter()
            .enumerate()
            .map(|(rank, (id, n, s))| {
                let f = table.function(*id);
                json!({
                    "rank": rank + 1, "id": id, "name": name(*id), "calls": n, "scripts": s,
                    "signature": f.map(|f| format!("{} {}({})", table.type_name(&f.ret), f.name,
                        f.params.iter().map(|p| format!("{} {}", table.type_name(&p.ty), p.name)).collect::<Vec<_>>().join(", "))),
                })
            })
            .collect();
        let never: Vec<String> = table
            .functions
            .iter()
            .filter(|f| !calls.contains_key(&f.id))
            .map(|f| f.name.clone())
            .collect();
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&json!({
                "scripts": scripts, "parse_failed": parse_failed,
                "vm_smoke_ok": vm_ok,
                "vm_smoke_failures": vm_fail.iter().map(|(k, (n, ex))| json!({"error": k, "count": n, "example": ex})).collect::<Vec<_>>(),
                "functions_declared": table.functions.len(),
                "functions_called": ranked.len(),
                "ranking": entries,
                "never_called": never,
            }))?,
        )?;
        let md = path.with_extension("md");
        let mut s = String::new();
        s.push_str(&format!(
            "# Engine functions by static call count\n\n{scripts} distinct compiled scripts in the install; {} of {} engine functions are called at least once.\n\n| rank | id | function | calls | scripts |\n| --- | --- | --- | --- | --- |\n",
            ranked.len(),
            table.functions.len()
        ));
        for (rank, (id, n, sc)) in ranked.iter().enumerate() {
            s.push_str(&format!(
                "| {} | {id} | {} | {n} | {sc} |\n",
                rank + 1,
                name(*id)
            ));
        }
        std::fs::write(&md, s)?;
        eprintln!("wrote {} and {}", path.display(), md.display());
    }
    Ok(())
}
