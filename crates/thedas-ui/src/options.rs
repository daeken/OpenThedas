//! The player's settings (`ClientOptions.*`), as the options screen (optionsmenu.gfx) reads
//! and changes them. They belong to the player, not to the game: the interface keeps them
//! ([`crate::binding::Presentation::options`]), except the difficulty, which the simulation
//! keeps (`GetGameDifficulty` reads it; see `thedas_sim::chargen::MSG_CLIENT_OPTION`).
//!
//! optionsmenu.gfx reads each option as `ClientOptions.<Group>.<Name>` and sends
//! `ClientOptions.SetClientOptionMessage <Group>.<Name> <value>` as soon as the player
//! changes it (its resolution revert sends the old values back the same way, so a change
//! takes effect when it is set), `ClientOptions.CommitOptions` from OK and
//! `ClientOptions.RestoreDefaults` from its button. The defaults are those DAOrigins.exe's
//! ClientOptions sets up (0x5e8735-0x5ec18d). Provisional: The app drops what was set when the
//! screen is cancelled and keeps the settings in the player's settings file ([`to_text`]),
//! written when the screen's OK commits them (the executable's handlers, registered at
//! 0x7946c0, are not traced). The audio volumes and mutes, auto-pause in combat, the
//! tutorials, mouse sensitivity and the inverted axes take effect; the other settings change nothing yet but what the screen shows.

use std::collections::BTreeMap;

use crate::UiValue;

/// A setting changed: its name (`GameOptions.ShowGore`) and value.
pub const MSG_SET: &str = "ClientOptions.SetClientOptionMessage";
/// Every setting back to its default.
pub const MSG_RESTORE_DEFAULTS: &str = "ClientOptions.RestoreDefaults";
/// The setting the simulation keeps.
pub const DIFFICULTY: &str = "GameOptions.DifficultyLevel";

/// The settings optionsmenu.gfx shows and their defaults, from DAOrigins.exe's ClientOptions
/// (0x5e8735-0x5ec18d; the group names are the movie's). `ScreenResIndex` is the place of
/// the resolution in `ScreenResList`.
pub const DEFAULTS: &[(&str, f64)] = &[
    ("GameOptions.SubtitlesLevel", 0.0),
    ("GameOptions.ShowGore", 1.0),
    ("GameOptions.ShowTutorials", 1.0),
    ("GameOptions.PlotAssist", 1.0),
    ("GameOptions.AutoPauseCombat", 0.0),
    ("GameOptions.AutoSave", 1.0),
    ("GameOptions.ShowPartyDamageNumbers", 1.0),
    ("GameOptions.ShowHostileDamageNumbers", 1.0),
    ("GameOptions.ShowSpecialMoveFloaties", 1.0),
    ("GameOptions.LootShimmer", 1.0),
    // Not among the executable's defaults found; off. Provisional.
    ("GameOptions.RevealPlotHelp", 0.0),
    ("ControlOptions.InvertXAxis", 0.0),
    ("ControlOptions.InvertYAxis", 0.0),
    ("ControlOptions.SwapMouseButtons", 0.0),
    ("ControlOptions.MouseSensitivity", 1.0),
    ("ControlOptions.MouseZoomSensitivity", 2.0),
    ("AudioOptions.SFXDisabled", 0.0),
    ("AudioOptions.DialogDisabled", 0.0),
    ("AudioOptions.MusicDisabled", 0.0),
    ("AudioOptions.UseStereoSound", 0.0),
    ("AudioOptions.SoundFXVolume", 10.0),
    ("AudioOptions.DialogVolume", 10.0),
    ("AudioOptions.MusicVolume", 10.0),
    ("VideoOptions.FullScreen", 1.0),
    ("VideoOptions.UseVSync", 0.0),
    ("VideoOptions.Gamma", 0.5),
    ("VideoOptions.TexturePack", 1.0),
    ("VideoOptions.EnableFrameBufferEffects", 1.0),
    ("VideoOptions.GraphicsDetailLevel", 3.0),
    ("VideoOptions.ForceLowSettings", 0.0),
    // Off: there are no online features here (the bridge seeds it 0 and nobody is logged
    // in). A deliberate change from the exe's default.
    ("OnlineOptions.EnableOnlineFeatures", 0.0),
    ("OnlineOptions.EnableOnlineTelemetry", 1.0),
    ("OnlineOptions.AutoDownloadOldEntitlements", 0.0),
    ("OnlineOptions.AutoScreenshots", 1.0),
    ("OnlineOptions.AutoScreenshotsNotification", 1.0),
    ("OnlineOptions.AutoUploadScreenshot", 0.0),
    ("OnlineOptions.AutoUploadProfile", 1.0),
    ("ScreenResIndex", 0.0),
    // The place of the anti-aliasing choice in `AntialiasList`; its default depends on the
    // list (see [`default_antialias_index`]), so this 0 (off) is only the list's first.
    (ANTIALIAS_INDEX, 0.0),
];

