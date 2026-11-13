# Known gaps: interface, input and controller

[Back to the index](../KNOWN_GAPS.md)

The original Scaleform interface on Ruffle, its binding to the simulation, mouse and keyboard, and the controller support this project adds.

## Part 1: the owning stream's list

Written by the stream that works on this area. Entries marked traced were read from the original executable; trace notes live outside the repository with the test data.

Kept current in place by the Input/UI thread. Last pass: 2026-11-12 16:30Z, main at #324.

Sizes: S = under a day, M = a few days, L = a week or more.

How the list was built:
- The movies in `guiexport.erf` and the patches, compared with the screens we load (`crates/thedas-app/src/screens.rs`).
- `keymap.gda` actions, compared with the keys `main.rs` handles.
- A headless run of `dc_13_hideout_fight.json` that opened I, C, M, J, T, K and P with `RUST_LOG=thedas_app::ui=debug`. It logged 73 distinct engine values the movies asked for and got no answer (log in `OpenThedas-testdata/ui/gaps/log.txt`). Some are benign: party slots 2 and 3 in a two-member party, empty quickbar slots, and chain entries the screen asks about before filtering. The quickbar's cooldowns, status and item icons are fed for filled slots; that was checked against `binding.rs`.
- `handoff-ui.md`.

### 1. Screens never built (the movie exists, we never load it)

| Item | What the original does | What we do | Size | Depends on |
|---|---|---|---|---|
| load_light for same-list area changes, load_fade for the party picker, `fademap` | The exe shows load_light.gfx for an area change within the same area list and load_fade.gfx before the party picker (trace: OpenThedas-testdata/notes/loading-screen-trace.md). | load_town.gfx shows for every area load (UI side merged); the app does not tell area lists apart, and the party picker does not fade. | S | Area-list knowledge (sim/app) |
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

### 2. Screens that exist but are incomplete or wrong

#### HUD and party bar

| Item | Original | Ours | Evidence | Size | Depends on |
|---|---|---|---|---|---|
| Quickslots 11–20 key labels | `InputMappings…ActivateQuickslot11..20` (expanded bar). | Unanswered (keys 1–0 are fed from defaults, not keymap). | Unanswered list | S | — |
| Status effects | `GUI.SelectedChar.EffectList` shows buff and debuff icons on portraits and the record. | Unanswered: no effect icons. | Unanswered list | M | Sim effect list with icons and durations |
| Party approval | `Party.Approval` (approval meter in record and party). | Unanswered. | Unanswered list | S | Sim approval values (exist as plot/props?) |
| Hold position | H toggles hold; `Party.HoldPositions` shows it. | No key, value unanswered. | keymap.gda ToggleHoldParty | S | Sim: hold order |
| Summons in the party bar | `Party.Summons.*` (animated dead, and others). | Unanswered. | Unanswered list | S | Sim summons |
| Combat target tooltip | tooltips.gfx `UpdateCreatureTarget`: name, health and mana bars, rank for the hovered or targeted creature. | Not fed. | handoff-ui.md | S–M | — |
| Floaty stats show/hide rule | Exe 0x764820: on screen + LOS + game mode; alpha and fade rule untraced. | Partly: #2xx stat floaties shown by our own rule. | handoff-ui.md RE notes | S | Trace the alpha/hover rule |
| Hide interface | HideMainInterface key hides the HUD. | Not handled. | keymap.gda | S | — |
| Combat/explore HUD mode | `GameModeExplore.PendingTargetID`, `DebugOptions.SuppressPartyActionDisplay`. | Unanswered. | Unanswered list | S | — |

#### Book screens

