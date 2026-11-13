//! One-press issue reports: F8 (or clicking both of the pad's sticks, or a `report` request
//! over `--control`) bundles what someone needs to see and reproduce a problem into a folder
//! of its own under `$HOME/.openthedas/reports`:
//!
//! - `screenshot.png`: the frame as the player saw it when they pressed;
//! - `save_now.json`: the game's state at the press (`Sim::save_json`);
//! - `save_before.json`: the game's state about a minute earlier, from a ring of snapshots
//!   kept in memory while playing ([`SNAPSHOT_EVERY`], [`SNAPSHOTS_KEPT`]);
//! - `report.json`: the build (version, commit), the platform and GPU, the window and the
//!   settings in force, where the game was (area, leader, party, conversation, cutscene,
//!   screens), the frame times, and what the snapshots cover;
//! - `log.txt`: the last [`LOG_LINES`] log lines;
//! - `trace.json`: the simulation's last [`TRACE_RECENT`] orders, script log lines,
//!   conversation lines, plot flags, failed scripts and missing functions and resources
//!   (`Trace::recent`), oldest first;
//! - `input.txt`: the player's input over the last [`INPUT_WINDOW`], timed from the earlier
//!   save;
//! - `note.txt`: what the player typed about it.
//!
//! This is an addition of ours, not the original's. The snapshots only read the
//! simulation: they do not run the module's PRESAVE event a real save runs (it runs
//! scripts), and none is taken while saving is refused (a training lesson), so each is a
//! state the game itself could have saved. Nothing leaves the machine.

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{Value as J, json};

/// How often a snapshot of the game is kept while playing.
pub const SNAPSHOT_EVERY: Duration = Duration::from_secs(20);
/// How soon a snapshot that could not be taken is tried again.
pub const SNAPSHOT_RETRY: Duration = Duration::from_secs(2);
/// How many snapshots are kept (the oldest goes): a minute and a bit back.
pub const SNAPSHOTS_KEPT: usize = 4;
/// The age of the snapshot a report calls "before" (the youngest at least this old).
pub const BEFORE_AGE: Duration = Duration::from_secs(50);
/// The log lines kept for a report.
pub const LOG_LINES: usize = 400;
/// The input kept for a report.
pub const INPUT_WINDOW: Duration = Duration::from_secs(120);
/// The simulation's recent trace entries kept for a report (`Trace::recent`).
pub const TRACE_RECENT: usize = 4000;
/// The frame times kept for a report's statistics.
pub const FRAMES_KEPT: usize = 600;

/// The game's state at one moment, as a save holds it.
#[derive(Clone)]
pub struct Snapshot {
    pub taken: Instant,
    /// The simulation's clock, milliseconds.
    pub time_ms: u64,
    pub area: String,
    pub save: Vec<u8>,
}

/// The last lines logged, for reports (see [`init_logging`]).
static LOG: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Logs as `builder` would, keeping the last [`LOG_LINES`] lines it prints for reports.
pub fn init_logging(mut builder: env_logger::Builder) {
    let inner = builder.build();
    let max = inner.filter();
    let started = Instant::now();
    if log::set_boxed_logger(Box::new(Tee { inner, started })).is_ok() {
        log::set_max_level(max);
    }
}

struct Tee {
    inner: env_logger::Logger,
    started: Instant,
}

impl log::Log for Tee {
    fn enabled(&self, m: &log::Metadata) -> bool {
        self.inner.enabled(m)
    }