/// The anti-aliasing slider's setting (`ClientOptions.AntialiasIndex`, no group), and its list
/// (`ClientOptions.AntialiasList`).
pub const ANTIALIAS_INDEX: &str = "AntialiasIndex";
/// The settings file's section for [`ANTIALIAS_INDEX`] (the original keeps the choice as
/// `AntialiasingLevel` among its video settings, exe 0xb0100c).
const ANTIALIAS_SECTION: &str = "VideoOptions";

/// `AntialiasList`: 0 (shown "Off") then each sample count the device draws with but 1. The
/// movie shows a member of 0 as its zero label and any other as "<n>x", and the setting is
/// the member's place (optionsmenu.gfx `SliderPickerLabelsChanged`; the exe's entries,
/// 0x5def4f, are the device's multisample types and qualities).
pub fn antialias_list(samples: &[u32]) -> Vec<u32> {
    let mut list = vec![0];
    list.extend(samples.iter().copied().filter(|&n| n > 1));
    list
}

/// The place in `list` ([`antialias_list`]) the setting starts at: with `on` (a player's
/// window) the highest count up to 4x, else off. Not the exe's default (untraced): 4x smooths
/// the jagged edges the original's post-process smoothing leaves. Automated runs (tests, QA's
/// captures) start off, as before, so their pictures and timings stay as they were.
pub fn default_antialias_index(list: &[u32], on: bool) -> usize {
    if !on {
        return 0;
    }
    list.iter()
        .rposition(|&n| (2..=4).contains(&n))
        .unwrap_or(0)
}

/// The sample count the settings choose (1: off) among those the device draws with.
pub fn antialiasing(options: &BTreeMap<String, f64>, samples: &[u32], default_on: bool) -> u32 {
    let list = antialias_list(samples);
    let at = options.get(ANTIALIAS_INDEX).map_or_else(
        || default_antialias_index(&list, default_on),
        |v| v.max(0.0) as usize,
    );
    list.get(at).copied().unwrap_or(0).max(1)
}

/// Takes a settings message into `options` (the settings changed from their defaults);
/// returns whether it was one. The difficulty is left to the simulation.
pub fn option_message(options: &mut BTreeMap<String, f64>, name: &str, args: &[String]) -> bool {
    match name {
        MSG_SET => {
            let Some(key) = args.first().map(|a| a.trim()) else {
                return false;
            };
            if key == DIFFICULTY || !DEFAULTS.iter().any(|(k, _)| *k == key) {
                return false;
            }
            let value = match args.get(1).map(|a| a.trim()) {
                Some("true") => 1.0,
                Some("false") => 0.0,
                Some(v) => match v.parse::<f64>() {
                    Ok(v) if v.is_finite() => v,
                    _ => return true,
                },
                None => return true,
            };
            options.insert(key.to_string(), value);
            true
        }
        MSG_RESTORE_DEFAULTS => {
            options.clear();
            true
        }
        _ => false,
    }
}

