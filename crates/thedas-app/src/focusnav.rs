//! The pad's focus on screens: the D-pad and left stick move from button to button (the
//! clips that take a click, as `thedas_ui::FocusTarget`s), the pointer rests on the one
//! focused so its own rollover highlight shows, and A clicks it.
//!
//! A deliberate addition, like the rest of the pad (see [`crate::pad`]): the PC movies'
//! console focus (FocusManager) is half-built (no first focus, dead ends), so the host picks
//! the next button by position instead. Provisional: the 360 version's own focus order is
//! not traced.

use thedas_ui::FocusTarget;

/// The target the pointer is on: the smallest that contains it (the last of equals, the
/// topmost drawn). An open picker's full-screen overlay (which closes it on a click outside)
/// contains its entries' places too, and is not the one focused there.
pub fn under(targets: &[FocusTarget], (x, y): (f64, f64)) -> Option<usize> {
    let area = |t: &FocusTarget| (t.rect[2] - t.rect[0]) * (t.rect[3] - t.rect[1]);
    targets
        .iter()
        .enumerate()
        .filter(|(_, t)| x >= t.rect[0] && x <= t.rect[2] && y >= t.rect[1] && y <= t.rect[3])
        .min_by(|a, b| area(a.1).total_cmp(&area(b.1)).then(b.0.cmp(&a.0)))
        .map(|(i, _)| i)
}

/// The targets the focus may move among: not the drop areas, and with a picker's list open (its entries are up,
/// `PickerList_mc`), only that list's entries, since the list is modal over the screen
/// (its overlay closes it on a click outside) and the buttons under it are not to be had.
pub fn modal(targets: Vec<FocusTarget>) -> Vec<FocusTarget> {
    const LIST: &str = ".PickerList_mc.";
    // Areas that only take a dragged item (the inventory's list as a whole, its destroy
    // icon) are not buttons to step onto.
    let targets: Vec<FocusTarget> = targets
        .into_iter()
        .filter(|t| !t.path.contains("DropArea") && !t.path.contains("DropTarget"))
        // Nor a list's scrollbar (its track, thumb and arrows): the right stick scrolls a
        // list, and in the store the scrollbar took the step meant for the items beside it.
        .filter(|t| !t.path.contains(".Scrollbar_mc."))
        // Nor is the inventory's paperdoll (an unnamed clip of the stage, dragged to turn
        // the figure): it sat between the two columns of equipment slots and took the
        // step from one to the other.
        .filter(|t| {
            t.path
                .strip_prefix("_level0.instance")
                .is_none_or(|n| !n.chars().all(|c| c.is_ascii_digit()))
        })
        .collect();
    if targets.iter().any(|t| t.path.contains(LIST)) {
        return targets
            .into_iter()
            .filter(|t| t.path.contains(LIST))
            .collect();
    }
    // A dialog box (popuplayer.gfx's, known by its row of buttons) is modal over
    // everything else its movie shows: the tutorial panel beside it is not to be had while
    // the box is up (its Next sat above a tall box's buttons and took the first focus).
    const BOX_BUTTONS: &str = ".Buttons_mc.Button";
    let root = targets
        .iter()
        .find(|t| t.movie == POPUP_MOVIE && t.path.contains(BOX_BUTTONS))
        .and_then(|t| {
            t.path
                .find(BOX_BUTTONS)
                .map(|i| format!("{}.", &t.path[..i]))
        });
    match root {
        Some(root) => targets
            .into_iter()
            .filter(|t| t.movie == POPUP_MOVIE && t.path.starts_with(&root))
            .collect(),
        None => targets,
    }
}

/// The movie of the game's dialog boxes and tutorial panel.
const POPUP_MOVIE: &str = "popuplayer.gfx";

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