    fn log(&self, record: &log::Record) {
        if self.inner.matches(record) {
            let line = format!(
                "{:9.3} {:5} {}: {}",
                self.started.elapsed().as_secs_f64(),
                record.level(),
                record.target(),
                record.args()
            );
            keep_line(line);
        }
        self.inner.log(record);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Keeps a line for reports (the log's, or one printed to the console).
pub fn keep_line(line: String) {
    let mut log = LOG.lock().unwrap_or_else(|p| p.into_inner());
    if log.len() >= LOG_LINES {
        log.pop_front();
    }
    log.push_back(line);
}

fn log_lines() -> Vec<String> {
    LOG.lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .cloned()
        .collect()
}

/// What a report keeps between presses: the snapshots, the input and the frame times.
pub struct Recorder {
    snapshots: VecDeque<Snapshot>,
    /// When the next snapshot is due (now, when none is).
    next_snapshot: Option<Instant>,
    input: VecDeque<(Instant, String)>,
    frames: VecDeque<f32>,
    /// Where reports are written.
    pub dir: PathBuf,
    /// A report waiting for its screenshot and the player's note.
    pending: Option<Pending>,
}

impl Recorder {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            snapshots: VecDeque::new(),
            next_snapshot: None,
            input: VecDeque::new(),
            frames: VecDeque::new(),
            dir: dir.into(),
            pending: None,
        }
    }

    /// `reports` in the player's data folder (see [`crate::saves::data_dir`]).
    pub fn default_dir() -> PathBuf {
        crate::saves::data_dir().join("reports")
    }

    /// One frame took `dt_ms`.
    pub fn frame(&mut self, dt_ms: f64) {
        if self.frames.len() >= FRAMES_KEPT {
            self.frames.pop_front();
        }
        self.frames.push_back(dt_ms as f32);
    }

    /// The player did something (`--steps` syntax where there is one).
    pub fn input(&mut self, what: impl Into<String>) {
        let now = Instant::now();
        while self
            .input
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) > INPUT_WINDOW)
        {
            self.input.pop_front();
        }
        self.input.push_back((now, what.into()));
    }

    /// Whether a snapshot is due now.
    pub fn snapshot_due(&self, now: Instant) -> bool {
        self.next_snapshot.is_none_or(|t| now >= t)
    }

    /// Keeps a snapshot (the oldest goes past [`SNAPSHOTS_KEPT`]).
    pub fn keep(&mut self, s: Snapshot) {
        self.next_snapshot = Some(s.taken + SNAPSHOT_EVERY);
        // The game's clock went back: another save was loaded, and the snapshots kept are
        // not of the game being played now.
        if self.snapshots.back().is_some_and(|b| b.time_ms > s.time_ms) {
            self.snapshots.clear();
        }
        if self.snapshots.len() >= SNAPSHOTS_KEPT {
            self.snapshots.pop_front();
        }
        self.snapshots.push_back(s);
    }

    /// A snapshot could not be taken (saving refused, nothing playing): try again in
    /// [`SNAPSHOT_RETRY`], not every frame.
    pub fn skip_snapshot(&mut self, now: Instant) {
        self.next_snapshot = Some(now + SNAPSHOT_RETRY);
    }

    /// Forgets the snapshots (a different game was loaded or started: they are not of it).
    pub fn forget(&mut self) {
        self.snapshots.clear();
        self.next_snapshot = None;
    }

    /// The snapshot a report made `now` calls "before": the youngest at least
    /// [`BEFORE_AGE`] old, else the oldest there is.
    pub fn before(&self, now: Instant) -> Option<&Snapshot> {
        self.snapshots
            .iter()
            .rev()
            .find(|s| now.duration_since(s.taken) >= BEFORE_AGE)
            .or_else(|| self.snapshots.front())
    }

    /// Whether a report is under way (waiting for its screenshot or note).
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Starts a report made now: everything but the screenshot and the note, which come
    /// later (`screenshot`, `note`). Writes what it has at once.
    pub fn start(&mut self, now_save: Option<Vec<u8>>, trace: Option<J>, mut info: J) -> PathBuf {
        let now = Instant::now();
        let written = crate::saves::now_secs();
        let area = info["game"]["area"].as_str().unwrap_or("none").to_string();
        let id = format!(
            "{}-{}",
            crate::saves::format_time(written)
                .replace([' ', ':'], "-")
                .replace("--", "-"),
            sanitize(&area)
        );
        let mut path = self.dir.join(&id);
        let mut n = 1;
        while path.exists() {
            n += 1;
            path = self.dir.join(format!("{id}-{n}"));
        }
        let before = self.before(now).cloned();
        info["id"] = json!(path.file_name().map(|f| f.to_string_lossy().to_string()));
        info["written"] = json!(written);
        info["snapshots"] = json!({
            "kept": self.snapshots.iter().map(|s| json!({
                "age_s": now.duration_since(s.taken).as_secs_f64(),
                "time_ms": s.time_ms,
                "area": s.area,
            })).collect::<Vec<_>>(),
            "before": before.as_ref().map(|s| json!({
                "age_s": now.duration_since(s.taken).as_secs_f64(),
                "time_ms": s.time_ms,
                "area": s.area,
            })),
        });
        info["frames"] = frame_stats(&self.frames);
        let since = before
            .as_ref()
            .map_or_else(|| self.input.front().map_or(now, |(t, _)| *t), |s| s.taken);
        let input: Vec<String> = self
            .input
            .iter()
            .map(|(t, what)| {
                let at = if *t >= since {
                    t.duration_since(since).as_secs_f64()
                } else {
                    -since.duration_since(*t).as_secs_f64()
                };
                format!("{at:8.3} {what}")
            })
            .collect();
        let files = Files {
            dir: path.clone(),
            info,
            now_save,
            before: before.map(|s| s.save),
            log: log_lines(),
            input,
            trace,
        };
        if let Err(e) = files.write() {
            log::warn!("report: writing {}: {e:#}", path.display());
        }
        self.pending = Some(Pending {
            dir: path.clone(),
            shot: None,
            note: None,
        });
        path
    }

    /// The report's screenshot came (a PNG).
    pub fn screenshot(&mut self, png: Result<Vec<u8>>) {
        let Some(p) = self.pending.as_mut() else {
            return;
        };
        let wrote = png.and_then(|png| {
            std::fs::write(p.dir.join("screenshot.png"), png).context("writing the screenshot")
        });
        if let Err(e) = &wrote {
            log::warn!("report: no screenshot: {e:#}");
        }
        p.shot = Some(wrote.is_ok());
    }

    /// The player's note came: `None` when they cancelled the report (it is deleted).
    pub fn note(&mut self, text: Option<String>) {
        if let Some(p) = self.pending.as_mut() {
            p.note = Some(text);
        }
    }

    /// Finishes the report once its screenshot and note are in. Returns its folder (`None`
    /// when cancelled) once done.
    pub fn finish(&mut self) -> Option<Option<PathBuf>> {
        let p = self.pending.as_ref()?;
        if p.shot.is_none() || p.note.is_none() {
            return None;
        }
        let p = self.pending.take()?;
        match p.note.flatten() {
            None => {
                let _ = std::fs::remove_dir_all(&p.dir);
                log::info!("report: cancelled");
                Some(None)
            }
            Some(text) => {
                if let Err(e) = std::fs::write(p.dir.join("note.txt"), text.as_bytes()) {
                    log::warn!("report: writing the note: {e}");
                }
                log::info!("report: written to {}", p.dir.display());
                Some(Some(p.dir))
            }
        }
    }
}

