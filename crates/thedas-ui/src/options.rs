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
//! ClientOptions sets up (0x5e8735-0x5ec18d). Provisional: Cancel keeps what was set (the
//! executable's handlers, registered at 0x7946c0, are not traced) and nothing is written to the
//! player's settings file. The audio volumes and mutes, auto-pause in combat, the
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
];

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

/// The settings as optionsmenu.gfx reads them (`ClientOptions.<Group>.<Name>`), with the
/// resolutions it offers (`ClientOptions.ScreenResList`): the one the game runs at
/// (`resolution`, also as `VideoOptions.ResolutionWidth` and `ResolutionHeight`).
/// Provisional: the display's other modes are not listed.
pub fn values(
    options: &BTreeMap<String, f64>,
    resolution: (u32, u32),
    v: &mut BTreeMap<String, UiValue>,
) {
    for (k, d) in DEFAULTS {
        let x = options.get(*k).copied().unwrap_or(*d);
        v.insert(format!("ClientOptions.{k}"), UiValue::Number(x));
    }
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
        values(&o, (1280, 720), &mut v);
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
        values(&o, (1280, 720), &mut v);
        assert_eq!(
            v["ClientOptions.GameOptions.ShowGore"],
            UiValue::Number(1.0)
        );
    }
}
