# Gaps: UI, HUD, input and controller (Input/UI stream)

Kept current in place by the Input/UI thread. Last pass: 2026-11-12 16:10Z, main at #315.

Sizes: S = under a day, M = a few days, L = a week or more.

How the list was built:
- The movies in `guiexport.erf` and the patches, compared with the screens we load (`crates/thedas-app/src/screens.rs`).
- `keymap.gda` actions, compared with the keys `main.rs` handles.
- A headless run of `dc_13_hideout_fight.json` that opened I, C, M, J, T, K and P with `RUST_LOG=thedas_app::ui=debug`. It logged 73 distinct engine values the movies asked for and got no answer (log in `OpenThedas-testdata/ui/gaps/log.txt`). Some are benign: party slots 2 and 3 in a two-member party, empty quickbar slots, and chain entries the screen asks about before filtering. The quickbar's cooldowns, status and item icons are fed for filled slots; that was checked against `binding.rs`.
- `handoff-ui.md`.

## 1. Screens never built (the movie exists, we never load it)

| Item | What the original does | What we do | Size | Depends on |
|---|---|---|---|---|
| Loading screens (`load_fade`, `load_light`, `load_town`, `fademap`) | Shows art, a hint and a progress bar while an area loads, and fades in and out. | Area loads hitch, with no loading screen. | M | Hint table, async area load (render/sim) |
| Epilogue slideshow (`slideshow.gfx`) | End-game slides with text from plot flags. | Nothing. | M | Sim: epilogue plot evaluation |
| Credits (`credits.gfx`) | Start-menu Credits and after the ending. | Nothing. | S | — |
| Crafting (`crafting.gfx`) | Herbalism, Poison-Making and Trap-Making recipe screens: lists known recipes, checks ingredients, crafts. | Recipes are learned (#25x), but there is no screen. The item binding has a `Crafting` category. | M | Sim: craft action, ingredient consume |
| Rune/item upgrade (`itemupgrade.gfx`) | Puts runes in weapon and armour slots (Sandal). | Nothing. | M | Sim: rune slot properties |
| Army control (`armycontrol.gfx`) | Summoning allied armies in the final battle. | Nothing. | M | Sim: army summons (late game) |
| Dialogue pop (`dialoguepop.gfx`) | Floating ambient/bark lines (not in conversation). Provisional reading of the name, not traced. | Barks may show via floaty text only. | S–M | Trace the exe |
| Save screen variant (`savescreen.gfx`) | Unclear: possibly the console save UI. Not traced. | We use `saveload.gfx`. | ? | Trace |
| New game (`newgame.gfx`) | Unclear: may be unused on PC (chargen covers origin select). Not traced. | — | ? | Trace |
| Achievement popup (`guiachievementunlocked.gfx`) | Toast when an achievement unlocks. | Nothing. | S | Achievement data/conditions (sim) |
| Screenshots browser (`guiscreenshots.gfx`) | Screenshot gallery. | Nothing. | S | Low priority |
| Book front/back animation (`bookfront.gfx`) | The book opens and closes around book screens. | We draw `bookback` as a still backdrop, with no open or close animation. | S–M | — |
| Out of scope | `login*`, `purchase*`, `prccontentmanager`, `contentinstalling`, `guicommunityfeatures`, `debug*` (BioWare online and DLC store). | Skip. | — | — |

## 2. Screens that exist but are incomplete or wrong

### HUD and party bar

| Item | Original | Ours | Evidence | Size | Depends on |
|---|---|---|---|---|---|
| Quickslots 11–20 key labels | `InputMappings…ActivateQuickslot11..20` (expanded bar). | Unanswered (keys 1–0 are fed from defaults, not keymap). | Unanswered list | S | — |
| Party portraits: current and pending action icons | Icon of what each member is doing or about to do (`CurrentActionIcon`, `PendingActionIcon`). | Unanswered. | Unanswered list | S | Sim command queue (exists) |
| Portrait level-up marker | `PortraitLevelUp` shows the "+" level-up badge. | Unanswered (the class-rank "+" may come from elsewhere). | Unanswered list | S | — |
| Portrait tint | `PortraitProperties.PortraitTint` (dead or paralysed tint). | Unanswered. | Unanswered list | S | — |
| Status effects | `GUI.SelectedChar.EffectList` shows buff and debuff icons on portraits and the record. | Unanswered: no effect icons. | Unanswered list | M | Sim effect list with icons and durations |
| Party approval | `Party.Approval` (approval meter in record and party). | Unanswered. | Unanswered list | S | Sim approval values (exist as plot/props?) |
| Hold position | H toggles hold; `Party.HoldPositions` shows it. | No key, value unanswered. | keymap.gda ToggleHoldParty | S | Sim: hold order |
| Summons in the party bar | `Party.Summons.*` (animated dead, and others). | Unanswered. | Unanswered list | S | Sim summons |
| Combat target tooltip | tooltips.gfx `UpdateCreatureTarget`: name, health and mana bars, rank for the hovered or targeted creature. | Not fed. | handoff-ui.md | S–M | — |
| Floaty stats show/hide rule | Exe 0x764820: on screen + LOS + game mode; alpha and fade rule untraced. | Partly: #2xx stat floaties shown by our own rule. | handoff-ui.md RE notes | S | Trace the alpha/hover rule |
| Hide interface | HideMainInterface key hides the HUD. | Not handled. | keymap.gda | S | — |
| Combat/explore HUD mode | `GameModeExplore.PendingTargetID`, `DebugOptions.SuppressPartyActionDisplay`. | Unanswered. | Unanswered list | S | — |

### Book screens

| Item | Original | Ours | Evidence | Size | Depends on |
|---|---|---|---|---|---|
| Tooltip box height | Scale9 tooltip box sized to text. | Box draws about half height (Ruffle scale9 bounds bug). | handoff-ui.md Tooltips | M | Ruffle patch or host-side size override |
| Store coin images | Inline coin images in prices (`<img align='baseline'>`). | Ruffle floats the inline image, so the coins misalign. | Parked | M | Patched Ruffle (Sera's decision) |
| Inventory | Equipment/backpack fully works by mouse. | `Equipment.N.StackSize`, `CanChangeEquipment`, `CanSwitchWeaponSets` and `Party.BackpackSort` are unanswered. Backpack items' `AbilityInstance` (usable items) too. | Unanswered list | S | — |
| Journal | Quest givers and destinations on the map (`Journal.PlotGivers`, `PlotDestinations`), offers (`PlotList.N.OfferId`), remembers the last codex entry. | Unanswered. `undefined.QuestCompleted` is a path bug on our side. | Unanswered list | S–M | Sim plot data |
| Journal text for Dwarf/City Elf openings | — | Empty because the sim has no flag text yet. Correct as far as it goes. | Earlier QA | — | Sim |
| Character record | Class name (`Stats.ClassName`), attributes current vs total for party slots. | ClassName unanswered on the party list. | Unanswered list | S | — |
| Area map quest markers | Plot givers and destinations drawn on the map. | Only map notes (#2xx). | — | S | Journal item above |
| Save thumbnails | Each save stores a screenshot shown in the list. | No thumbnail. | — | M | Render: offscreen capture into the save |
| Options | Applies resolution, graphics, audio, gameplay settings and writes the settings file. | The screen works, but settings change nothing and nothing is saved, and Cancel is untraced (see `options.rs`). | options.rs doc | M | Render/audio hooks per setting |
| Key rebinding | Controls tab rebinds keymap.gda actions. | Keys are hard-coded in `main.rs`; keymap is only used for key names in text. | main.rs | M | Input action layer |
| Chargen | Full chargen. | Open items: tattoos (0x8039b0), the title swirl too bright, the Dual Weapon header bar, the Expression list (57). | handoff-ui.md | S each | — |
| Item examine, Party picker, Death screen, Chanter's board, World map, Store, Level Up | Built. | No known gaps beyond the shared ones above. Not exhaustively diffed. | — | — | — |

## 3. Input paths

| Item | Original | Ours | Size | Notes |
|---|---|---|---|---|
| Keyboard actions not handled | keymap.gda binds SelectPartyMemberNInclusive (Shift+F-keys), ToggleRunWalk, ToggleHoldParty, HideMainInterface, OpenConsole, ToggleWorldMap, QuickSave, QuickLoad, pan-camera keys. | Not all handled. F5 and F9 open the save and load screens instead of quick save and quick load (check against keymap's bindings). | S–M | Best done with a keymap-driven action layer (rebinding item) |
| Bink intro movies | EA logo and main intro at boot, gated by DisableIntroMovies/FirstRun (exe 0x5ec247). | Not played. | M | Video decoder (`thedas-video`); audio stream for sound |
| Cutscene movies (.bik) | In-game Bink cutscenes. | Not played. | M | Same decoder |
| Drag and drop to quickbar | Drag abilities and items from screens to the quickbar. | Partly: AcceptDrop is walked, not fully verified. | S | — |
| Radial/context menus | — | Not checked whether the PC exe has any. | ? | Trace |
| Text entry by pad | — | Pad letters type into the focused field (#30x); there is no on-screen keyboard. | S | Ours, not parity |

## 4. Controller (no console parity data; these are our own additions)

| Item | State | Size |
|---|---|---|
| Pad names in tutorial text (the console's "Press A") | Parked: needs a PC-to-360 string mapping. | S–M |
| Whole-party selection (LB+RB) effect | Toggle works; no HUD shows it; not checked in combat. | S |
| Empty equipment slots as focus targets | Not targets (no handler). | S |
| Inventory paperdoll is one big focus target | — | S |
| Elite 2 paddles | gilrs reports them as Unknown, so they are ignored. | S |
| Movies' own console focus mode | Vestigial on PC; replaced by our host-side focus. | — |

## 5. Smaller known oddities

- startmenu2 rollover highlight shows nothing (mouse included).
- The level's `Quest updated` and `Codex updated` notifications show but were not checked for timing against the original.
- `GUI.SelectedBookPage`, `GUIBookback.ShowContents`, `GUI.TooltipText`/`GUITooltips.TooltipText` are logged unanswered on first read; check whether they matter.