struct Pending {
    dir: PathBuf,
    /// The screenshot came (and was written).
    shot: Option<bool>,
    /// The note came: `Some(None)` cancelled.
    note: Option<Option<String>>,
}

/// A report's files, but the screenshot and the note.
struct Files {
    dir: PathBuf,
    info: J,
    now_save: Option<Vec<u8>>,
    before: Option<Vec<u8>>,
    log: Vec<String>,
    input: Vec<String>,
    trace: Option<J>,
}

impl Files {
    fn write(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        let put = |name: &str, bytes: &[u8]| {
            std::fs::write(self.dir.join(name), bytes).with_context(|| format!("writing {name}"))
        };
        put("report.json", &serde_json::to_vec_pretty(&self.info)?)?;
        if let Some(s) = &self.now_save {
            put("save_now.json", s)?;
        }
        if let Some(s) = &self.before {
            put("save_before.json", s)?;
        }
        let mut log = Vec::new();
        for l in &self.log {
            writeln!(log, "{l}")?;
        }
        put("log.txt", &log)?;
        let mut input = Vec::new();
        writeln!(
            input,
            "# seconds from save_before.json (or the first input kept), then what was done"
        )?;
        for l in &self.input {
            writeln!(input, "{l}")?;
        }
        put("input.txt", &input)?;
        if let Some(t) = &self.trace {
            put("trace.json", &serde_json::to_vec_pretty(t)?)?;
        }
        Ok(())
    }
}