/// The player's settings file, in the form of the original's own (`DragonAge.ini` in
/// `BioWare\Dragon Age\Settings\` under the player's documents, exe 0xb01458): a
/// `[Group]` section per group with `Name=value` lines, here only for the settings changed
/// from their defaults (the difficulty is the game's, and `ScreenResIndex` follows the
/// window, so neither is written). Provisional: when the original writes it (OK, or each
/// change) is not traced.
pub fn to_text(options: &BTreeMap<String, f64>) -> String {
    let mut out = String::new();
    let mut group = "";
    // The anti-aliasing choice is the one setting with no group; it is kept with the video
    // settings.
    let grouped: BTreeMap<String, f64> = options
        .iter()
        .map(|(k, v)| {
            let key = if k == ANTIALIAS_INDEX {
                format!("{ANTIALIAS_SECTION}.{k}")
            } else {
                k.clone()
            };
            (key, *v)
        })
        .collect();
    for (k, v) in &grouped {
        let Some((g, name)) = k.split_once('.') else {
            continue;
        };
        let known = DEFAULTS.iter().any(|(d, _)| {
            d == k || format!("{ANTIALIAS_SECTION}.{d}") == *k && *d == ANTIALIAS_INDEX
        });
        if k == DIFFICULTY || !known || !v.is_finite() {
            continue;
        }
        if g != group {
            out.push_str(&format!("[{g}]\n"));
            group = g;
        }
        out.push_str(&format!("{name}={v}\n"));
    }
    out
}

/// Reads a settings file ([`to_text`]): the settings it names that the options screen
/// keeps, with finite values; anything else is skipped.
pub fn from_text(text: &str) -> BTreeMap<String, f64> {
    let mut group = String::new();
    let mut out = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if let Some(g) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            group = g.trim().to_string();
            continue;
        }
        let Some((name, v)) = line.split_once('=') else {
            continue;
        };
        let mut key = format!("{group}.{}", name.trim());
        if key == format!("{ANTIALIAS_SECTION}.{ANTIALIAS_INDEX}") {
            key = ANTIALIAS_INDEX.to_string();
        }
        let Ok(v) = v.trim().parse::<f64>() else {
            continue;
        };
        if key != DIFFICULTY && v.is_finite() && DEFAULTS.iter().any(|(d, _)| *d == key) {
            out.insert(key, v);
        }
    }
    out
}

