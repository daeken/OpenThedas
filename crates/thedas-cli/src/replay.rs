//! Strict, read-only report ingestion and replay. Crash recovery's permissive log reader
//! deliberately accepts a torn tail; an F8 reproduction must account for every line.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use thedas_sim::Sim;
use thedas_sim::commands::{InputEntry, SimCommand};
use thedas_sim::replay::{JournalHeader, JournalLine, load_checkpoint, world_hash};
use thedas_sim::save::{SAVE_FORMAT, SAVE_REVISION, SaveGame};

const BUILD: &str = match option_env!("THEDAS_GIT_COMMIT") {
    Some(build) => build,
    None => "dev",
};
const FILES: [&str; 4] = [
    "journal_checkpoint.json",
    "journal.jsonl",
    "save_now.json",
    "report.json",
];
const MAX_FILE: u64 = 128 * 1024 * 1024;

struct ReportFiles(BTreeMap<String, Vec<u8>>);

impl ReportFiles {
    fn open(path: &Path) -> Result<Self> {
        let mut files = BTreeMap::new();
        if path.is_dir() {
            for name in FILES {
                let file = path.join(name);
                if file.exists() {
                    let f = std::fs::File::open(&file)
                        .with_context(|| format!("reading {}", file.display()))?;
                    files.insert(name.to_string(), read_limited(f)?);
                }
            }
        } else {
            let f =
                std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
            let mut zip = zip::ZipArchive::new(f).context("opening report zip")?;
            let mut folder = None;
            for i in 0..zip.len() {
                let entry = zip.by_index(i).context("reading report zip entry")?;
                let enclosed = entry.enclosed_name().context("unsafe path in report zip")?;
                let Some(name) = enclosed.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if entry.is_dir() || !FILES.contains(&name) {
                    continue;
                }
                let parent = enclosed.parent().unwrap_or(Path::new("")).to_path_buf();
                if let Some(folder) = &folder {
                    ensure!(
                        *folder == parent,
                        "zip contains more than one report folder"
                    );
                } else {
                    folder = Some(parent);
                }
                let name = name.to_string();
                ensure!(!files.contains_key(&name), "duplicate {name} in report zip");
                ensure!(entry.size() <= MAX_FILE, "{name} exceeds 128 MiB");
                files.insert(name, read_limited(entry)?);
            }
        }
        let files = Self(files);
        for name in &FILES[..3] {
            files.get(name)?;
        }
        Ok(files)
    }

    fn get(&self, name: &str) -> Result<&[u8]> {
        self.0.get(name).map(Vec::as_slice).with_context(|| {
            format!(
                "missing {name}; report may predate journals or its upload left the journal out"
            )
        })
    }

    fn install(&self) -> Option<PathBuf> {
        let info: Value = serde_json::from_slice(self.0.get("report.json")?).ok()?;
        info.pointer("/settings/install/root")
            .and_then(Value::as_str)
            .filter(|s| !s.starts_with('~'))
            .map(PathBuf::from)
    }
}

fn read_limited(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE,
        "report file exceeds 128 MiB"
    );
    Ok(bytes)
}

struct Journal {
    header: JournalHeader,
    lines: Vec<JournalLine>,
    inputs: u64,
}

impl Journal {
    fn parse(bytes: &[u8], build: &str) -> Result<Self> {
        let text = std::str::from_utf8(bytes).context("journal.jsonl is not UTF-8")?;
        let mut source = text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty());
        let (i, first) = source.next().context("journal.jsonl has no header")?;
        let first: JournalLine = serde_json::from_str(first)
            .with_context(|| format!("invalid journal.jsonl line {}", i + 1))?;
        let JournalLine::Header(header) = &first else {
            bail!("journal.jsonl has no header");
        };
        ensure!(
            header.build == build,
            "build differs: journal build {}, this build {build}; build the journal's commit to replay",
            header.build
        );
        // 'dev' cannot establish that two binaries came from the same source.
        ensure!(build != "dev", "this binary has no git build identity");
        ensure!(
            header.save_revision == SAVE_REVISION,
            "journal save revision {}, this build supports {SAVE_REVISION}",
            header.save_revision
        );
        let header = header.clone();
        let mut lines = vec![first];
        for (i, line) in source {
            lines.push(
                serde_json::from_str::<JournalLine>(line)
                    .with_context(|| format!("invalid journal.jsonl line {}", i + 1))?,
            );
        }
        let mut inputs = 0u64;
        let mut have_input = false;
        for line in &lines[1..] {
            let count = match line {
                JournalLine::Header(_) => bail!("second header in journal.jsonl"),
                JournalLine::Input(_) => {
                    have_input = true;
                    1
                }
                JournalLine::Repeat { n } => {
                    ensure!(have_input && *n > 0, "invalid repeat in journal.jsonl");
                    u64::from(*n)
                }
                JournalLine::Check { index, .. } => {
                    ensure!(
                        *index == inputs,
                        "journal check index {index} follows {inputs} inputs"
                    );
                    0
                }
            };
            inputs = inputs
                .checked_add(count)
                .context("journal input count overflow")?;
        }
        Ok(Self {
            header,
            lines,
            inputs,
        })
    }
}