/// Where a screen's focus starts: the first entry of its item list when it has one, else its
/// first button ([`first`]) outside the tab bar and other than its Close or Back (B's), or
/// the first of all when it has nothing else.
pub fn first_content(targets: &[FocusTarget]) -> Option<usize> {
    // A list of items (the inventory's, a store's) is what the screen is for: the focus
    // starts on its first entry, not on the sort picker or the tabs above it.
    let entries: Vec<usize> = (0..targets.len())
        .filter(|&i| targets[i].path.contains(".ItemEntry"))
        .collect();
    if !entries.is_empty() {
        let picked: Vec<FocusTarget> = entries.iter().map(|&i| targets[i].clone()).collect();
        return first(&picked).map(|j| entries[j]);
    }
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
/// step, or nearly). Past the last one, it wraps to the farthest in line the other way; nothing in
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
    // Nearly in line counts as in line: a button a little above or below the row (the
    // tactics screen's two pickers, 17 px apart in height) is still the one beside it, rather
    // than a nearer button a row away.
    // Only for a button of the same size (a sibling control): a different one near the row
    // (the inventory's category icons above its list) is not beside it.
    let size = |t: &FocusTarget| (t.rect[2] - t.rect[0], t.rect[3] - t.rect[1]);
    let (cw, ch) = size(cur);
    let slack = |t: &FocusTarget| {
        let (w, h) = size(t);
        let alike = (w - cw).abs() <= cw * 0.2 && (h - ch).abs() <= ch * 0.2;
        if alike { cw.min(ch) * 0.75 } else { 0.0 }
    };
    let ahead = targets
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != from)
        .map(|(i, t)| (i, measure(t), slack(t)))
        .filter(|&(_, (along, _), _)| along > 1.0)
        .min_by(|a, b| {
            let score =
                |(along, across): (f64, f64), slack: f64| (across > slack, along + across * 3.0);
            let (sa, sb) = (score(a.1, a.2), score(b.1, b.2));
            sa.0.cmp(&sb.0).then(sa.1.total_cmp(&sb.1))
        })
        .map(|(i, m, _)| (i, m));
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

    #[test]
    fn a_dialog_box_is_modal_over_the_tutorial_panel() {
        let p = |path: &str, y: f64| FocusTarget {
            movie: POPUP_MOVIE.into(),
            path: path.into(),
            rect: [500.0, y, 700.0, y + 30.0],
        };
        let next = "_level0.TutorialContainer_mc.Fade_mc.Tutorial_mc.TutorialNextOKButton_mc";
        let targets = vec![
            // The tutorial's Next sits above the box's buttons.
            p(next, 300.0),
            p("_level0.null.Buttons_mc.Button1_mc", 420.0),
            p("_level0.null.Buttons_mc.Button2_mc", 420.0),
            p("_level0.null.TextInput_mc", 380.0),
            t("_level0.Other_mc", 0.0, 0.0, 10.0, 10.0),
        ];
        let kept: Vec<String> = modal(targets).into_iter().map(|t| t.path).collect();
        assert_eq!(
            kept,
            [
                "_level0.null.Buttons_mc.Button1_mc",
                "_level0.null.Buttons_mc.Button2_mc",
                "_level0.null.TextInput_mc"
            ]
        );
        // Without a box the tutorial's button is a target like any other.
        assert_eq!(modal(vec![p(next, 300.0)]).len(), 1);
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
        // A button a little out of line (the tactics screen's Presets, lower than its
        // Behaviour picker) is still the one beside it, not a nearer button a row away.
        let pickers = [
            t("behaviour", 668.0, 100.0, 230.0, 28.0),
            t("preset", 256.0, 145.0, 230.0, 28.0),
            t("row_action", 529.0, 183.0, 340.0, 28.0),
        ];
        assert_eq!(step(&pickers, 0, (-1, 0)), 1);
        // An open picker's list keeps the focus to its entries.
        let open = vec![
            t("s.Row.Button", 150.0, 183.0, 340.0, 28.0),
            t("s.P.PickerList_mc.E1.HitArea_mc", 258.0, 173.0, 225.0, 32.0),
            t("s.P.PickerList_mc.E2.HitArea_mc", 258.0, 205.0, 225.0, 32.0),
            t("s.P.Overlay_mc", -170.0, -3.0, 1365.0, 768.0),
        ];
        let entries = modal(open.clone());
        assert_eq!(entries.len(), 2);
        assert_eq!(step(&entries, 0, (0, 1)), 1);
        assert_eq!(under(&open, (300.0, 190.0)), Some(1));
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
    fn a_lists_scrollbar_is_not_a_focus_target() {
        let targets = vec![
            t(
                "_level0.StorePage_mc.StoreItemWindow_mc.Scrollbar_mc.Slider_mc.Thumb_mc.HitArea_mc",
                600.0,
                200.0,
                10.0,
                40.0,
            ),
            t(
                "_level0.StorePage_mc.StoreItemWindow_mc.Scrollbar_mc.UpArrow_mc.HitArea_mc",
                600.0,
                180.0,
                10.0,
                10.0,
            ),
            t(
                "_level0.StorePage_mc.StoreItemWindow_mc.Scrollable_mc.StoreItemEntry1.HitArea_mc",
                300.0,
                200.0,
                280.0,
                40.0,
            ),
        ];
        let kept = modal(targets);
        assert_eq!(kept.len(), 1);
        assert!(kept[0].path.ends_with("StoreItemEntry1.HitArea_mc"));
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