/// The settings as optionsmenu.gfx reads them (`ClientOptions.<Group>.<Name>`), with the
/// resolutions it offers (`ClientOptions.ScreenResList`): the one the game runs at
/// (`resolution`, also as `VideoOptions.ResolutionWidth` and `ResolutionHeight`).
/// Provisional: the display's other modes are not listed.
pub fn values(
    options: &BTreeMap<String, f64>,
    resolution: (u32, u32),
    (samples, default_on): (&[u32], bool),
    v: &mut BTreeMap<String, UiValue>,
) {
    for (k, d) in DEFAULTS {
        let x = options.get(*k).copied().unwrap_or(*d);
        v.insert(format!("ClientOptions.{k}"), UiValue::Number(x));
    }
    let list = antialias_list(samples);
    for (i, n) in list.iter().enumerate() {
        v.insert(
            format!("ClientOptions.AntialiasList.{i}"),
            UiValue::Number(f64::from(*n)),
        );
    }
    let at = options
        .get(ANTIALIAS_INDEX)
        .copied()
        .unwrap_or_else(|| default_antialias_index(&list, default_on) as f64);
    v.insert(
        format!("ClientOptions.{ANTIALIAS_INDEX}"),
        UiValue::Number(at),
    );
    v.insert(
        "ClientOptions.VideoOptions.ResolutionWidth".into(),
        UiValue::Number(resolution.0 as f64),
    );
    v.insert(
        "ClientOptions.VideoOptions.ResolutionHeight".into(),
        UiValue::Number(resolution.1 as f64),
    );
    v.insert(
        "ClientOptions.ScreenResList.0".into(),
        UiValue::String(format!("{} x {}", resolution.0, resolution.1)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_file_keeps_known_settings_and_skips_the_rest() {
        let mut o = BTreeMap::new();
        o.insert("GameOptions.SubtitlesLevel".to_string(), 1.0);
        o.insert("AudioOptions.MusicVolume".to_string(), 4.5);
        o.insert(DIFFICULTY.to_string(), 3.0);
        let text = to_text(&o);
        assert_eq!(
            text,
            "[AudioOptions]\nMusicVolume=4.5\n[GameOptions]\nSubtitlesLevel=1\n"
        );
        let back = from_text(&format!(
            "{text}garbage\n[Nonsense]\nOption=1\n[GameOptions]\nShowGore=x\nAutoSave=NaN\nDifficultyLevel=2\nLocale=en-us\n"
        ));
        assert_eq!(back.len(), 2);
        assert_eq!(back["GameOptions.SubtitlesLevel"], 1.0);
        assert_eq!(back["AudioOptions.MusicVolume"], 4.5);
        assert!(from_text("").is_empty());
    }

    #[test]
    fn anti_aliasing_lists_the_device_counts_and_defaults_to_four() {
        // Off, then each count the device draws with.
        assert_eq!(antialias_list(&[1, 2, 4, 8]), vec![0, 2, 4, 8]);
        assert_eq!(antialias_list(&[1]), vec![0]);
        let mut o = BTreeMap::new();
        // Unset: the highest count up to 4x; a device with only 2x gets 2x; none, off.
        assert_eq!(antialiasing(&o, &[1, 2, 4, 8], true), 4);
        assert_eq!(antialiasing(&o, &[1, 2], true), 2);
        assert_eq!(antialiasing(&o, &[1], true), 1);
        // Automated runs start off.
        assert_eq!(antialiasing(&o, &[1, 2, 4, 8], false), 1);
        // A choice wins, Off included; one past the list is off.
        o.insert(ANTIALIAS_INDEX.to_string(), 0.0);
        assert_eq!(antialiasing(&o, &[1, 2, 4], true), 1);
        o.insert(ANTIALIAS_INDEX.to_string(), 3.0);
        assert_eq!(antialiasing(&o, &[1, 2, 4, 8], true), 8);
        o.insert(ANTIALIAS_INDEX.to_string(), 9.0);
        assert_eq!(antialiasing(&o, &[1, 2, 4], true), 1);
        // The screen reads the list and the place; the settings file keeps the place with
        // the video settings.
        let mut v = BTreeMap::new();
        o.insert(ANTIALIAS_INDEX.to_string(), 1.0);
        values(&o, (1280, 720), (&[1, 2, 4], true), &mut v);
        assert_eq!(v["ClientOptions.AntialiasList.0"], UiValue::Number(0.0));
        assert_eq!(v["ClientOptions.AntialiasList.2"], UiValue::Number(4.0));
        assert_eq!(v["ClientOptions.AntialiasIndex"], UiValue::Number(1.0));
        let text = to_text(&o);
        assert_eq!(text, "[VideoOptions]\nAntialiasIndex=1\n");
        assert_eq!(from_text(&text)[ANTIALIAS_INDEX], 1.0);
        let mut v = BTreeMap::new();
        values(&BTreeMap::new(), (1280, 720), (&[1, 2, 4], true), &mut v);
        assert_eq!(v["ClientOptions.AntialiasIndex"], UiValue::Number(2.0));
    }

    #[test]
    fn settings_change_restore_and_leave_the_difficulty_to_the_game() {
        let mut o = BTreeMap::new();
        let a = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(option_message(
            &mut o,
            MSG_SET,
            &a(&["GameOptions.ShowGore", "0"])
        ));
        assert!(option_message(
            &mut o,
            MSG_SET,
            &a(&["AudioOptions.MusicVolume", "4.5"])
        ));
        assert!(!option_message(&mut o, MSG_SET, &a(&[DIFFICULTY, "3"])));
        assert!(!option_message(
            &mut o,
            MSG_SET,
            &a(&["Nonsense.Option", "1"])
        ));
        let mut v = BTreeMap::new();
        values(&o, (1280, 720), (&[1, 2, 4], true), &mut v);
        assert_eq!(
            v["ClientOptions.GameOptions.ShowGore"],
            UiValue::Number(0.0)
        );
        assert_eq!(
            v["ClientOptions.AudioOptions.MusicVolume"],
            UiValue::Number(4.5)
        );
        assert_eq!(
            v["ClientOptions.GameOptions.AutoSave"],
            UiValue::Number(1.0)
        );
        assert_eq!(
            v["ClientOptions.ScreenResList.0"],
            UiValue::String("1280 x 720".into())
        );
        assert!(!v.contains_key(&format!("ClientOptions.{DIFFICULTY}")));
        assert!(option_message(&mut o, MSG_RESTORE_DEFAULTS, &[]));
        let mut v = BTreeMap::new();
        values(&o, (1280, 720), (&[1, 2, 4], true), &mut v);
        assert_eq!(
            v["ClientOptions.GameOptions.ShowGore"],
            UiValue::Number(1.0)
        );
    }
}