fn save(bytes: &[u8], name: &str) -> Result<SaveGame> {
    let save: SaveGame =
        serde_json::from_slice(bytes).with_context(|| format!("parsing {name}"))?;
    ensure!(
        save.format == SAVE_FORMAT,
        "unsupported {name} format {}",
        save.format
    );
    ensure!(
        save.revision == SAVE_REVISION,
        "{name} revision {}, this build supports {SAVE_REVISION}",
        save.revision
    );
    Ok(save)
}

struct Progress {
    applied: u64,
    stopped: bool,
    drift: Option<String>,
}

fn replay(sim: &mut Sim, journal: &Journal, until: Option<u64>) -> Result<Progress> {
    ensure!(
        journal.header.checkpoint == world_hash(sim),
        "checkpoint hash differs: journal {:016x}, loaded {:016x}",
        journal.header.checkpoint,
        world_hash(sim)
    );
    if let Some(until) = until {
        ensure!(
            until >= sim.world.time_ms,
            "--until {until} precedes checkpoint at {} ms",
            sim.world.time_ms
        );
    }
    sim.input_log = None;
    let mut progress = Progress {
        applied: 0,
        stopped: false,
        drift: None,
    };
    let mut last: Option<&InputEntry> = None;
    'lines: for line in &journal.lines[1..] {
        let (entry, n) = match line {
            JournalLine::Input(e) => {
                last = Some(e);
                (e, 1)
            }
            JournalLine::Repeat { n } => (last.expect("validated repeat"), *n),
            JournalLine::Check { index, t, hash } => {
                let got = world_hash(sim);
                if got != *hash || sim.world.time_ms != *t {
                    progress.drift = Some(format!(
                        "journal diverged at input {index}: expected {hash:016x} at {t} ms, got {got:016x} at {} ms",
                        sim.world.time_ms
                    ));
                    break;
                }
                continue;
            }
            JournalLine::Header(_) => unreachable!("validated header"),
        };
        for _ in 0..n {
            // Input timestamps are AFTER application. Repeats carry no fresh timestamp.
            // Only a tick can advance the clock; near the limit keep a snapshot in case
            // it crosses. Paused ticks can stay at the same time and must still replay.
            let before = until.and_then(|limit| match entry.command {
                SimCommand::Tick { ms } if sim.world.time_ms.saturating_add(ms) > limit => {
                    Some(sim.snapshot())
                }
                _ => None,
            });
            sim.apply(entry.source, entry.command.clone());
            if until.is_some_and(|limit| sim.world.time_ms > limit) {
                sim.restore(before.context("non-tick command advanced game time")?);
                progress.stopped = true;
                break 'lines;
            }
            progress.applied += 1;
        }
    }
    Ok(progress)
}