| Item | Original | Ours | Evidence | Size | Depends on |
|---|---|---|---|---|---|
| Tooltip box height | Scale9 tooltip box sized to text. | Box draws about half height (Ruffle scale9 bounds bug). | handoff-ui.md Tooltips | M | Ruffle patch or host-side size override |
| Store coin images | Inline coin images in prices (`<img align='baseline'>`). | Ruffle floats the inline image, so the coins misalign. | Parked | M | Patched Ruffle (maintainer's decision) |
| Inventory | Equipment/backpack fully works by mouse. | `Equipment.N.StackSize`, `CanChangeEquipment`, `CanSwitchWeaponSets` and `Party.BackpackSort` are unanswered. Backpack items' `AbilityInstance` (usable items) too. | Unanswered list | S | — |
| Journal | Quest givers and destinations on the map (`Journal.PlotGivers`, `PlotDestinations`), offers (`PlotList.N.OfferId`), remembers the last codex entry. | Unanswered. `undefined.QuestCompleted` is a path bug on our side. | Unanswered list | S–M | Sim plot data |
| Journal text for Dwarf/City Elf openings | — | Empty because the sim has no flag text yet. Correct as far as it goes. | Earlier QA | — | Sim |
| Character record | Attributes current vs total for party slots. | Not checked since the class name was fed (#323). | Unanswered list | S | — |
| Area map quest markers | Plot givers and destinations drawn on the map. | Only map notes (#2xx). | — | S | Journal item above |
| Save thumbnails | Each save stores a screenshot shown in the list. | No thumbnail. | — | M | Render: offscreen capture into the save |
| Options | Applies resolution, graphics, audio, gameplay settings and writes the settings file. | The screen works and settings are kept between runs (`options.ini`, the original's ini form), but most settings change nothing, and Cancel is untraced (see `options.rs`). | options.rs doc | M | Render/audio hooks per setting |
| Key rebinding | Controls tab rebinds keymap.gda actions. | Keys are hard-coded in `main.rs`; keymap is only used for key names in text. | main.rs | M | Input action layer |
| Chargen | Full chargen. | Open items: tattoos (0x8039b0), the title swirl too bright, the Dual Weapon header bar, the Expression list (57). | handoff-ui.md | S each | — |
| Item examine, Party picker, Death screen, Chanter's board, World map, Store, Level Up | Built. | No known gaps beyond the shared ones above. Not exhaustively diffed. | — | — | — |

### 3. Input paths

| Item | Original | Ours | Size | Notes |
|---|---|---|---|---|
| Keyboard actions not handled | keymap.gda binds SelectPartyMemberNInclusive (Shift+F-keys), ToggleRunWalk, ToggleHoldParty, HideMainInterface, OpenConsole, ToggleWorldMap, QuickSave, QuickLoad, pan-camera keys. | Not all handled. F5 and F9 open the save and load screens instead of quick save and quick load (check against keymap's bindings). | S–M | Best done with a keymap-driven action layer (rebinding item) |
| Bink intro movies | EA logo and main intro at boot, gated by DisableIntroMovies/FirstRun (exe 0x5ec247). | Not played. | M | Video decoder (`thedas-video`); audio stream for sound |
| Cutscene movies (.bik) | In-game Bink cutscenes. | Not played. | M | Same decoder |
| Drag and drop to quickbar | Drag abilities and items from screens to the quickbar. | Partly: AcceptDrop is walked, not fully verified. | S | — |
| Radial/context menus | — | Not checked whether the PC exe has any. | ? | Trace |
| Text entry by pad | — | Pad letters type into the focused field (#30x); there is no on-screen keyboard. | S | Ours, not parity |

### 4. Controller (no console parity data; these are our own additions)

| Item | State | Size |
|---|---|---|
| Pad names in tutorial text (the console's "Press A") | Parked: needs a PC-to-360 string mapping. | S–M |
| Whole-party selection (LB+RB) effect | Toggle works; no HUD shows it; not checked in combat. | S |
| Empty equipment slots as focus targets | Not targets (no handler). | S |
| Inventory paperdoll is one big focus target | — | S |
| Elite 2 paddles | gilrs reports them as Unknown, so they are ignored. | S |
| Movies' own console focus mode | Vestigial on PC; replaced by our host-side focus. | — |

### 5. Smaller known oddities

- startmenu2 rollover highlight shows nothing (mouse included).
- The level's `Quest updated` and `Codex updated` notifications show but were not checked for timing against the original.
- `GUI.SelectedBookPage`, `GUIBookback.ShowContents`, `GUI.TooltipText`/`GUITooltips.TooltipText` are logged unanswered on first read; check whether they matter.

## Interface movies measured against a real install

Gathered 2026-11-12 at main `1eb844a` from every archive in the install, a search of
`thedas-ui` and `thedas-app`, and headless runs with the interface log on. The install has 68
distinct `.gfx` movies: 28 are opened as screens, 2 are font libraries, `bookback` is drawn as
an image instead of run, and 37 are never opened.

### Never opened (37)

- **Gameplay**: `armycontrol`, `crafting`, `itemupgrade`, `slideshow`, `credits`,
  `dialoguepop`, `fademap`, `bookfront`, `newgame`, `savescreen`. The matching engine functions
  (`ShowCraftingGUI`, `OpenItemUpgradeGUI`, `BeginSlideshow`) are unimplemented too.
- **Loading screens**: `load_fade` (the party picker's; load_town and load_light show, and the world map is held for journeys).
- **Online and downloadable content**: `login`, `loginconnected`, `loginconnecting`,
  `loginnewaccount`, `loginviewtos`, `prccontentmanager`, `purchaseconfirm`,
  `purchasingcontent`, `contentinstalling`, `guicommunityfeatures`, `guiachievementunlocked`,
  `guiscreenshots`.
- **Debug**: `debug`, `debugoptions`, `fonttest`.
- **Other languages' fonts**: `fonts_cs-cz`, `de-de`, `es-es`, `fr-fr`, `hu-hu`, `it-it`,
  `pl-pl`, `pseudo`, `ru-ru`.

### Engine values a screen asks for and nothing answers

Counted per screen as the keys a run adds over the HUD on the same save. The counts depend on
the save and on party size. No external command went unanswered in any run.

| Screen (movie) | Unanswered | What |
| --- | --- | --- |
| HUD (`floatylayer`, `portraits`, `quickbar`, `minimap`, `notifications`, `navbar`, `tooltips`, `popuplayer`) | 220 | quickslot cooldown, pending, status, item icon and stack size (16 slots); key names for quickslots 11 to 28 and the party picker; per-member current and pending action, portrait tint, paralysed, upkeep cost; the same fields for summons; hold positions; the effect list |
| Spells and talents (`abilities`) | 170 | cooldown, pending and status per ability in the chains |
| Skills (`abilities`) | 98 | the same per skill |
| Inventory | 42 | ability cooldown and pending per backpack row, equipment stack size, backpack sort, approval, can-change-equipment, can-switch-weapon-sets |
| Area map | 14 | tooltip layout keys, plot destinations, plot givers |
| Journal | 5 | last selected codex entry and group, plot offer id, DLC enabled |
| Store | 4 | tooltip text, pending target |
| Party picker | 3 | selected member portraits |
| Character record | 2 | approval |
| Start menu | 2 | tooltip text, hero list |
| Tactics | 1 | |
| Options, save and load, death, conversation, cutscene | 0 | |

`GUIBookback.ShowContents` is unanswered on every book screen because the `bookback` movie
never runs. Not measured, because no headless step opens them: character creation, level up,
the Chanter's Board, the world map, item examine and the game menu. Some HUD keys may be probes
of empty slots; that is not checked against the original.

## Part 2: code inventory

Found by reading the code at main on 2026-11-12: provisional markers, stubs, ignored inputs and features with no code. "Original: not traced" means nobody has yet checked what the original does. Line numbers drift; search for the quoted names if a line has moved.

Scope: `crates/thedas-ui/src/*`, the interface and input parts of `crates/thedas-app/src`
(`ui.rs`, `screens.rs`, `focusnav.rs`, `pad.rs`, `paddev.rs`, `padplay.rs`, `control.rs`,
`saves.rs`, `reticle.rs`, input handling in `main.rs` and `live.rs`) and
`crates/thedas-formats/src/gfx*.rs`. Every item is read from the code at the time of writing.
"Original" quotes what a comment or trace in the code says the original does; where nothing
in the code says, it reads "not traced". For a checklist feature with no code, Original is the
feature's name and Ours is "no code found". Controller items are measured against the project's
own goal of controller support: the PC original has none (`crates/thedas-app/src/pad.rs:7`).

Screens the code knows (`crates/thedas-app/src/screens.rs:20`): Hud (floatylayer, portraits,
quickbar, minimap, notifications, navbar, tooltips, popuplayer), Cutscene (cutscene.gfx),
Conversation (conversation.gfx), WorldMap (worldmap.gfx), Store (store.gfx), Chanters
(chanters.gfx), LevelUp (levelup.gfx), Inventory (inventory.gfx), CharacterRecord
(characterrecord.gfx), Tactics (tactics.gfx), AreaMap (map.gfx), PartyPicker (partypicker.gfx),
Death (deathscreen.gfx), SaveLoad (saveload.gfx), CharGen (chargen.gfx), Journal (journal.gfx),
Options (optionsmenu.gfx), ItemExamine (itemexamine.gfx), Skills and SpellsTalents
(abilities.gfx), StartMenu (startmenu2.gfx), GameMenu (mainmenu.gfx). No other movie is opened
by code.

Movie messages acted on: by the simulation (`crates/thedas-sim/src/chanters.rs:149`): job board
accept and clear, inventory equip, unequip and next/previous character, store buy and sell,
container acquire, acquire all and closed, party picker submit, clear, unselect and show,
`*.SetQuickslot`, world map travel, swap and closed, the journal's and tactics' attributes,
character creation and level-up messages; by `thedas-ui` (`crates/thedas-ui/src/input.rs:86`,
`:189`): conversation reply, perform ability, level-up button, appearance sliders, item filter,
item examine, paperdoll turn, drag, camera preset and portrait refresh, options set and restore;
by the app (`crates/thedas-app/src/ui.rs:942`): dialog boxes and callbacks, options and game
menu, exit to start menu, save, load, delete, resume, journal notifications, book page tabs,
voice previews, `LoadModule`, `QuitGame`. Anything else is logged once as not handled.

### Protocol and binding

- **UI-001** **GUI protocol inferred from bytecode.** Original: not traced. Ours: scene start within 100 ms, `GetValue`/`SetValue`, `GetTlkString`, `SendMessage` and the context fscommands are modelled from the movies' `GUISystem`, `ExternalCommands` and `Localization` classes, not from DAOrigins.exe. Evidence: `crates/thedas-ui/src/protocol.rs:3`. Size: M. Depends: none. Kind: provisional.
- **UI-002** **Values attached to messages inferred.** Original: not traced. Ours: `GUICharGen.CharGenSelectName` carries `GUICharGen.TempCharName` and `SaveLoad.SaveGame` carries `SaveLoad.SaveName` as an extra argument, inferred from the movies setting the value just before sending. Evidence: `crates/thedas-ui/src/protocol.rs:22`. Size: S. Depends: none. Kind: provisional.
- **UI-004** **Input contexts not modelled.** Original: the game's context system decides which input reaches which movie (comment). Ours: `CreateContext` is ignored; the pushed and popped stack is consulted only for the popup layer's `GUIPopup` context. Evidence: `crates/thedas-ui/src/protocol.rs:601`, `crates/thedas-ui/src/stage.rs:548`. Size: M. Depends: none. Kind: provisional.
- **UI-005** **Game message blocking ignored.** Original: not traced. Ours: `BlockGameMessage` and `UnblockGameMessage` are accepted and do nothing. Evidence: `crates/thedas-ui/src/protocol.rs:614`. Size: S. Depends: UI-004. Kind: missing.
- **UI-007** **No inventory of unhandled movie messages.** Original: not traced. Ours: a message nothing takes is logged once at run time ("the simulation does not handle ... yet"); there is no list of the messages the shipped movies send against those handled. Evidence: `crates/thedas-app/src/ui.rs:963`. Size: M. Depends: none. Kind: missing.
- **UI-008** **Unanswered engine values reported only headless.** Original: not traced. Ours: `log_gaps` (failed loads, unanswered `GetValue` names) is called from the headless capture path only, not from the window's loop. Evidence: `crates/thedas-app/src/ui.rs:1767`, `crates/thedas-app/src/main.rs:466`. Size: S. Depends: none. Kind: missing.
- **UI-009** **Movie queries fail while the simulation is locked.** Original: not traced. Ours: tactics pickers, `GetObjectNameByID`, `GetGender`, `HasAbility` and `CreatureCanUseItem` use `try_lock` and return undefined to the movie when the simulation is busy. Evidence: `crates/thedas-ui/src/input.rs:264`. Size: S. Depends: none. Kind: provisional.
- **UI-010** **Message timing relative to the movie's next instruction inferred.** Original: the game acts on a message before the movie goes on (comment). Ours: most messages are acted on inside the call; replies, alert dismissals and tactics messages are deliberately deferred to after the frame, and any message is deferred when the simulation is locked. Evidence: `crates/thedas-ui/src/input.rs:371`. Size: S. Depends: none. Kind: provisional.
- **UI-011** **Designer tags only partly replaced.** Original: `ReplaceDesignerTags` (0x51d620, 0x51f110) replaces dialog tags, key tags and more (comment). Ours: dialog tags, key tags and `<FirstName/>` are replaced; gendered text and other tags are left as written. Evidence: `crates/thedas-ui/src/protocol.rs:51`. Size: M. Depends: none. Kind: missing.
- **UI-012** **Key tag sources assumed.** Original: not traced. Ours: key tags use the first of `keymap.gda`'s binding columns and assume unswapped mouse buttons. Evidence: `crates/thedas-ui/src/keys.rs:12`. Size: S. Depends: UI-059. Kind: provisional.
- **UI-013** **Engine value names and meanings inferred.** Original: not traced. Ours: key names come from the movies' bytecode and their meanings from how the movies use them. Evidence: `crates/thedas-ui/src/binding.rs:4`. Size: M. Depends: none. Kind: provisional.
- **UI-014** **Creature status flags constant.** Original: not traced. Ours: `Poisoned`, `Paralyzed` and `AbilityUpkeepCost` are 0 for every creature ("no status simulation yet"), so portraits never show a status. (The current and pending actions are fed since #321.) Evidence: `crates/thedas-ui/src/binding.rs`, the `creature` closure in `sim_values`. Size: M. Depends: none. Kind: missing.
- **UI-015** **Mana or stamina chosen by class.** Original: not traced. Ours: `Stats.ClassHasMana` is set for the mage class only. Evidence: `crates/thedas-ui/src/binding.rs:303`. Size: S. Depends: none. Kind: provisional.
- **UI-016** **Character record property range assumed.** Original: not traced. Ours: properties 1 to 60 are bound for every creature as the record's rows. Evidence: `crates/thedas-ui/src/binding.rs:340`. Size: S. Depends: none. Kind: provisional.
- **UI-017** **Summons never shown.** Original: not traced. Ours: `Party.Summons.0..3` are always null ("no summoning yet"). Evidence: `crates/thedas-ui/src/binding.rs:477`. Size: M. Depends: none. Kind: missing.
- **UI-018** **Selection count fixed.** Original: not traced. Ours: `Party.SelectedCount` is 1 when there is a hero; only the controlled member reads as selected. Evidence: `crates/thedas-ui/src/binding.rs:272`. Size: M. Depends: UI-123. Kind: missing.
- **UI-019** **Quickbar lock constant.** Original: not traced. Ours: `Party.QuickbarLocked` and `GUI.SelectedChar.Stats.QuickbarLocked` are always 0. Evidence: `crates/thedas-ui/src/binding.rs:273`, `crates/thedas-ui/src/binding.rs:926`. Size: S. Depends: none. Kind: missing.
- **UI-021** **Quickslot picture rule inferred.** Original: not traced. Ours: an item's slot shows the item's icon, else the ability's, inferred from quickbar.gfx. Evidence: `crates/thedas-ui/src/binding.rs:971`. Size: S. Depends: none. Kind: provisional.
- **UI-022** **Quickslot key labels hard-coded.** Original: not traced. Ours: `ActivateQuickslot1..10.PrimaryMappedKey` are always "1".."0", not read from `keymap.gda` or any rebinding. Evidence: `crates/thedas-ui/src/binding.rs:927`. Size: S. Depends: UI-059. Kind: provisional.
- **UI-023** **Book tab key labels hard-coded.** Original: the game's default KeyBindings.ini (comment). Ours: the navbar labels (C, I, J, K, P, M, \) are constants, written before and partly overridden by the `keymap.gda` pass. Evidence: `crates/thedas-ui/src/binding.rs:662`. Size: S. Depends: UI-059. Kind: provisional.
- **UI-024** **Platform values constant.** Original: not traced. Ours: `Scaleform.Platform` is "PC", `Scaleform.IsMac` is 0 on every host, and `DebugOptions.EnableController` is 0. Evidence: `crates/thedas-ui/src/protocol.rs:182`. Size: S. Depends: none. Kind: unverified.
- **UI-026** **Item flags constant.** Original: not traced. Ours: `Damaged`, `Indestructible`, `InfiniteStock` (outside stores), `NewlyAcquired`, `StoreFlag` and `RuneSlotMask` are 0 for every item; new items are never marked and rune slots never show. Evidence: `crates/thedas-ui/src/binding.rs:1055`. Size: M. Depends: none. Kind: missing.
- **UI-027** **Item icon fallback not traced.** Original: not traced. Ours: an item with no icon of its own shows its base type's icon. Evidence: `crates/thedas-ui/src/binding.rs:1027`. Size: S. Depends: none. Kind: provisional.
- **UI-028** **Item lists never group stacks.** Original: not traced. Ours: every backpack, plot, container and store row has `GroupSize` 1. Evidence: `crates/thedas-ui/src/binding.rs:1093`, `crates/thedas-ui/src/binding.rs:1592`. Size: M. Depends: none. Kind: missing.
- **UI-029** **Item requirements not checked.** Original: DAOrigins.exe 0x6ae8b0 checks the item's requirements against the creature (comment). Ours: `CreatureCanUseItem` is true for any existing item and creature, so no row is dimmed. Evidence: `crates/thedas-ui/src/input.rs:337`. Size: M. Depends: none. Kind: missing.
- **UI-030** **Item category tabs inferred.** Original: not traced. Ours: weapons, armour, accessories, crafting, plot and other are decided from `bitm_base` Type and Label. Evidence: `crates/thedas-ui/src/binding.rs:221`. Size: S. Depends: none. Kind: provisional.
- **UI-031** **Equipment slot range assumed.** Original: not traced. Ours: the inventory binds `INVENTORY_SLOT_*` 0 to 19. Evidence: `crates/thedas-ui/src/binding.rs:1659`. Size: S. Depends: none. Kind: provisional.
- **UI-032** **Dialog game mode set by the interface.** Original: not traced. Ours: `GameModeController.CurrentGameMode` reads 4 while a conversation is on screen because the simulation does not switch modes itself. Evidence: `crates/thedas-ui/src/binding.rs:66`. Size: S. Depends: none. Kind: provisional.
- **UI-033** **`GetNumMembers` counting rule inferred.** Original: not traced. Ours: a child counts when it has members or a value other than 0, null or undefined. Evidence: `crates/thedas-ui/src/protocol.rs:403`. Size: S. Depends: none. Kind: unverified.
- **UI-034** **Token observer model inferred.** Original: not traced. Ours: `TokenManager` batches and pushes are modelled from the bytecode; host observer ids start at 1,000,000. Evidence: `crates/thedas-ui/src/lib.rs:244`, `crates/thedas-ui/src/lib.rs:1025`. Size: M. Depends: none. Kind: provisional.
- **UI-035** **Ability attributes delivered by a wrapper.** Original: Scaleform's `SetVariable` from inside an `ExternalInterface` handler (comment). Ours: `SharedLibrary.Ability.LoadFromToken` is wrapped to set `_root.ReturnedAbilityAttributes` first, since Ruffle cannot. Evidence: `crates/thedas-ui/src/avm1.rs:139`. Size: S. Depends: none. Kind: unverified.
- **UI-036** **Scaleform mouse button values inferred.** Original: not traced. Ours: left 1, right 2, middle 3, double click adds 4, as the movies' tests imply. Evidence: `crates/thedas-ui/src/avm1.rs:58`. Size: S. Depends: none. Kind: provisional.
- **UI-037** **Mouse-up ordering worked around.** Original: Scaleform released the button before `onMouseUp` handlers ran (comment). Ours: `MovieUtils.HideMovie` is deferred during a release and flushed after; the order is inferred. Evidence: `crates/thedas-ui/src/avm1.rs:29`. Size: S. Depends: none. Kind: provisional.
- **UI-038** **Post-load function ordering worked around.** Original: Scaleform ran `onLoad` after the movie's code (comment). Ours: `RegisterPostLoadFunction` calls are deferred and run after each frame and mouse event; inferred. Evidence: `crates/thedas-ui/src/avm1.rs:77`. Size: S. Depends: none. Kind: provisional.
- **UI-039** **Hit search ignores text fields and buttons.** Original: not traced. Ours: the clip-under-mouse search counts only clips with mouse handlers. Evidence: `crates/thedas-ui/src/avm1.rs:856`. Size: M. Depends: none. Kind: provisional.
- **UI-040** **Hiding a full-screen interface inferred.** Original: not traced. Ours: `GUI.HideFullScreenGUI` finishes the movie with `FinishSceneFromGame` and closes it. Evidence: `crates/thedas-ui/src/stage.rs:458`. Size: S. Depends: none. Kind: provisional.
- **UI-041** **Quickbar first-frame width restored by the host.** Original: not traced. Ours: the engine's expansion value is written back after quickbar.gfx's first frames because Ruffle runs clip inits in another order. Evidence: `crates/thedas-ui/src/stage.rs:472`. Size: S. Depends: none. Kind: provisional.
- **UI-042** **`_root.Filename` form inferred.** Original: the exe pushes the name at 0x51a242 (comment). Ours: the file name with its extension. Evidence: `crates/thedas-ui/src/stage.rs:424`. Size: S. Depends: none. Kind: provisional.
- **UI-043** **Scaleform tags other than external images dropped.** Original: not traced. Ours: only tag 1001 (`DefineExternalImage`) is inlined; 1002 `FontTextureInfo`, 1003 `DefineExternalGradient`, 1004 `DefineGradientMap`, 1005 `DefineCompactedFont`, 1006 `DefineExternalSound`, 1007 `DefineExternalStreamSound`, 1008 `DefineSubImage` and 1009 `DefineExternalImage2` are dropped. Evidence: `crates/thedas-formats/src/gfx.rs:540`. Size: M. Depends: none. Kind: missing.
- **UI-044** **Scaleform tag numbers and layout inferred.** Original: not traced. Ours: tag codes and the `DefineExternalImage` layout are read from the shipped movies. Evidence: `crates/thedas-formats/src/gfx.rs:9`. Size: S. Depends: none. Kind: provisional.
- **UI-045** **Font library matched by name only.** Original: not traced. Ours: a glyph-less font is resolved from the library by name, not by bold or italic. Evidence: `crates/thedas-formats/src/gfx.rs:339`. Size: S. Depends: none. Kind: provisional.
- **UI-046** **Device-font text switched to outlines.** Original: Scaleform draws device-font fields with its font library (comment). Ours: `UseOutlines` is set on such fields. Evidence: `crates/thedas-formats/src/gfx.rs:395`. Size: S. Depends: none. Kind: provisional.
- **UI-047** **Scaling-grid rule below corner size not traced.** Original: not traced. Ours: Flash's rule for a sprite scaled smaller than its corners is guessed. Evidence: `crates/thedas-formats/src/gfx/scale9.rs:14`. Size: S. Depends: none. Kind: provisional.
- **UI-048** **Atlas cropping inferred.** Original: not traced. Ours: `OffsetAndScale` and `OriginalImageSize` are read from the shipped atlas files. Evidence: `crates/thedas-ui/src/atlas.rs:6`. Size: S. Depends: none. Kind: provisional.
- **UI-049** **Inline images in text laid out by the host.** Original: not traced. Ours: `<img>` is replaced by spaces and the image placed centred on its line; `align`, `vspace`, `hspace` and lines the field wraps itself are not followed, and a 1-2 pixel gutter is guessed. Evidence: `crates/thedas-ui/src/lib.rs:796`, `crates/thedas-ui/src/lib.rs:894`. Size: M. Depends: none. Kind: provisional.

### Front end and options

- **UI-050** **Start menu values partly invented.** Original: not traced. Ours: `GameMode.ShowStartMenuContents` 1, `Client.ModuleInfoList` empty and `Client.VersionString` "OpenThedas <version>". Evidence: `crates/thedas-ui/src/binding.rs:681`. Size: S. Depends: none. Kind: provisional.
- **UI-051** **Other campaigns not listed.** Original: other campaigns (module list). Ours: no code found; `Client.ModuleInfoList` is empty. Evidence: `crates/thedas-ui/src/binding.rs:684`. Size: M. Depends: none. Kind: missing.
- **UI-052** **Social, profile and login screens (EA online).** Original: social/profile/login screens. Ours: no code found; `OnlineFeatures.LoggedIn` 0, `AccountName` empty, `SkipWelcomeMessage` 1. Evidence: `crates/thedas-ui/src/binding.rs:691`. Size: L. Depends: none. Kind: missing.
- **UI-054** **Options Cancel keeps changes.** Original: the exe's handlers at 0x7946c0 are not traced (comment). Ours: a change applies when set, and Cancel keeps it. Evidence: `crates/thedas-ui/src/options.rs:11`. Size: S. Depends: UI-053. Kind: provisional.
- **UI-055** **Video options not applied.** Original: not traced. Ours: `FullScreen`, `UseVSync`, `Gamma`, `TexturePack`, `EnableFrameBufferEffects`, `GraphicsDetailLevel` and `ForceLowSettings` change only what the screen shows; the swap chain is always `AutoVsync`. Evidence: `crates/thedas-ui/src/options.rs:54`, `crates/thedas-app/src/main.rs:804`. Size: M. Depends: UI-053. Kind: missing.
- **UI-056** **Resolution list holds one entry.** Original: not traced. Ours: `ClientOptions.ScreenResList` lists only the current size; the display's other modes are not listed and picking one changes nothing. Evidence: `crates/thedas-ui/src/options.rs:105`. Size: M. Depends: UI-055. Kind: missing.
- **UI-057** **Control options partly applied.** Original: not traced. Ours: `InvertXAxis`, `InvertYAxis` and `MouseSensitivity` apply to mouse look (sensitivity as a plain multiplier); `SwapMouseButtons` and `MouseZoomSensitivity` are still ignored (zoom is a fixed step). Evidence: `crates/thedas-ui/src/options.rs:42`, `crates/thedas-app/src/main.rs` (`device_event`, `wheel`). Size: S. Depends: UI-053. Kind: missing.
- **UI-059** **Key rebinding.** Original: key bindings actually applied and persisted. Ours: no code found; the game's keys are hard-coded in the app (`key_down`, `menu_key`, `quickslot_key`) and `keymap.gda` is read only to name keys in text and tabs. Evidence: `crates/thedas-app/src/main.rs:1282`, `crates/thedas-ui/src/binding.rs:875`. Size: L. Depends: UI-053. Kind: missing.
- **UI-209** **Ambient lines' subtitles and the voice flag's cases.** Original: an ambient line with a voice shows as a floaty over its speaker only when Subtitles is "Dialog and ambient" (exe 0x6fe73c: skipped when voiced and `SubtitlesLevel` is 1 or less); a line's voice flag (line+0x61) is set when its voice-over starts playing (0x7cf998). Ours: ambient lines' text is not shown as floaties at all; a line counts as heard when its voice-over is in the banks or its cutscene carries it (`vo_mode`). Evidence: `crates/thedas-app/src/main.rs` (`line_is_voiced`). Size: S. Depends: none. Kind: provisional.
- **UI-205** **Special-move floaty option not applied.** Original: not traced (`ShowSpecialMoveFloaties`, the options' +0xf8; no reader found beside the damage numbers' at 0x6a7414). Ours: ignored. Evidence: `crates/thedas-ui/src/options.rs` (`DEFAULTS`). Size: S. Depends: none. Kind: missing.
- **UI-063** **Gore option not applied.** Original: not traced. Ours: `GameOptions.ShowGore` is ignored, and `SetCreatureGoreLevel` is a no-op. Evidence: `crates/thedas-ui/src/options.rs:31`, `crates/thedas-sim/src/engine.rs:2823`. Size: S. Depends: none. Kind: missing.
- **UI-064** **Loot shimmer and plot assist options not applied.** Original: not traced. Ours: `LootShimmer`, `PlotAssist` and `RevealPlotHelp` are kept but nothing reads them. Evidence: `crates/thedas-ui/src/options.rs:33`. Size: S. Depends: none. Kind: missing.
- **UI-065** **Plot help default guessed.** Original: not among the exe's defaults found (comment). Ours: `RevealPlotHelp` defaults to 0. Evidence: `crates/thedas-ui/src/options.rs:40`. Size: S. Depends: none. Kind: provisional.
- **UI-066** **Autosave.** Original: autosave (`GameOptions.AutoSave`, `DoAutoSave`). Ours: no code found; the option is ignored and `DoAutoSave` is a no-op. Evidence: `crates/thedas-sim/src/engine.rs:2823`. Size: M. Depends: none. Kind: missing.
- **UI-067** **Online options without online features.** Original: not traced. Ours: telemetry, screenshot upload, profile upload and entitlement options are listed and do nothing. Evidence: `crates/thedas-ui/src/options.rs:61`. Size: S. Depends: UI-052. Kind: missing.
- **UI-069** **Character creation flags fixed.** Original: not traced. Ours: `GUICharGen.CharCreatorRunning` 0 and `ImportEnabled` 0, so importing a hero is never offered. Evidence: `crates/thedas-ui/src/binding.rs:1293`. Size: S. Depends: none. Kind: provisional.
- **UI-070** **Appearance slider ranges partly guessed.** Original: the exe's slider table 0x80ada0 and groups 0x73a0c1 (comment). Ours: ranges from `chargenmorphcfg.xml` lists; Expression's range is fixed; a slider with no list for the race and gender is left out. Evidence: `crates/thedas-ui/src/appearance.rs:8`. Size: S. Depends: none. Kind: provisional.
- **UI-071** **Appearance colour slider names guessed.** Original: not traced. Ours: Hair / Beard Color for those with beards, Eyebrow / Stubble Color for men. Evidence: `crates/thedas-ui/src/appearance.rs:233`. Size: S. Depends: none. Kind: provisional.
- **UI-072** **Voice preview argument order inferred.** Original: not traced. Ours: `GUI.TestSoundSet` and `GUI.SetSoundSet` arguments are read from the movie's strings. Evidence: `crates/thedas-app/src/ui.rs:338`. Size: S. Depends: none. Kind: provisional.
- **UI-073** **Character creation stage camera blend and head guessed.** Original: interpolation 0x5b0db0 (comment). Ours: a linear blend; the first page's head is the face screen's first preset. Evidence: `crates/thedas-app/src/chargen_stage.rs:23`, `crates/thedas-ui/src/models.rs:145`. Size: S. Depends: none. Kind: provisional.
- **UI-074** **Return to the start menu not traced.** Original: not traced. Ours: `ExitToStartMenuMessage` and `ShutDownToStartMenuMessage` reopen the game with `Start::Menu`. Evidence: `crates/thedas-app/src/ui.rs:475`. Size: S. Depends: none. Kind: provisional.
- **UI-076** **Boot movies depend on ffmpeg and play silent.** Original: `dragon_age_ea_logo.bik` then `dragon_age_main.bik` (comment). Ours: decoded by an external `ffmpeg`, skipped without one, no sound; `DisableIntroMovies` and `FirstRun` are not read; the skip input is not traced. Evidence: `crates/thedas-app/src/intro.rs:5`. Size: M. Depends: none. Kind: provisional.
- **UI-077** **Credits.** Original: credits. Ours: no code found. Evidence: `crates/thedas-app/src/screens.rs:20`. Size: M. Depends: none. Kind: missing.
- **UI-079** **Downloadable content screen.** Original: DLC/downloadable content screen. Ours: no code found; the journal's downloadable content tab values are empty. Evidence: `crates/thedas-ui/src/binding.rs:1240`. Size: M. Depends: UI-052. Kind: missing.
- **UI-080** **Achievements display.** Original: achievements display. Ours: no code found. Evidence: `crates/thedas-app/src/screens.rs:20`. Size: M. Depends: none. Kind: missing.
- **UI-081** **Save thumbnails, kinds and story.** Original: not traced. Ours: no screenshots, every save of kind 0 (`SaveType`), `Story` empty. Evidence: `crates/thedas-ui/src/binding.rs:1306`, `crates/thedas-app/src/saves.rs:7`. Size: M. Depends: none. Kind: missing.
- **UI-082** **One hero in the save list.** Original: not traced. Ours: the hero list holds the hero playing, or a nameless hero with id 1 standing for every save before a game; saves are not grouped by hero. Evidence: `crates/thedas-ui/src/binding.rs:1332`. Size: M. Depends: none. Kind: provisional.
- **UI-083** **Saving allowed at any time.** Original: not traced. Ours: `SaveLoad.SavingEnabled` is 1 whenever there is a hero, in combat and cutscenes too. Evidence: `crates/thedas-ui/src/binding.rs:1325`. Size: S. Depends: none. Kind: provisional.
- **UI-084** **"New Save" row naming not traced.** Original: not traced. Ours: talk-table 336242 is used for the row. Evidence: `crates/thedas-ui/src/binding.rs:1375`. Size: S. Depends: none. Kind: provisional.
- **UI-207** **Quick save details not traced.** Original: `QuickSave` and `QuickLoad` actions (exe 0x51e4d0, 0x51e6f6), saves kept in `QuickSave_` folders (0x642c70); the row's name, how many are kept, any notice shown and when the keys are refused are not traced. Ours: F5 writes one quick save named "Quicksave" over the one before, F9 loads it into the running game, both only with no menu up; nothing is shown on screen; no pad binding. Evidence: `crates/thedas-app/src/ui.rs` (`quick_save`, `quick_load`). Size: S. Depends: none. Kind: provisional.
- **UI-086** **Loading in a game replaces state in place.** Original: not traced. Ours: a load from the in-game screen calls `Sim::load_json` on the running simulation; the front end's `Live::open` path (screens, camera, renderer) is used only before a game. Evidence: `crates/thedas-app/src/ui.rs:658`. Size: M. Depends: none. Kind: unverified.
- **UI-087** **Death screen hint and capture.** Original: not traced. Ours: `GUIDeath.DeathHint` is empty and `Party.CapturedOnDeath` is 0. Evidence: `crates/thedas-ui/src/binding.rs:1426`. Size: S. Depends: none. Kind: missing.

### HUD

- **UI-088** **HUD composition inferred.** Original: not traced. Ours: which movies make up the HUD, that every other screen hides it, and that the world stops behind full-screen menus are inferred. Evidence: `crates/thedas-app/src/screens.rs:10`. Size: M. Depends: none. Kind: provisional.
- **UI-089** **Effect icons.** Original: effects icons on portraits and floaty icons (`ShowFloatyIcon`, 0x662db0). Ours: no code found; `BatchUpdateIconIDs` is always empty and `ShowFloatyIcon` is a no-op. Evidence: `crates/thedas-app/src/ui.rs:1464`, `crates/thedas-sim/src/engine.rs:205`. Size: M. Depends: none. Kind: missing.
- **UI-090** **Off-screen and hidden floaties.** Original: not traced. Ours: `BatchUpdateOffscreenIDs` and `BatchHideIDs` are always empty; a floaty behind the camera gets alpha 0. Evidence: `crates/thedas-app/src/ui.rs:1448`, `crates/thedas-app/src/ui.rs:1465`. Size: S. Depends: none. Kind: missing.
- **UI-091** **Floaty linger and stacking inferred.** Original: not traced. Ours: a 1 s linger and stacking by `BatchUpdateOffsets` are inferred (the head height and distance scale are the exe's, 0x764884 and 0x63bfa0). Evidence: `crates/thedas-app/src/ui.rs` (`update_floaties`). Size: S. Depends: none. Kind: provisional.
- **UI-093** **Floaty layer size message ignored.** Original: not traced. Ours: `GUI.UpdateFloatyLayerStageSize` is accepted and ignored; floaties are placed from the stage's own size. Evidence: `crates/thedas-app/src/ui.rs:594`. Size: S. Depends: none. Kind: unverified.
- **UI-095** **Combat log.** Original: combat log. Ours: no code found. Evidence: `crates/thedas-app/src/screens.rs:57`. Size: M. Depends: none. Kind: missing.
- **UI-096** **Target display.** Original: target display. Ours: no code found beyond the name and health bar over the leader's attack target. Evidence: `crates/thedas-app/src/ui.rs:1367`. Size: M. Depends: none. Kind: missing.
- **UI-097** **Minimap creature filter guessed.** Original: not traced. Ours: every shown creature is flagged within radius; hostiles show once perceived; allies (`FLAG_SHOW_AS_ALLY`) are not told apart and neutrals are hidden. Evidence: `crates/thedas-app/src/ui.rs:1241`, `crates/thedas-app/src/ui.rs:1527`. Size: M. Depends: none. Kind: provisional.
- **UI-098** **Minimap orientation and file fields inferred.** Original: not traced. Ours: north is up (`Area.North` 0), the picture's top is the world's upper y, and MMI fields 3137-3139 are read from the files' contents. Evidence: `crates/thedas-ui/src/binding.rs:774`, `crates/thedas-ui/src/minimap.rs:4`, `crates/thedas-ui/src/minimap.rs:43`. Size: S. Depends: none. Kind: provisional.
- **UI-099** **Minimap picture alpha forced.** Original: the game shows the picture (comment). Ours: the host sets the authored-transparent clip's alpha to 100 every update. Evidence: `crates/thedas-app/src/ui.rs:1557`. Size: S. Depends: none. Kind: provisional.
- **UI-100** **Explored-area mask not tracked.** Original: not traced. Ours: `mapmask.dds` is a clear 1x1 image, so the whole map shows as explored; the simulation's `MapRevealed` field is not used. Evidence: `crates/thedas-app/src/ui.rs:410`, `crates/thedas-sim/src/engine.rs:2817`. Size: M. Depends: none. Kind: missing.
- **UI-101** **Area map blend modes forced.** Original: not traced. Ours: map.gfx's `alpha` and `erase` clips are drawn as `normal`. Evidence: `crates/thedas-app/src/ui.rs:1568`. Size: S. Depends: UI-100. Kind: provisional.
- **UI-102** **Map notes always visible.** Original: not traced. Ours: every enabled note shows (the explored-area rule is not modelled); icon types are read from the data. Evidence: `crates/thedas-ui/src/binding.rs:800`. Size: S. Depends: UI-100. Kind: provisional.
- **UI-103** **Player map and quickbar preferences seeded.** Original: not traced. Ours: `Player.MapZoom` 0, `Player.LegendVisible` 1 and `Party.QuickbarExpandValue` 0 at every start; no profile is read or written. Evidence: `crates/thedas-ui/src/stage.rs:290`, `crates/thedas-ui/src/stage.rs:295`. Size: S. Depends: UI-053. Kind: provisional.
- **UI-104** **Quickbar reach constant.** Original: not traced. Ours: `GUI.QuickbarXMax` is 100000. Evidence: `crates/thedas-ui/src/binding.rs:648`. Size: S. Depends: none. Kind: provisional.
- **UI-105** **Journal notifications simplified.** Original: not traced. Ours: every alert shows as new, there are no large ("uber") alerts, `IntegerArg`, `CustomText` and `CustomIcon` are empty, and which events raise one is inferred. Evidence: `crates/thedas-ui/src/binding.rs:1253`. Size: M. Depends: none. Kind: provisional.
- **UI-106** **Unlock notifications not shown.** Original: the specialisation notice (0x663b80, comment). Ours: `ShowSpecUnlockedNotification` and `ShowAreaUnlockedNotification` do nothing in the interface. Evidence: `crates/thedas-sim/src/engine.rs:206`, `crates/thedas-sim/src/engine.rs:2432`. Size: S. Depends: none. Kind: missing.
- **UI-107** **Tutorial highlights and training step constant.** Original: DAConstants `GUI_HIGHLIGHT_*` (comment). Ours: every highlight is off and `Tutorials.TrainingStep` is 0. Evidence: `crates/thedas-ui/src/binding.rs:759`. Size: M. Depends: none. Kind: missing.
- **UI-108** **Tutorials held under menus guessed.** Original: not traced. Ours: no tutorial popup is listed while a full-screen menu is up. Evidence: `crates/thedas-ui/src/binding.rs:715`. Size: S. Depends: none. Kind: provisional.
- **UI-109** **Area name banner.** Original: area name banner. Ours: no code found. Evidence: `crates/thedas-app/src/screens.rs:57`. Size: S. Depends: none. Kind: missing.
- **UI-110** **Loot and experience notifications.** Original: loot/XP notifications. Ours: no code found. Evidence: `crates/thedas-ui/src/binding.rs:1255`. Size: M. Depends: UI-105. Kind: missing.
- **UI-111** **Hold/free toggle.** Original: hold/free toggle. Ours: no code found. Evidence: `crates/thedas-app/src/main.rs:1282`. Size: M. Depends: none. Kind: missing.
- **UI-112** **Party selection by portrait.** Original: party selection. Ours: no handler for a portrait click selecting a member was found; F1-F4 and a click on a member in the world change the leader. Evidence: `crates/thedas-app/src/main.rs:1301`, `crates/thedas-app/src/live.rs:1514`. Size: M. Depends: UI-123. Kind: missing.
- **UI-113** **Modal ability indicator by query.** Original: `Engine_SetModalAbilityGUI` sets the quickbar indicator (comment). Ours: the call is a no-op; the indicator follows the simulation's modal state instead. Evidence: `crates/thedas-sim/src/engine.rs:202`, `crates/thedas-ui/src/binding.rs:963`. Size: S. Depends: none. Kind: unverified.
- **UI-114** **Portrait drawing guessed.** Original: portraits redrawn by `UpdatePortraitMessage` (comment). Ours: which changes redraw them is not traced; a 1.8 brightness gain stands in for the portrait light rig; the camera uses some `portraits.gda` columns only. Evidence: `crates/thedas-app/src/ui.rs:197`, `crates/thedas-app/src/live.rs:1049`, `crates/thedas-app/src/live.rs:1279`. Size: S. Depends: none. Kind: provisional.

### In-game screens

- **UI-116** **Destroying and dropping items.** Original: destroy. Ours: no code found; the simulation takes only equip, unequip and next/previous character from the inventory. Evidence: `crates/thedas-sim/src/chanters.rs:159`. Size: M. Depends: none. Kind: missing.
- **UI-117** **Splitting stacks.** Original: split stacks. Ours: no code found. Evidence: `crates/thedas-sim/src/chanters.rs:159`. Size: M. Depends: UI-028. Kind: missing.
- **UI-118** **Sorting and junk.** Original: sorting/filter tabs and junk. Ours: filter tabs work; sorting has no code (`Party.BackpackSort` 0) and junk has no code found. Evidence: `crates/thedas-ui/src/binding.rs:1080`. Size: M. Depends: none. Kind: missing.
- **UI-119** **Weapon set switching from the interface.** Original: weapon sets. Ours: `CanSwitchWeaponSets` and `ActiveWeaponSet` are bound, but no interface message or key switches sets. Evidence: `crates/thedas-ui/src/binding.rs:1652`. Size: S. Depends: none. Kind: missing.
- **UI-120** **Equipment always changeable.** Original: not traced. Ours: `GUI.SelectedChar.CanChangeEquipment` is always 1. Evidence: `crates/thedas-ui/src/binding.rs:1648`. Size: S. Depends: none. Kind: unverified.
- **UI-121** **Containers take nothing.** Original: not traced. Ours: `GUIInventory.CanAddItemsToContainer` is 0. Evidence: `crates/thedas-ui/src/binding.rs:1598`. Size: S. Depends: none. Kind: provisional.
- **UI-122** **Paperdoll turning guessed.** Original: `RotatePaperdollCameraMessage` (comment). Ours: the drag is taken as degrees of yaw, its vertical part unused; one button press turns 22.5 degrees. Evidence: `crates/thedas-ui/src/input.rs:173`, `crates/thedas-ui/src/input.rs:183`. Size: S. Depends: none. Kind: provisional.
- **UI-123** **Selection box and group selection.** Original: selection box/marquee and group selection. Ours: no code found for the mouse; a click orders the leader only. The pad has a whole-party toggle. Evidence: `crates/thedas-app/src/live.rs:1157`, `crates/thedas-app/src/padplay.rs:329`. Size: L. Depends: none. Kind: missing.
- **UI-124** **Store buyback.** Original: buy/sell/buyback. Ours: buy and sell are handled; no code found for buyback. Evidence: `crates/thedas-sim/src/chanters.rs:163`. Size: M. Depends: none. Kind: missing.
- **UI-125** **Store lists' layout borrowed.** Original: not traced. Ours: store.gfx's lists are taken to use inventory.gfx's layout. Evidence: `crates/thedas-ui/src/binding.rs:1070`. Size: S. Depends: none. Kind: provisional.
- **UI-126** **Crafting.** Original: crafting. Ours: no code found; no crafting screen is opened. Evidence: `crates/thedas-app/src/screens.rs:20`. Size: L. Depends: none. Kind: missing.
- **UI-127** **Party camp gift UI.** Original: party camp gift UI. Ours: no code found. Evidence: `crates/thedas-app/src/screens.rs:20`. Size: M. Depends: none. Kind: missing.
- **UI-128** **Party picker hover text.** Original: not traced. Ours: `PartyPicker.MouseOverName` and `MouseOverDescription` are empty. Evidence: `crates/thedas-ui/src/binding.rs:1440`. Size: S. Depends: none. Kind: missing.
- **UI-129** **Party picker closing submits.** Original: not traced. Ours: a picker that closes without Accept keeps the members picked. Evidence: `crates/thedas-app/src/ui.rs:845`. Size: S. Depends: none. Kind: provisional.
- **UI-130** **Tactics presets and camp.** Original: not traced. Ours: no custom presets (`NumCustomSlots` 0), `DirtyPreset` 0, `Party.InCamp` 0; row count from `NumTactics`. Evidence: `crates/thedas-ui/src/binding.rs:1490`, `crates/thedas-ui/src/binding.rs:1520`. Size: M. Depends: none. Kind: missing.
- **UI-131** **Conversation history.** Original: conversation history. Ours: no code found; `Journal.Conversations` is empty. Evidence: `crates/thedas-ui/src/binding.rs:1243`. Size: M. Depends: none. Kind: missing.
- **UI-132** **Journal selection not remembered.** Original: the exe keeps the last selection (`GUIJournal.LastSelected*`, comment). Ours: the active or latest quest stands in; quest groups are always expanded. Evidence: `crates/thedas-ui/src/binding.rs:1216`, `crates/thedas-ui/src/binding.rs:1154`. Size: S. Depends: none. Kind: provisional.
- **UI-133** **Auto-level and level-up checks.** Original: the exe sums points 34, 35, 36, 38 (0x6acbc0) then checks the class (comment). Ours: the class checks are not traced; `Stats.AutoLevelUp` is always 0. Evidence: `crates/thedas-ui/src/binding.rs:366`, `crates/thedas-ui/src/binding.rs:453`. Size: S. Depends: none. Kind: provisional.
- **UI-134** **Ability chain rules inferred.** Original: the engine's own rule is not recovered (comment). Ours: trees, roots, screen masks and display states are read from ABI_base, guitypes and cla_data. Evidence: `crates/thedas-ui/src/chains.rs:12`. Size: M. Depends: none. Kind: provisional.
- **UI-135** **World map pin states guessed.** Original: not traced. Ours: pin states are taken to be the scripts' `WM_LOCATION_*` values and `RecentlyActivated` is always 0. Evidence: `crates/thedas-ui/src/binding.rs:548`, `crates/thedas-ui/src/binding.rs:616`. Size: S. Depends: none. Kind: provisional.
- **UI-136** **Game completion figure.** Original: not traced. Ours: `GUICharacterRecord.GameCompletionPercentage` is 0. Evidence: `crates/thedas-ui/src/binding.rs:852`. Size: S. Depends: none. Kind: missing.
- **UI-137** **Item examine placement guessed.** Original: `GUI_ITEM_EXAMINE` (58) over the screen (comment). Ours: allowed over the inventory, character record and store only. Evidence: `crates/thedas-app/src/screens.rs:165`. Size: S. Depends: none. Kind: provisional.
- **UI-138** **Book backdrop simplified.** Original: `GUIBookback.ShowBlack` 0x716a00 (comment). Ours: the black is drawn whenever the book is, and the book's opening and closing animations are not played. Evidence: `crates/thedas-ui/src/stage.rs:191`. Size: M. Depends: none. Kind: provisional.
- **UI-139** **One screen at a time.** Original: not traced. Ours: a cutscene, conversation, picker, creation, store, container, job board or world map pre-empts the player's menu; only the item examine screen stacks. Evidence: `crates/thedas-app/src/screens.rs:124`. Size: S. Depends: UI-088. Kind: provisional.
- **UI-140** **Save name field restrict changed.** Original: the movie's restrict excludes space, `/` and `t` in Flash syntax (comment). Ours: a deliberate deviation asked for by the owner lets spaces and lowercase `t` through. Evidence: `crates/thedas-ui/src/lib.rs:388`. Size: S. Depends: none. Kind: unverified.

### Conversation and cutscene UI

- **UI-141** **Reply icons constant.** Original: skill-check tags (conversation wheel/list). Ours: `Conversation.Responses.N.Icon` is always 0, so persuade, intimidate and investigate replies carry no icon. Evidence: `crates/thedas-ui/src/binding.rs:528`. Size: M. Depends: none. Kind: missing.
- **UI-142** **Voice-over flag constant and line time guessed.** Original: a line that plays its own cutscene gets the cutscene's length (0x68e5be, comment). Ours: `Conversation.HasVO` is 0; a line without voice-over plays for a reading time (1.5 s plus 55 ms a character, at most 12 s). Evidence: `crates/thedas-ui/src/binding.rs:531`, `crates/thedas-app/src/ui.rs:323`. Size: M. Depends: none. Kind: provisional.
- **UI-143** **Cutscene-spoken line end assumed.** Original: not traced. Ours: a line whose cutscene the cutscene mode plays (`CUTR`) is taken to end with it. Evidence: `crates/thedas-app/src/ui.rs:312`. Size: S. Depends: none. Kind: provisional.
- **UI-144** **Conversation continue input guessed.** Original: not traced. Ours: any left click skips a playing line or continues one with nothing to choose. Evidence: `crates/thedas-app/src/ui.rs:978`. Size: S. Depends: UI-004. Kind: provisional.
- **UI-145** **Cutscene subtitle comment out of date.** Original: n/a. Ours: the comment says nothing is staged behind cutscene subtitles (no camera), while cutscene cameras exist (`cutscene_camera`). Evidence: `crates/thedas-app/src/ui.rs:1204`, `crates/thedas-app/src/main.rs:877`. Size: S. Depends: none. Kind: unverified.
- **UI-146** **Cutscene skip input guessed.** Original: not traced. Ours: any key or left click skips a cutscene. Evidence: `crates/thedas-app/src/main.rs:1391`, `crates/thedas-app/src/main.rs:2041`. Size: S. Depends: none. Kind: provisional.
- **UI-147** **Cutscene pause.** Original: cutscene pause. Ours: no code found; Space pauses the live simulation, but no cutscene pause is offered. Evidence: `crates/thedas-app/src/main.rs:1345`. Size: S. Depends: none. Kind: missing.
- **UI-148** **Cutscene movies silent; path resolution guessed.** Original: not traced. Ours: Bink movies play through ffmpeg without sound; the movie path follows the resource stack. Evidence: `crates/thedas-app/src/intro.rs:88`. Size: M. Depends: UI-076. Kind: provisional.
- **UI-149** **Reply highlight for the pad guessed.** Original: the PC game has none (comment). Ours: reply n is taken to be `Response{n+1}_mc`. Evidence: `crates/thedas-app/src/ui.rs:1004`. Size: S. Depends: none. Kind: provisional.

### Mouse and keyboard

- **UI-150** **Keyboard movement.** Original: WASD movement. Ours: no code found for moving the leader by keyboard; WASD fly the free camera when the follow camera is off. Evidence: `crates/thedas-app/src/main.rs:895`. Size: M. Depends: UI-059. Kind: missing.
- **UI-151** **Camera pan.** Original: camera pan. Ours: no code found (no keyboard pan, no edge scroll). Evidence: `crates/thedas-app/src/main.rs:849`. Size: M. Depends: none. Kind: missing.
- **UI-152** **Middle mouse button.** Original: not traced. Ours: the window handles left and right buttons only; Scaleform's middle button value is defined but never delivered. Evidence: `crates/thedas-app/src/main.rs:2073`, `crates/thedas-ui/src/avm1.rs:61`. Size: S. Depends: none. Kind: missing.
- **UI-153** **Zoom step guessed.** Original: the exe scales zoom by an option times 0.05 (0x7714c4, comment). Ours: a fixed 0.1 per wheel notch. Evidence: `crates/thedas-app/src/follow.rs:23`. Size: S. Depends: UI-057. Kind: provisional.
- **UI-154** **Extra hotkeys.** Original: not traced. Ours: T (besides \) opens tactics and O opens options, keys the comment says are not traced. Evidence: `crates/thedas-app/src/main.rs:1322`. Size: S. Depends: UI-059. Kind: provisional.
- **UI-155** **Hotkeys by physical position.** Original: not traced. Ours: game keys are matched by winit physical key code while the labels shown come from `keymap.gda` names, so on non-US layouts the key pressed and the label differ. Evidence: `crates/thedas-app/src/main.rs:2037`. Size: S. Depends: UI-059. Kind: unverified.
- **UI-157** **Tab highlight.** Original: tab highlight. Ours: no code found. Evidence: `crates/thedas-app/src/main.rs:1282`. Size: M. Depends: none. Kind: missing.
- **UI-158** **Orders while paused unverified.** Original: tactical pause and orders while paused. Ours: clicks while paused queue ordinary commands that run on resume; not compared with the original. Evidence: `crates/thedas-app/src/live.rs:297`. Size: S. Depends: none. Kind: unverified.
- **UI-159** **Cursor states.** Original: cursor states. Ours: no code found; the system cursor is shown, hidden only while the right button turns the camera. Evidence: `crates/thedas-app/src/main.rs:1476`. Size: M. Depends: none. Kind: missing.
- **UI-160** **Context actions on hover.** Original: context actions on hover. Ours: no code found; mouse movement goes only to the movies. Evidence: `crates/thedas-app/src/main.rs:1381`. Size: M. Depends: UI-159. Kind: missing.
- **UI-161** **Interface hit test by pixel alpha.** Original: the game hit-tests the movies' shapes (comment). Ours: a pixel at least 16/255 opaque takes the click, read back from the GPU per query. Evidence: `crates/thedas-app/src/ui.rs:1590`. Size: M. Depends: none. Kind: provisional.
- **UI-162** **Double click thresholds guessed.** Original: not traced. Ours: 0.35 s and 6 pixels. Evidence: `crates/thedas-app/src/main.rs:771`. Size: S. Depends: none. Kind: provisional.
- **UI-163** **Movies hear keys only while typing.** Original: the game forwards keys to its movies by rules not traced (comment). Ours: key events reach a movie only while one of its text fields has focus; otherwise movies' own key handlers hear nothing. Evidence: `crates/thedas-app/src/main.rs:1235`. Size: M. Depends: UI-004. Kind: missing.
- **UI-164** **No input method editor support.** Original: not traced. Ours: typed characters come from winit's key event text only; there is no IME composition. Evidence: `crates/thedas-app/src/main.rs:1272`. Size: M. Depends: none. Kind: missing.
- **UI-165** **Text focus release guessed.** Original: not traced. Ours: a press drops the text field's focus first so one click on a dialog button is enough. Evidence: `crates/thedas-ui/src/stage.rs:158`. Size: S. Depends: none. Kind: provisional.
- **UI-166** **Screenshot key.** Original: screenshot key (`ScreenshotKey` tag). Ours: F12 writes a capture without the interface to the working directory; `TakeScreenshot` is a no-op. Evidence: `crates/thedas-app/src/main.rs:1351`, `crates/thedas-sim/src/engine.rs:2823`. Size: S. Depends: UI-059. Kind: missing.
- **UI-167** **Aiming reticle art.** Original: not traced. Ours: a plain outline, not the game's reticle art. Evidence: `crates/thedas-app/src/reticle.rs:4`. Size: S. Depends: none. Kind: provisional.
- **UI-168** **Creature pick volume guessed.** Original: not traced. Ours: creatures are picked as 1.9 m cylinders. Evidence: `crates/thedas-app/src/live.rs:1059`. Size: S. Depends: none. Kind: provisional.

### Controller

Controller support is a deliberate addition; the PC original has none. These items are gaps
against the project's own goal.

- **UI-169** **Layout and tuning chosen.** Original: no controller support on PC; the layout follows the Xbox 360 version's published controls (comment). Ours: dead zones, repeat rates and speeds are chosen. Evidence: `crates/thedas-app/src/pad.rs:19`. Size: S. Depends: none. Kind: provisional.
- **UI-171** **Rumble.** Original: rumble (not in the PC original). Ours: no code found. Evidence: `crates/thedas-app/src/paddev.rs:1`. Size: S. Depends: none. Kind: missing.
- **UI-172** **Button glyphs and prompts.** Original: glyphs/button prompts (not in the PC original). Ours: no code found beyond "(A: Next)" text appended to tutorial titles. Evidence: `crates/thedas-ui/src/binding.rs:737`. Size: L. Depends: none. Kind: missing.
- **UI-173** **Non-Xbox pads.** Original: not in the PC original. Ours: pads map through gilrs's SDL database to an Xbox layout; there are no PlayStation or Nintendo labels or layouts. Evidence: `crates/thedas-app/src/paddev.rs:25`. Size: M. Depends: UI-172. Kind: missing.
- **UI-174** **Pad remapping.** Original: not in the PC original. Ours: no code found; the mapping is a constant table. Evidence: `crates/thedas-app/src/pad.rs:375`. Size: M. Depends: UI-059. Kind: missing.
- **UI-175** **Focus navigation by geometry.** Original: the 360 version's focus order (comment, not traced). Ours: the next button is chosen by position; coverage per screen is not recorded. Evidence: `crates/thedas-app/src/focusnav.rs:7`. Size: M. Depends: none. Kind: provisional.
- **UI-176** **Pad text entry.** Original: not in the PC original. Ours: the D-pad steps letters; there is no on-screen keyboard. Evidence: `crates/thedas-app/src/padplay.rs:434`. Size: M. Depends: none. Kind: missing.
- **UI-177** **Auto-target rule chosen.** Original: the 360 version's auto-targeting (comment). Ours: the nearest sensible thing in front of the camera. Evidence: `crates/thedas-app/src/padplay.rs:11`. Size: S. Depends: none. Kind: provisional.
- **UI-178** **B on screens guessed.** Original: not traced. Ours: B presses a Back, Previous, Cancel, Close or No button, or a dialog's last button. Evidence: `crates/thedas-app/src/padplay.rs:494`. Size: S. Depends: none. Kind: provisional.
- **UI-179** **Quickbar picker pointer placed for one resolution.** Original: not applicable. Ours: the pointer starts at a spot placed by eye at 1280x720, scaled by window size. Evidence: `crates/thedas-app/src/padplay.rs:648`. Size: S. Depends: none. Kind: provisional.
- **UI-180** **Radial menu.** Original: radial menu (the 360 version's). Ours: no code found for a radial; LT opens a paused pointer over the quickbar instead. Evidence: `crates/thedas-app/src/pad.rs:243`. Size: L. Depends: UI-172. Kind: missing.
- **UI-181** **Pad settings and console movie paths.** Original: not in the PC original. Ours: no pad settings exist in the options screen, and `DebugOptions.EnableController` is 0, so the movies' controller paths stay off. Evidence: `crates/thedas-ui/src/protocol.rs:184`. Size: M. Depends: UI-053. Kind: missing.
- **UI-182** **Steam Deck.** Original: Steam Deck. Ours: no code found. Evidence: `crates/thedas-app/src/paddev.rs:1`. Size: M. Depends: UI-176. Kind: missing.
- **UI-183** **Touch input.** Original: touch (iOS/Android goals). Ours: no code found; the window handles mouse and keyboard events only. Evidence: `crates/thedas-app/src/main.rs:2009`. Size: L. Depends: none. Kind: missing.

### Localisation and layout

- **UI-184** **English talk table only.** Original: not traced. Ours: strings come from `*en-us.tlk` only. Evidence: `crates/thedas-ui/src/stage.rs:77`. Size: M. Depends: none. Kind: missing.
- **UI-185** **English font library only.** Original: the game installs `fonts_en-us.gfx` (comment). Ours: no other locale's font library is loaded. Evidence: `crates/thedas-ui/src/lib.rs:409`. Size: M. Depends: UI-184. Kind: missing.
- **UI-186** **Locale value constant.** Original: not traced. Ours: `ClientOptions.GameOptions.Locale` is "en-us"; the `--language` argument selects only the voice-over folder. Evidence: `crates/thedas-ui/src/binding.rs:701`, `crates/thedas-app/src/main.rs:218`. Size: S. Depends: UI-184. Kind: missing.
- **UI-187** **Stage scaling inferred.** Original: show-all scale mode (0x95af2a) and centred views (0x514aaa) (comment). Ours: no-scale with the visible area reported as `Stage.width`/`height`; that reading is inferred, not checked against Scaleform, and 4:3, 16:10 and ultrawide windows are not verified. Evidence: `crates/thedas-ui/src/lib.rs:443`. Size: M. Depends: none. Kind: provisional.
- **UI-188** **Focus target placement inferred.** Original: not traced. Ours: read from where the start menu's buttons draw, not from Ruffle's view matrix. Evidence: `crates/thedas-ui/src/lib.rs:1144`. Size: S. Depends: UI-187. Kind: provisional.
- **UI-189** **HiDPI.** Original: HiDPI. Ours: no code found that reads the window's scale factor or handles a scale change; the window opens at a logical size and the interface fills physical pixels. Evidence: `crates/thedas-app/src/main.rs:788`, `crates/thedas-app/src/main.rs:2012`. Size: S. Depends: none. Kind: unverified.
- **UI-190** **Fullscreen.** Original: full screen (`VideoOptions.FullScreen`). Ours: no code found; the game runs in a window. Evidence: `crates/thedas-app/src/main.rs:777`. Size: M. Depends: UI-055. Kind: missing.

### Other

- **UI-192** **Tooltip relayout by repeated calls.** Original: not traced. Ours: the fixed tooltip is laid out again for six frames after its text changes, because Ruffle reports unwrapped heights for a frame. Evidence: `crates/thedas-app/src/ui.rs:904`. Size: S. Depends: none. Kind: unverified.
- **UI-193** **Paperdoll and portrait lighting guessed.** Original: rigs and probes named at 0x7bbb30 and 0x7f4037 (comment). Ours: the rig sits at the member's feet facing the viewer; the game's camera presets are not used. Evidence: `crates/thedas-app/src/live.rs:1387`, `crates/thedas-app/src/live.rs:1426`. Size: S. Depends: none. Kind: provisional.
- **UI-194** **Head and eye meshes found by name.** Original: not traced. Ours: portraits aim at `_uem_` or `_uhm_` meshes found by name. Evidence: `crates/thedas-app/src/live.rs:1000`. Size: S. Depends: none. Kind: provisional.
- **UI-195** **Model presentation guesses in thedas-ui.** Original: not traced. Ours: the base head, helmet hiding hair, body parts, body tint rows, corpse fallback, weapon draw by combat state, weapon style, default attack time, animation variants, placeable swings, played-animation loops and location effects are each marked provisional. Evidence: `crates/thedas-ui/src/models.rs:19`, `crates/thedas-ui/src/models.rs:49`, `crates/thedas-ui/src/models.rs:62`, `crates/thedas-ui/src/models.rs:219`, `crates/thedas-ui/src/models.rs:242`, `crates/thedas-ui/src/models.rs:399`, `crates/thedas-ui/src/models.rs:569`, `crates/thedas-ui/src/models.rs:651`, `crates/thedas-ui/src/models.rs:675`, `crates/thedas-ui/src/models.rs:681`, `crates/thedas-ui/src/models.rs:821`, `crates/thedas-ui/src/models.rs:868`, `crates/thedas-ui/src/models.rs:902`, `crates/thedas-ui/src/models.rs:983`. Size: M. Depends: none. Kind: provisional.
- **UI-197** **Plot-action use not raised.** Original: `UsePlotActionMessage` (exe 0x6fc4ac) raises `EVENT_TYPE_USE_PLOTACTION` (68) from the interface; of the shipped movies only armycontrol.gfx (the final battle's army summons) sends it. Ours: armycontrol.gfx is never opened, so nothing sends it. Evidence: exe 0x6fc540. Size: S. Depends: the army control screen. Kind: missing.
- **UI-198** **No one-press pad route to level-up.** Original: the PC build has no console layout; the Xbox 360 version's route is not traced. Ours: a pad reaches Level Up through Start or Back, then the Character Record's Level Up button (or the pointer on the portrait's +). Evidence: `crates/thedas-app/src/pad.rs` (`BINDINGS`). Size: S. Depends: 360 layout. Kind: missing.
- **UI-202** **Only new quests show the large notification.** Original: notifications.gfx also has large headers for a new codex entry, ability, specialization and map area. Ours: only a quest's first entry raises one. Evidence: `crates/thedas-sim/src/journal.rs`. Size: M. Depends: none. Kind: missing.
- **UI-206** **Stats floaties: selected object, sight and mana not traced.** Original: a creature's name floaty exists while hovered, while the highlight key is held, or while it is the game mode's selected object (creature+0x42d, set at 0x70d6e0 from the mode's +0xcc, not traced); it shows only in sight (raycast 0x648da0, mask 0x3024) in game modes 0, 1 and 12; flags add HAS_MANA for a class with mana (0x71a060); floatylayer.gfx's SHOW_TARGET flag is never sent by the exe's flags (0x764e40). Ours: the leader's attack target stands in for the selected object; no sight test; HAS_MANA never sent; dead creatures get none; the pad has no highlight button. Evidence: `crates/thedas-app/src/ui.rs` (`stat_floaties`). Size: S. Depends: none. Kind: provisional.

### Checked and looks complete

- Start menu (startmenu2.gfx): New Game, Load, Options and Quit reach the app (`LoadModule`, `QuitGame`, front-end requests).
- Game menu over a running game (Escape, pad Start), with Options, Save, Load and Exit to the start menu.
- Options screen: settings set, restore defaults, difficulty passed to the simulation, audio volumes and mutes applied.
- Character creation screen (chargen.gfx): gender, race, class, background, name, appearance sliders, voice set and previews, paperdoll camera presets.
- Level-up screen (levelup.gfx) opened from the character record or a portrait; points spent through the simulation.
- Skills and spells/talents screens (abilities.gfx) with ability chains, specialization trees included.
- Character record with attributes, modifiers and Heroic Accomplishments rows.
- Inventory: backpack, plot items, category tabs, equip and unequip, next/previous member, paperdoll drawing and turning, equipment layouts (Shale, the dog), backpack size and capacity, money.
- Item examine screen opened from inventory, character record and store.
- Container looting through the inventory's container panel (take one, take all, close).
- Store buy and sell with mark-up and mark-down.
- Job board (chanters.gfx) accept and close.
- Party picker: open from the HUD when scripts allow, pick in the world, unselect, clear, accept.
- Tactics screen: slots, targets, conditions, actions, abilities, items, predefined presets, enable switch.
- Journal: active and completed quests, quest groups, codex groups and entries, read marks, active quest, opening on a clicked notification.
- World map: primary and secondary maps, pins, travel, swap, close.
- Area map and minimap: picture, camera and creature markers, map notes.
- HUD: portraits with health and mana or stamina, quickbar with 40 slots, drag to set, cooldowns, item counts, modal status; navbar tabs; tooltips; notifications.
- Floating messages from `DisplayFloatyMessage` and health bars over hostiles in combat.
- Pause (Space) with the floaty layer's pause border, also shown for tutorial pauses.
- Tutorial popups with Next, Close and Codex.
- Message boxes from movies (popup layer `GUI.ShowDialogBox` and `GUI.InvokeCallback`).
- Save, load and delete through saveload.gfx with typed save names; death screen with Resume and Load.
- Conversation screen: NPC line, replies, continue, line timing held for voice-over, pad reply highlight.
- Cutscene subtitles and skip.
- Mouse: click to move, attack, talk and use; right-drag camera orbit; wheel zoom or list scroll; right click on interface; double click into movies; ability aiming with reticle and right-click cancel.
- Keyboard: 1-0 quickslots, F1-F4 leader, I/C/J/K/P/M/\ screens, Escape, F5/F9 save and load screens, Q/E camera turn, text entry with edit keys and Ctrl/Cmd+A.
- Controller: device hot-plug (the pad used last drives), field, picker, screen, conversation and cutscene contexts, stick walking, camera, pointer and scroll, LB/RB leader and screen paging, whole-party toggle, D-pad target cycling.
- Key names in text from `keymap.gda` and `inputmap.gda` (`<...Key/>` designer tags).
- GFx loading: external images inlined, font library fonts substituted, scaling grids and engine image slots laid out.