/// Mean, 95th percentile and worst of the frame times kept (milliseconds).
fn frame_stats(frames: &VecDeque<f32>) -> J {
    if frames.is_empty() {
        return J::Null;
    }
    let mut v: Vec<f32> = frames.iter().copied().collect();
    v.sort_by(f32::total_cmp);
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    let p95 = v[((v.len() - 1) as f32 * 0.95) as usize];
    json!({
        "count": v.len(),
        "mean_ms": mean,
        "p95_ms": p95,
        "max_ms": v[v.len() - 1],
    })
}

/// A name safe as part of a folder's.
fn sanitize(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    if out.is_empty() { "none".into() } else { out }
}

/// The build: its version and the commit it was built from (changes not committed then
/// are not told).
pub fn build_info() -> J {
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "commit": option_env!("THEDAS_GIT_COMMIT"),
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    })
}

/// The install the game runs: where it is, its add-ins (which DLC and expansions it has,
/// which tells editions apart), the voice-over language played, and whether its volume
/// tells case apart (installs copied from Windows can differ only in case).
pub fn install_info(root: &std::path::Path, language: &str) -> J {
    let addins = thedas_vfs::DaoInstall::open(root)
        .and_then(|i| i.addins())
        .map(|a| {
            a.iter()
                .map(|a| json!({ "uid": a.uid, "enabled": a.enabled }))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "root": root.display().to_string(),
        "addins": addins,
        "language": language,
        "case_sensitive": case_sensitive(root),
    })
}

