//! Frame-rate figures for the window's title bar, so a slow spot can be measured rather than
//! described: the frame rate over the last window, the 1% low (the frame rate of the slowest
//! 1% of frames) and the 90th and 99th percentile frame times. Not in the original game; a
//! development aid that reads nothing from the simulation.

use std::time::{Duration, Instant};

/// How often the title is rewritten, and so the window the figures cover.
const WINDOW: Duration = Duration::from_secs(1);

/// Frame times gathered since the title was last written.
#[derive(Default)]
pub struct FrameStats {
    last: Option<Instant>,
    since: Option<Instant>,
    /// Frame times in milliseconds.
    times: Vec<f32>,
}

impl FrameStats {
    /// Notes a frame drawn at `now`; once a second gives the text for the title.
    pub fn frame(&mut self, now: Instant) -> Option<String> {
        if let Some(last) = self.last.replace(now) {
            self.times
                .push(now.saturating_duration_since(last).as_secs_f32() * 1000.0);
        }
        let since = *self.since.get_or_insert(now);
        if now.saturating_duration_since(since) < WINDOW || self.times.is_empty() {
            return None;
        }
        self.since = Some(now);
        let text = summary(&mut self.times);
        self.times.clear();
        text
    }
}

/// "60 fps, 1% low 31, p90 17.1 ms, p99 30.2 ms, max 41.0 ms" for these frame times
/// (milliseconds; sorted in place), or `None` for none.
fn summary(times: &mut [f32]) -> Option<String> {
    if times.is_empty() {
        return None;
    }
    times.sort_by(f32::total_cmp);
    let n = times.len();
    let total: f32 = times.iter().sum();
    let pct = |p: f32| times[(((n as f32) * p).ceil() as usize).clamp(1, n) - 1];
    // The 1% low: the average frame rate over the slowest 1% of frames (at least one).
    let worst = n.div_ceil(100);
    let worst_ms: f32 = times[n - worst..].iter().sum::<f32>() / worst as f32;
    let fps = |ms: f32| if ms > 0.0 { 1000.0 / ms } else { 0.0 };
    Some(format!(
        "{:.0} fps, 1% low {:.0}, p90 {:.1} ms, p99 {:.1} ms, max {:.1} ms",
        fps(total / n as f32),
        fps(worst_ms),
        pct(0.90),
        pct(0.99),
        times[n - 1],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_frames() {
        let mut t = vec![10.0; 100];
        assert_eq!(
            summary(&mut t).unwrap(),
            "100 fps, 1% low 100, p90 10.0 ms, p99 10.0 ms, max 10.0 ms"
        );
        assert!(summary(&mut []).is_none());
    }

    #[test]
    fn a_hitch_shows_in_the_low_and_the_max() {
        let mut t = vec![10.0; 99];
        t.push(100.0);
        let s = summary(&mut t).unwrap();
        assert!(s.contains("1% low 10,"), "{s}");
        assert!(s.contains("p90 10.0 ms"), "{s}");
        assert!(s.contains("max 100.0 ms"), "{s}");
    }

    #[test]
    fn written_once_a_window() {
        let mut f = FrameStats::default();
        let t0 = Instant::now();
        assert!(f.frame(t0).is_none());
        for i in 1..60 {
            assert!(f.frame(t0 + Duration::from_millis(i * 16)).is_none());
        }
        let s = f.frame(t0 + Duration::from_millis(1001)).unwrap();
        assert!(s.contains("fps"), "{s}");
        assert!(f.times.is_empty());
    }
}
