//! The pad's focus on screens: the D-pad and left stick move from button to button (the
//! clips that take a click, as `thedas_ui::FocusTarget`s), the pointer rests on the one
//! focused so its own rollover highlight shows, and A clicks it.
//!
//! A deliberate addition, like the rest of the pad (see [`crate::pad`]): the PC movies'
//! console focus (FocusManager) is half-built (no first focus, dead ends), so the host picks
//! the next button by position instead. Provisional: the 360 version's own focus order is
//! not traced.

use thedas_ui::FocusTarget;

/// The target the pointer is on (the last that contains it: the topmost drawn).
pub fn under(targets: &[FocusTarget], (x, y): (f64, f64)) -> Option<usize> {
    targets
        .iter()
        .rposition(|t| x >= t.rect[0] && x <= t.rect[2] && y >= t.rect[1] && y <= t.rect[3])
}

/// The first target in reading order: the topmost row, then its leftmost.
pub fn first(targets: &[FocusTarget]) -> Option<usize> {
    (0..targets.len()).min_by(|&a, &b| {
        let (ax, ay) = targets[a].center();
        let (bx, by) = targets[b].center();
        let row = |y: f64| (y / 8.0).round();
        row(ay).total_cmp(&row(by)).then(ax.total_cmp(&bx))
    })
}

/// The last target in reading order: the bottom row, then its rightmost.
pub fn last(targets: &[FocusTarget]) -> Option<usize> {
    (0..targets.len()).max_by(|&a, &b| {
        let (ax, ay) = targets[a].center();
        let (bx, by) = targets[b].center();
        let row = |y: f64| (y / 8.0).round();
        row(ay).total_cmp(&row(by)).then(ax.total_cmp(&bx))
    })
}

/// The movie of the screens' tab bar (navbar.gfx), which LB and RB already work.
pub const TAB_BAR: &str = "navbar.gfx";

/// Where a screen's focus starts: its first button ([`first`]) outside the tab bar and other
/// than its Close or Back (B's), or the first of all when it has nothing else.
pub fn first_content(targets: &[FocusTarget]) -> Option<usize> {
    let content: Vec<usize> = (0..targets.len())
        .filter(|&i| targets[i].movie != TAB_BAR && !is_back(&targets[i]))
        .collect();
    let picked: Vec<FocusTarget> = content.iter().map(|&i| targets[i].clone()).collect();
    match first(&picked) {
        Some(j) => Some(content[j]),
        None => first(targets),
    }
}

/// How far `[a0, a1]` and `[b0, b1]` are apart (0 when they overlap).
fn gap(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    (b0 - a1).max(a0 - b1).max(0.0)
}

/// The target a step `(dx, dy)` (one of them ±1, the other 0) from `from` lands on: the
/// nearest whose centre lies that way, preferring those in line (overlapping across the
/// step). Past the last one, it wraps to the farthest in line the other way; nothing in
/// line, it stays.
pub fn step(targets: &[FocusTarget], from: usize, (dx, dy): (i8, i8)) -> usize {
    let Some(cur) = targets.get(from) else {
        return from;
    };
    let (cx, cy) = cur.center();
    let horizontal = dx != 0;
    let sign = if horizontal { dx } else { dy } as f64;
    // Along the step, and across it (0 when in line).
    let measure = |t: &FocusTarget| {
        let (tx, ty) = t.center();
        if horizontal {
            (
                (tx - cx) * sign,
                gap(cur.rect[1], cur.rect[3], t.rect[1], t.rect[3]),
            )
        } else {
            (
                (ty - cy) * sign,
                gap(cur.rect[0], cur.rect[2], t.rect[0], t.rect[2]),
            )
        }
    };
    let ahead = targets
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != from)
        .map(|(i, t)| (i, measure(t)))
        .filter(|&(_, (along, _))| along > 1.0)
        .min_by(|a, b| {
            let score = |(along, across): (f64, f64)| along + across * 3.0;
            score(a.1).total_cmp(&score(b.1))
        });
    if let Some((i, _)) = ahead {
        return i;
    }
    // Wrap: the farthest in line behind.
    targets
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != from)
        .map(|(i, t)| (i, measure(t)))
        .filter(|&(_, (along, across))| along < -1.0 && across == 0.0)
        .min_by(|a, b| a.1.0.total_cmp(&b.1.0))
        .map_or(from, |(i, _)| i)
}

/// Where the pointer rests on a target: its centre.
pub fn rest(t: &FocusTarget) -> (f64, f64) {
    t.center()
}

/// Whether a target's own name (the clip that takes the click, or the button holding its
/// hit area) says it goes back: Back, Previous, Cancel, Close or No.
pub fn is_back(t: &FocusTarget) -> bool {
    back_rank(t).is_some()
}