/// First structural difference, in stable key order. JSON pointer paths name object IDs
/// and fields, e.g. /world/objects/42/position/0; show tags too when available.
fn difference(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    if expected == actual {
        return None;
    }
    match (expected, actual) {
        (Value::Object(a), Value::Object(b)) => {
            let keys: BTreeSet<_> = a.keys().chain(b.keys()).collect();
            for key in keys {
                let child = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                match (a.get(key), b.get(key)) {
                    (Some(a), Some(b)) => {
                        if let Some(diff) = difference(a, b, &child) {
                            let tag = expected.get("tag").and_then(Value::as_str);
                            return Some(match tag {
                                Some(tag) => format!("{diff} (object tag {tag:?})"),
                                None => diff,
                            });
                        }
                    }
                    _ => {
                        return Some(format!(
                            "{child}: {}",
                            if a.contains_key(key) {
                                "missing from replay"
                            } else {
                                "extra in replay"
                            }
                        ));
                    }
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                if let Some(diff) = difference(a, b, &format!("{path}/{i}")) {
                    return Some(diff);
                }
            }
            return Some(format!(
                "{path}: expected {} entries, replay has {}",
                a.len(),
                b.len()
            ));
        }
        _ => {
            return Some(format!(
                "{path}: expected {}, replay {}",
                brief(expected),
                brief(actual)
            ));
        }
    }
    None
}

fn brief(value: &Value) -> String {
    let text = value.to_string();
    let mut chars = text.chars();
    let prefix: String = chars.by_ref().take(160).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn validate_output(report: &Path, out: &Path) -> Result<()> {
    let source = report.canonicalize()?;
    let dest = if out.exists() {
        out.canonicalize()?
    } else {
        let parent = out
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        parent
            .canonicalize()?
            .join(out.file_name().context("--save needs a file name")?)
    };
    ensure!(
        dest != source && !(source.is_dir() && dest.starts_with(&source)),
        "--save would modify the input report; choose an output outside it"
    );
    Ok(())
}

pub fn run(
    path: &Path,
    install: Option<&Path>,
    until: Option<u64>,
    out: Option<&Path>,
) -> Result<i32> {
    let files = ReportFiles::open(path)?;
    if let Some(out) = out {
        validate_output(path, out)?;
    }
    let journal = Journal::parse(files.get("journal.jsonl")?, BUILD)?;
    println!("journal build {}; this build {BUILD}", journal.header.build);
    let checkpoint = save(
        files.get("journal_checkpoint.json")?,
        "journal_checkpoint.json",
    )?;
    let expected = save(files.get("save_now.json")?, "save_now.json")?;
    ensure!(
        checkpoint.module == expected.module,
        "checkpoint and save_now.json belong to different modules"
    );
    let install = install
        .map(Path::to_path_buf)
        .or_else(thedas_sim::session::install_from_env)
        .or_else(|| files.install())
        .context("give --install or set THEDAS_INSTALL (the report has no usable install path)")?;
    let area = checkpoint
        .world
        .area
        .and_then(|id| checkpoint.world.get(id))
        .map(|o| o.template.clone());
    let mut sim = thedas_sim::session::open(&thedas_sim::session::SessionOptions {
        base_vfs: None,
        install,
        module: checkpoint.module.clone(),
        area,
        waypoint: None,
        seed: 1,
        keep_trace: false,
        cutscenes: thedas_sim::sim::CutscenePolicy::Play,
        chargen: thedas_sim::sim::ChargenPolicy::Manual,
    })
    .context("opening replay resources")?;
    // App::Live::open sets this before it starts a journal. It lives outside SaveGame.
    sim.party_picker_policy = thedas_sim::sim::PartyPickerPolicy::Player;
    load_checkpoint(&mut sim, checkpoint)?;
    let progress = replay(&mut sim, &journal, until)?;
    if let Some(out) = out {
        std::fs::write(out, sim.save_json()?)
            .with_context(|| format!("writing {}", out.display()))?;
        println!("saved {}", out.display());
    }
    let actual = serde_json::to_value(sim.save_game())?;
    let f8_time = expected.world.time_ms;
    let expected = serde_json::to_value(expected)?;
    let diff = difference(&expected, &actual, "");
    println!(
        "replayed {}/{} inputs; time {} ms (F8 {f8_time} ms); world hash {:016x}",
        progress.applied,
        journal.inputs,
        sim.world.time_ms,
        world_hash(&sim)
    );
    if let Some(drift) = &progress.drift {
        println!("{drift}");
    }
    if progress.stopped {
        println!(
            "stopped at --until {}; F8 moment not reached",
            until.unwrap()
        );
    }
    if let Some(diff) = &diff {
        println!("MISMATCH save_now.json: {diff}");
    } else if progress.drift.is_none() && !progress.stopped {
        println!("MATCH save_now.json (complete saved state)");
    }
    Ok(i32::from(
        diff.is_some() || progress.drift.is_some() || progress.stopped,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use std::sync::Arc;
    use thedas_sim::commands::CommandSource;
    use thedas_sim::replay::JournalWriter;
    use thedas_sim::world::ObjectKind;

    const TEST_BUILD: &str = "synthetic-build";

    fn sim() -> Sim {
        let mut s = Sim::new(
            Arc::new(thedas_vfs::Vfs::new()),
            Arc::new(thedas_script::EngineTable::default()),
            3,
        );
        let area = s.world.spawn(ObjectKind::Area, "area");
        s.world.area = Some(area);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        s.world.get_mut(hero).unwrap().area = Some(area);
        s
    }

    fn fixture() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut s = sim();
        let checkpoint = s.save_json().unwrap();
        let mut writer = JournalWriter::start(&mut s, TEST_BUILD, Vec::new()).unwrap();
        writer.check_every = 4;
        for _ in 0..6 {
            s.apply(CommandSource::App, SimCommand::Tick { ms: 100 });
            writer.take(&mut s).unwrap();
        }
        (checkpoint, writer.into_inner(), s.save_json().unwrap())
    }

    #[test]
    fn checkpoint_load_keeps_resource_tables_and_saved_state_without_load_events() {
        let mut live = sim();
        live.world.time_ms = 123;
        live.world.training.mode = 9;
        let bytes = live.save_json().unwrap();
        let expected: Value = serde_json::from_slice(&bytes).unwrap();
        let mut again = sim();
        let tables = again.world.tables();
        load_checkpoint(&mut again, save(&bytes, "checkpoint").unwrap()).unwrap();
        assert!(Arc::ptr_eq(&tables, &again.world.tables()));
        assert_eq!(expected, serde_json::to_value(again.save_game()).unwrap());
        assert_eq!(again.loads, 0);
        for (format, revision, module) in [
            (999, SAVE_REVISION, ""),
            (SAVE_FORMAT, 999, ""),
            (SAVE_FORMAT, SAVE_REVISION, "another module"),
        ] {
            let mut checkpoint = again.save_game();
            checkpoint.format = format;
            checkpoint.revision = revision;
            checkpoint.module = module.into();
            assert!(load_checkpoint(&mut again, checkpoint).is_err());
            assert_eq!(expected, serde_json::to_value(again.save_game()).unwrap());
        }
    }

    #[test]
    fn complete_report_matches_after_serialized_checkpoint_and_repeated_inputs() {
        let (checkpoint, log, expected) = fixture();
        let mut s = sim();
        load_checkpoint(&mut s, save(&checkpoint, "checkpoint").unwrap()).unwrap();
        let journal = Journal::parse(&log, TEST_BUILD).unwrap();
        let result = replay(&mut s, &journal, None).unwrap();
        assert_eq!(result.applied, 6);
        assert!(!result.stopped);
        assert!(result.drift.is_none());
        assert_eq!(
            difference(
                &serde_json::from_slice(&expected).unwrap(),
                &serde_json::to_value(s.save_game()).unwrap(),
                ""
            ),
            None
        );
    }

    #[test]
    fn until_stops_inside_repeat_without_splitting_tick() {
        let (checkpoint, log, _) = fixture();
        let mut s = sim();
        load_checkpoint(&mut s, save(&checkpoint, "checkpoint").unwrap()).unwrap();
        let journal = Journal::parse(&log, TEST_BUILD).unwrap();
        let result = replay(&mut s, &journal, Some(250)).unwrap();
        assert_eq!(result.applied, 2);
        assert_eq!(s.world.time_ms, 200);
        assert!(result.stopped);
        assert!(result.drift.is_none());
        let writer = JournalWriter::start(&mut s, TEST_BUILD, Vec::new()).unwrap();
        let later = Journal::parse(&writer.into_inner(), TEST_BUILD).unwrap();
        let error = replay(&mut s, &later, Some(100)).err().unwrap().to_string();
        assert!(error.contains("precedes checkpoint"), "{error}");
    }

    #[test]
    fn until_allows_zero_time_repeats_and_a_limit_after_the_end() {
        let mut s = sim();
        let checkpoint = s.save_game();
        let mut writer = JournalWriter::start(&mut s, TEST_BUILD, Vec::new()).unwrap();
        for _ in 0..6 {
            s.apply(CommandSource::App, SimCommand::Tick { ms: 0 });
            writer.take(&mut s).unwrap();
        }
        let journal = Journal::parse(&writer.into_inner(), TEST_BUILD).unwrap();
        load_checkpoint(&mut s, checkpoint).unwrap();
        let result = replay(&mut s, &journal, Some(0)).unwrap();
        assert_eq!(result.applied, 6);
        assert!(!result.stopped);
        let (checkpoint, log, _) = fixture();
        load_checkpoint(&mut s, save(&checkpoint, "checkpoint").unwrap()).unwrap();
        let journal = Journal::parse(&log, TEST_BUILD).unwrap();
        let result = replay(&mut s, &journal, Some(1000)).unwrap();
        assert_eq!(result.applied, 6);
        assert!(!result.stopped);
    }

    #[test]
    fn strict_reader_refuses_build_revision_corruption_and_bad_structure() {
        let (_, log, _) = fixture();
        let err = Journal::parse(&log, "another-build")
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains(TEST_BUILD) && err.contains("another-build"));
        let text = String::from_utf8(log).unwrap();
        for bad in [
            format!("{text}{{\"line\":"),
            format!("{text}{}", text.lines().next().unwrap()),
            text.replace("\"index\":4", "\"index\":5"),
            text.replace(
                &format!("\"save_revision\":{SAVE_REVISION}"),
                "\"save_revision\":999",
            ),
        ] {
            assert!(Journal::parse(bad.as_bytes(), TEST_BUILD).is_err(), "{bad}");
        }
        let orphan = format!(
            "{}\n{{\"line\":\"repeat\",\"n\":1}}",
            text.lines().next().unwrap()
        );
        assert!(Journal::parse(orphan.as_bytes(), TEST_BUILD).is_err());
    }

    #[test]
    fn wrong_checkpoint_is_refused_and_drift_keeps_the_actual_divergent_state() {
        let (checkpoint, log, _) = fixture();
        let mut s = sim();
        let mut journal = Journal::parse(&log, TEST_BUILD).unwrap();
        s.world.time_ms = 1;
        assert!(replay(&mut s, &journal, None).is_err());
        load_checkpoint(&mut s, save(&checkpoint, "checkpoint").unwrap()).unwrap();
        for line in &mut journal.lines {
            if let JournalLine::Check { hash, .. } = line {
                *hash ^= 1;
            }
        }
        let result = replay(&mut s, &journal, None).unwrap();
        assert!(result.drift.unwrap().contains("input 4"));
        assert_eq!(s.world.time_ms, 400);
        assert_eq!(result.applied, 4);
    }

    #[test]
    fn structural_diff_names_fields_outside_the_world_hash() {
        let mut s = sim();
        let expected = serde_json::to_value(s.save_game()).unwrap();
        let hash = world_hash(&s);
        let hero = s.world.hero.unwrap();
        s.world.get_mut(hero).unwrap().tag = "changed".into();
        assert_eq!(world_hash(&s), hash);
        let actual = serde_json::to_value(s.save_game()).unwrap();
        let diff = difference(&expected, &actual, "").unwrap();
        assert!(
            diff.contains(&format!("/world/objects/{hero}/tag")),
            "{diff}"
        );
        assert!(diff.contains("hero") && diff.contains("changed"));
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("thedas-replay-{}-{id}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn folders_and_wrapped_zips_read_the_same_synthetic_report() {
        let dir = Temp::new();
        let (checkpoint, log, now) = fixture();
        let entries = [
            (FILES[0], checkpoint.as_slice()),
            (FILES[1], log.as_slice()),
            (FILES[2], now.as_slice()),
        ];
        for (name, bytes) in entries {
            std::fs::write(dir.0.join(name), bytes).unwrap();
        }
        let folder = ReportFiles::open(&dir.0).unwrap();
        let names: Vec<_> = entries
            .iter()
            .map(|(name, _)| format!("report/{name}"))
            .collect();
        let wrapped: Vec<_> = names
            .iter()
            .zip(entries)
            .map(|(name, (_, bytes))| (name.as_str(), bytes))
            .collect();
        let path = dir.0.join("report.zip");
        std::fs::write(&path, zip(&wrapped)).unwrap();
        assert_eq!(folder.0, ReportFiles::open(&path).unwrap().0);
    }

    #[test]
    fn archives_reject_traversal_ambiguous_reports_and_missing_journals() {
        let dir = Temp::new();
        let path = dir.0.join("bad.zip");
        for entries in [
            vec![("../journal.jsonl", b"".as_slice())],
            vec![
                ("a/journal.jsonl", b"".as_slice()),
                ("b/save_now.json", b"".as_slice()),
            ],
            vec![("save_now.json", b"{}".as_slice())],
        ] {
            std::fs::write(&path, zip(&entries)).unwrap();
            assert!(ReportFiles::open(&path).is_err());
        }
    }

    #[test]
    fn save_cannot_modify_the_report_or_its_other_files() {
        let dir = Temp::new();
        let report = dir.0.join("report");
        std::fs::create_dir(&report).unwrap();
        let existing = report.join("note.txt");
        std::fs::write(&existing, "test note").unwrap();
        assert!(validate_output(&report, &existing).is_err());
        assert!(validate_output(&report, &report.join("new.json")).is_err());
        assert!(validate_output(&report, &dir.0.join("outside.json")).is_ok());
        assert!(validate_output(&existing, &existing).is_err());
    }
}
