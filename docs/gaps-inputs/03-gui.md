# 03: GUI movies

Gathered 2026-11-12 on main 1eb844a. Sources: `thedas ls` over every .erf and .rim in the install, a grep of
crates/thedas-ui and crates/thedas-app, and headless `thedas-app` runs (`--capture`, `--no-audio`,
`RUST_LOG=info,thedas_app::ui=debug`). No game text.

## Movies in the install

68 distinct `.gfx` names. They come from packages/core/data/guiexport.erf (67), patch002.erf (68), patch003.erf (57),
patch000/001.erf (chargen, optionsmenu) and chargen.rim (chargen, conversation, load_light). No addin or DLC
archive ships a `.gfx`.

## How unanswered values were counted

`thedas-ui` keeps one unanswered map for all open movies. It holds GetValue/GetNumber/GetString/GetBool keys that
nothing has set, unresolved table queries, and returning ExternalCommands/fscommands. The app logs the map as
"N engine values unanswered". Counts are per screen: the keys a run adds over the HUD baseline on the same save
(dc_17_next unless noted). Keys that the HUD already asked for are only counted once, under the HUD. Inferred: the
counts depend on the save, and indexed keys (quickslot N, party member N) change with party size.
In every run all the unanswered names were dotted engine-value keys, so no ExternalCommand went unanswered.

## (a) Opened and wired (Screen::movies in crates/thedas-app/src/screens.rs), with unanswered keys

| movie | screen | unanswered (new over HUD) |
| --- | --- | --- |
| floatylayer, portraits, quickbar, minimap, notifications, navbar, tooltips, popuplayer | HUD | 220 for the whole HUD (below) |
| inventory | Inventory (I) | 42: PartyBackpackView.N.AbilityInstance.{,CooldownPercentage,PendingActivation} ×12 each, Equipment.N.StackSize, Party.BackpackSort, Party.Approval, SelectedChar.CanChangeEquipment, SelectedChar.CanSwitchWeaponSets, GUIBookback.ShowContents |
| characterrecord | Character record (C) | 2: Party.Approval, GUIBookback.ShowContents |
| tactics | Tactics (T) | 1: GUIBookback.ShowContents |
| map | Area map (M) | 14: 11 GUITooltips.* layout keys, Journal.PlotDestinations, Journal.PlotGivers, GUIBookback.ShowContents |
| journal | Journal (J) | 5: GUIJournal.LastSelectedCodex, LastSelectedCodexGroup, Journal.PlotList.N.OfferId, ContentManager.DlcEnabled, GUIBookback.ShowContents |
| abilities | Skills (K) | 98: SkillChains.N.AbilityInstance.{CooldownPercentage,PendingActivation,Status} ×32 each, GUI.SelectedChar.Tag, GUIBookback.ShowContents |
| abilities | Spells/talents (P) | 170: TalentChains.N.AbilityInstance.{CooldownPercentage,PendingActivation,Status} ×56 each, GUI.SelectedChar.Tag, GUIBookback.ShowContents |
| optionsmenu | Options (O) | 0 |
| saveload | Save/Load (F5/F9) | 0 |
| partypicker | Party picker | 3: PartyPicker.SelectedPartyMember.N.Portrait |
| deathscreen | Death (party wipe) | 0 |
| store | Store (save dc_02b_store; HUD not up) | 4 in all: GUI.TooltipText, GUITooltips.TooltipText, GameModeExplore.PendingTargetID, GUIBookback.ShowContents |
| conversation | Conversation (save dc_03_tavern, talk) | 0 from the conversation itself |
| cutscene | Cutscene | 0 |
| startmenu2 | Start menu (no save, --no-intro) | 2 in all: GUI.TooltipText, SaveLoad.HeroList |
| chargen | Character creation | not measured: the headless `--new-game` run went straight to the cutscene and never showed the chargen screen |
| levelup, chanters, worldmap, itemexamine, mainmenu (game menu) | wired screens | not measured: no headless step opens them directly |
| gfxfontlib, fonts_en-us | font libraries, imported (not opened as screens) | n/a |
| bookback | never opened as a movie; thedas-ui draws its BookBack_I3.dds image under the book screens instead | GUIBookback.ShowContents is unanswered on every book screen |

GUIBookback.ShowContents is unanswered on every book screen, because the bookback movie itself never runs.

## HUD engine keys still unanswered

Headless capture, save dc_17_next.json, `--steps "run 500"`: **220 engine values unanswered**. Grouped:

- GUI.SelectedChar.Stats.QuickSlots.N.{CooldownPercentage, ItemIcon, PendingActivation, SourceItem.StackSize, Status}: 16 slots × 5 = 80, plus QuickSlots.undefined
- InputMappings.Mappings.ActivateQuickslot11..28.PrimaryMappedKey (18) and TogglePartyPicker.PrimaryMappedKey
- Party.Members.N.*: AbilityUpkeepCost, Attributes.N.Current/Total, CurrentAction, CurrentActionIcon, ID, Paralyzed, PendingAction, PendingActionIcon, Portrait, PortraitLevelUp, PortraitProperties.PortraitTint, Stats.ClassHasMana, Stats.ClassName, UserSelected (about 60)
- Party.Summons.N.* (the same fields for summons, about 60)
- Party.HoldPositions, GUI.SelectedChar.EffectList, GUI.SelectedBookPage, GUI.TooltipText, GUITooltips.TooltipText,
  GameModeExplore.PendingTargetID, DebugOptions.SuppressPartyActionDisplay, undefined.QuestCompleted

Some of these may be slots the HUD probes when nothing fills them (empty party slots, summons). This is inferred, not checked against the exe.

## (c) Never opened (37)

68 movies: 28 opened as screens, 2 font libraries imported, bookback drawn as an image, and these 37 never opened.

No reference anywhere in thedas-ui or thedas-app:
- Gameplay screens: armycontrol, crafting, itemupgrade, slideshow, credits, dialoguepop, fademap, bookfront, newgame, savescreen.
  Matching engine functions are also missing from the sim: ShowCraftingGUI, OpenItemUpgradeGUI, BeginSlideshow (see 01).
- Loading screens: load_fade, load_light, load_town.
- Online and DLC: login, loginconnected, loginconnecting, loginnewaccount, loginviewtos, prccontentmanager,
  purchaseconfirm, purchasingcontent, contentinstalling, guicommunityfeatures, guiachievementunlocked, guiscreenshots.
- Debug: debug, debugoptions (DebugOptions.* keys are referenced in thedas-ui, but the movie is never opened), fonttest.
- Other locales' fonts: fonts_cs-cz, de-de, es-es, fr-fr, hu-hu, it-it, pl-pl, pseudo, ru-ru. Only fonts_en-us is wired (`FONT_LIBRARY`).