/// How a back button ranks for B: a way out of the screen (Close, Cancel, No) first, then a
/// step back within it (Back, Previous). The level-up wizard has both: B leaves it, as its
/// close button does, rather than paging back. Provisional: the 360 version's B is not
/// traced.
pub fn back_rank(t: &FocusTarget) -> Option<u8> {
    ["close", "cancel", "no"]
        .iter()
        .any(|w| names_it(t, w))
        .then_some(0)
        .or_else(|| {
            ["back", "previous", "prev"]
                .iter()
                .any(|w| names_it(t, w))
                .then_some(1)
        })
}

/// Whether a target's own name starts with the word `w` (then a capital, `_`, a digit or
/// nothing).
fn names_it(t: &FocusTarget, w: &str) -> bool {
    let mut parts = t.path.rsplit('.');
    let own = parts.next().unwrap_or("");
    let name = if own.starts_with("HitArea") || own.starts_with("hit") {
        parts.next().unwrap_or(own)
    } else {
        own
    };
    let lower = name.to_ascii_lowercase();
    lower.starts_with(w)
        && name[w.len()..]
            .chars()
            .next()
            .is_none_or(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(path: &str, x: f64, y: f64, w: f64, h: f64) -> FocusTarget {
        FocusTarget {
            movie: "m".into(),
            path: path.into(),
            rect: [x, y, x + w, y + h],
        }
    }

    /// The start menu's column of buttons.
    fn column() -> Vec<FocusTarget> {
        (0..5)
            .map(|i| {
                t(
                    &format!("b{i}"),
                    900.0,
                    300.0 + 32.0 * i as f64,
                    300.0,
                    26.0,
                )
            })
            .collect()
    }

    #[test]
    fn a_column_steps_down_and_up_and_wraps() {
        let c = column();
        assert_eq!(first(&c), Some(0));
        assert_eq!(last(&c), Some(4));
        assert_eq!(step(&c, 0, (0, 1)), 1);
        assert_eq!(step(&c, 3, (0, 1)), 4);
        assert_eq!(step(&c, 4, (0, 1)), 0);
        assert_eq!(step(&c, 0, (0, -1)), 4);
        assert_eq!(step(&c, 2, (0, -1)), 1);
        // Nothing to either side: it stays.
        assert_eq!(step(&c, 2, (1, 0)), 2);
        assert_eq!(step(&c, 2, (-1, 0)), 2);
    }

    #[test]
    fn a_grid_prefers_what_is_in_line() {
        // Two rows of three; a far button off to the right, out of line.
        let mut g = Vec::new();
        for r in 0..2 {
            for c in 0..3 {
                g.push(t(
                    &format!("r{r}c{c}"),
                    100.0 + 120.0 * c as f64,
                    100.0 + 80.0 * r as f64,
                    100.0,
                    60.0,
                ));
            }
        }
        g.push(t("far", 900.0, 400.0, 100.0, 60.0));
        assert_eq!(step(&g, 0, (1, 0)), 1);
        assert_eq!(step(&g, 1, (0, 1)), 4);
        assert_eq!(step(&g, 4, (0, -1)), 1);
        assert_eq!(step(&g, 5, (-1, 0)), 4);
        // From the row's end, right reaches the far one only when nothing is in line.
        assert_eq!(step(&g, 2, (1, 0)), 6);
        assert_eq!(first(&g), Some(0));
    }

    #[test]
    fn a_screens_focus_starts_below_its_tab_bar() {
        let mut tab = t("tab", 400.0, 0.0, 30.0, 30.0);
        tab.movie = TAB_BAR.into();
        let ts = vec![
            tab.clone(),
            t("_level0.CloseButton", 900.0, 60.0, 20.0, 20.0),
            t("slot", 200.0, 200.0, 50.0, 50.0),
        ];
        assert_eq!(first(&ts), Some(0));
        assert_eq!(first_content(&ts), Some(2));
        assert_eq!(first_content(&[tab]), Some(0));
    }

    #[test]
    fn the_pointer_finds_the_topmost_target_under_it() {
        let ts = vec![
            t("under", 0.0, 0.0, 100.0, 100.0),
            t("over", 40.0, 40.0, 20.0, 20.0),
        ];
        assert_eq!(under(&ts, (50.0, 50.0)), Some(1));
        assert_eq!(under(&ts, (10.0, 10.0)), Some(0));
        assert_eq!(under(&ts, (500.0, 10.0)), None);
    }

    #[test]
    fn back_buttons_are_known_by_name() {
        let named = |p: &str| is_back(&t(p, 0.0, 0.0, 1.0, 1.0));
        assert!(named("_level0.Foo.BackButton.HitArea_mc"));
        assert!(named("_level0.CancelButton_mc"));
        assert!(named("_level0.dlg.NoButton"));
        assert!(named("_level0.Close_mc"));
        assert!(named("_level0.nav.NextAndBackButtons_mc.previous_mc"));
        assert!(!named("_level0.nav.NextAndBackButtons_mc.next_mc"));
        assert!(!named("_level0.Background_mc"));
        assert!(!named("_level0.NormalButton"));
        assert!(!named("_level0.QuitButton.HitArea_mc"));
        assert!(!named("_level0.Notes_mc"));
    }
}
