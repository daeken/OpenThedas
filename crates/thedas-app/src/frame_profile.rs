//! Opt-in wall-clock stages around live frame work, including waits inside each stage.
//!
//! Unlike the renderer's capture profile, these include simulation, scene sync and UI.
//! No GPU queries or extra submissions are made. Enable with `THEDAS_RENDER_PROFILE=1`.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const EVERY: u32 = 60;
type Stages = BTreeMap<&'static str, Duration>;
type Reports = BTreeMap<&'static str, (u32, Stages)>;

pub(crate) struct FrameProfile {
    scope: &'static str,
    last: Instant,
    stages: Stages,
}

impl FrameProfile {
    pub(crate) fn new(scope: &'static str) -> Option<Self> {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        if !*ENABLED.get_or_init(|| std::env::var_os("THEDAS_RENDER_PROFILE").is_some()) {
            return None;
        }
        Some(Self {
            scope,
            last: Instant::now(),
            stages: Stages::new(),
        })
    }

    pub(crate) fn mark(profile: &mut Option<Self>, stage: &'static str) {
        let Some(profile) = profile else { return };
        let now = Instant::now();
        *profile.stages.entry(stage).or_default() += now - profile.last;
        profile.last = now;
    }
}

impl Drop for FrameProfile {
    fn drop(&mut self) {
        static REPORTS: Mutex<Reports> = Mutex::new(BTreeMap::new());
        let mut reports = REPORTS.lock().unwrap_or_else(|p| p.into_inner());
        let (frames, stages) = reports.entry(self.scope).or_default();
        *frames += 1;
        for (stage, elapsed) in &self.stages {
            *stages.entry(stage).or_default() += *elapsed;
        }
        if *frames < EVERY {
            return;
        }
        for (stage, elapsed) in stages.iter() {
            log::info!(
                "frame-profile scope={} stage={stage} frames={frames} wall_ms={:.4}",
                self.scope,
                elapsed.as_secs_f64() * 1000.0 / f64::from(*frames)
            );
        }
        *frames = 0;
        stages.clear();
    }
}