/// Whether the volume `path` is on tells case apart: the same path with its last name's
/// case swapped is not the same file. `None` when it cannot tell (no letters, not there).
fn case_sensitive(path: &std::path::Path) -> Option<bool> {
    let name = path.file_name()?.to_str()?;
    let swapped: String = name
        .chars()
        .map(|c| {
            if c.is_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    if swapped == name || !path.exists() {
        return None;
    }
    let other = path.with_file_name(swapped);
    Some(!other.exists())
}

/// Asks the player what went wrong, on a thread of its own: the answer arrives on the
/// receiver, `None` when they cancel. `preset` (a client's note) or `THEDAS_REPORT_NOTE`
/// (tests) answers instead. The question is a system dialog box (tinyfiledialogs: Windows,
/// macOS and Linux), outside the game's own interface. Provisional: an in-game form, which
/// the pad could fill in too, would replace it.
pub fn ask_note(preset: Option<String>) -> mpsc::Receiver<Option<String>> {
    let (tx, rx) = mpsc::channel();
    if let Some(text) = preset.or_else(|| std::env::var("THEDAS_REPORT_NOTE").ok()) {
        let _ = tx.send(Some(text));
        return rx;
    }
    let _ = std::thread::Builder::new()
        .name("report note".into())
        .spawn(move || {
            // tinyfiledialogs takes no quotes in its text.
            let note = tinyfiledialogs::input_box(
                "Report an issue",
                "What went wrong? Say what you expected and what happened instead. A \
                 screenshot, two saves and the game log are kept with it, on this computer \
                 only. Cancel drops the report.",
                "",
            );
            let _ = tx.send(note);
        });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(taken: Instant, time_ms: u64) -> Snapshot {
        Snapshot {
            taken,
            time_ms,
            area: "lot100ar".into(),
            save: format!("{{\"t\":{time_ms}}}").into_bytes(),
        }
    }

    #[test]
    fn the_ring_keeps_the_last_snapshots_and_picks_one_a_minute_back() {
        let mut r = Recorder::new("unused");
        let t0 = Instant::now();
        assert!(r.snapshot_due(t0));
        for i in 0..6u64 {
            r.keep(snap(t0 + SNAPSHOT_EVERY * i as u32, i * 20_000));
        }
        assert_eq!(r.snapshots.len(), SNAPSHOTS_KEPT);
        let now = t0 + SNAPSHOT_EVERY * 5 + Duration::from_secs(5);
        assert!(!r.snapshot_due(t0 + SNAPSHOT_EVERY * 5 + Duration::from_secs(1)));
        assert!(r.snapshot_due(now + SNAPSHOT_EVERY));
        // Kept: 40, 60, 80, 100 s; at 105 s, the youngest at least 50 s old is 40 s's.
        assert_eq!(r.before(now).map(|s| s.time_ms), Some(40_000));
        // Early on, the oldest there is.
        let mut early = Recorder::new("unused");
        early.keep(snap(t0, 0));
        assert_eq!(
            early.before(t0 + Duration::from_secs(3)).map(|s| s.time_ms),
            Some(0)
        );
        early.forget();
        assert!(early.before(t0).is_none());
    }

    #[test]
    fn a_report_writes_its_files_and_a_cancelled_one_is_deleted() {
        let dir = std::env::temp_dir().join(format!("thedas-report-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut r = Recorder::new(&dir);
        let t0 = Instant::now();
        r.keep(snap(t0, 1234));
        r.input("key i");
        r.frame(16.0);
        r.frame(33.0);
        keep_line("a line for the report".into());
        let info = json!({ "game": { "area": "lot100ar/Lothering" } });
        let trace = json!([{ "kind": "log", "t": 5, "channel": 0, "text": "hello" }]);
        let path = r.start(Some(b"{\"now\":1}".to_vec()), Some(trace), info);
        assert!(r.busy());
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("lot100ar_Lothering")
        );
        for f in [
            "report.json",
            "save_now.json",
            "save_before.json",
            "log.txt",
            "input.txt",
        ] {
            assert!(path.join(f).is_file(), "{f}");
        }
        let info: J =
            serde_json::from_slice(&std::fs::read(path.join("report.json")).unwrap()).unwrap();
        assert_eq!(info["snapshots"]["before"]["time_ms"], 1234);
        assert_eq!(info["frames"]["count"], 2);
        assert_eq!(info["frames"]["max_ms"], 33.0);
        let log = std::fs::read_to_string(path.join("log.txt")).unwrap();
        assert!(log.contains("a line for the report"));
        let input = std::fs::read_to_string(path.join("input.txt")).unwrap();
        assert!(input.contains("key i"));
        assert_eq!(
            std::fs::read(path.join("save_before.json")).unwrap(),
            b"{\"t\":1234}"
        );
        // Not done until both the screenshot and the note are in.
        r.note(Some("the door would not open".into()));
        assert!(r.finish().is_none());
        r.screenshot(Ok(b"png".to_vec()));
        let done = r.finish().unwrap();
        assert_eq!(done.as_deref(), Some(path.as_path()));
        assert!(!r.busy());
        assert_eq!(
            std::fs::read_to_string(path.join("note.txt")).unwrap(),
            "the door would not open"
        );
        // A second report the same minute gets a folder of its own; cancelling deletes it.
        let info = json!({ "game": { "area": "lot100ar/Lothering" } });
        let second = r.start(None, None, info);
        assert_ne!(second, path);
        r.screenshot(Err(anyhow::anyhow!("no frame")));
        r.note(None);
        assert_eq!(r.finish(), Some(None));
        assert!(!second.exists());
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_names_are_safe() {
        assert_eq!(
            sanitize("lot100ar/Lothering: inn"),
            "lot100ar_Lothering__inn"
        );
        assert_eq!(sanitize(""), "none");
        assert_eq!(sanitize(&"x".repeat(100)).len(), 40);
    }
}
