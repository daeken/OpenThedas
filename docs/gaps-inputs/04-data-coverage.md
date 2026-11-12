# 04: Data coverage

Gathered 2026-11-12 on main 1eb844a. Sources: `thedas ls` over all 643 .erf and the .rim archives in the install (59,004 entries),
`thedas corpus` (PASS in 124 s; 563,593 entries, 45,962 encrypted DLC entries unread), the GOG 1.05 m2da_base.gda (patch003, 421
worksheet prefixes), and greps of crates/. Names and counts only.

## GDA tables

3,519 distinct .gda names (1,787 in packages/core, the rest only in addins, offers or core_ep1).

Classification (static and heuristic, so read the caveats):

| class | core (packages/core) | DLC/addin/offer only |
| --- | --- | --- |
| named in repo source | 50 | 0 |
| read by M2DA id in sim source | 12 | 49 |
| referenced by constant M2DA id in compiled scripts (the sim serves these generically) | 42 | 110 |
| M2DA worksheet with no static reference | 324 | 170 |
| not an M2DA worksheet, no static reference | 1359 | 1403 |

Caveats. "No static reference" overstates what is unread. Many tables are reached through ids or names stored in other tables or
computed at run time, and no grep sees those:
- sse_*/sst_* (686, sound-set events and types) are read by thedas-app/src/audio.rs as `sse_{key}`/`sst_{key}`, so they are consumed.
- *_variation tables are reached through bitm_base variation ids (thedas-ui models.rs `tables.get(id, "")`), so they are probably consumed.
- statecnt_* are named by placeable_types' state-controller column (nav.rs, models.rs read it by name), so they are probably consumed.
- Scripts reach tables by non-constant M2DA id at 3,059 call sites: ability tables, AI packages (aip_*), treasure (ts_*), the approval al*
  tables and others. The sim serves any table they ask for (m2da.rs), so these are read at run time whenever a script asks.
- The UI movies ask for tables by id through Get2DAValueByRowIndexColumnHash, which thedas-ui answers generically.

Tables with no known reader, static or dynamic (inferred):
- Load hints: res_* (351 per-area resource lists), loadhints, high/mid/low/vlowlevelloadhints, temploadhints, chargen_preload. SetLoadHint is a no-op.
- Gestures and ambient animation: gst_* (22), gestures_core_settings, poses, pose_transitions, placeable_interactions. InteractWithObject is not implemented.
- Melee rings: meleering1..9 (boss rings, gaps-sim A4).
- Sync animations: synccreatureanimations, syncdeathanimations, unsyncdeathanimations, nosynccombatanims, syncplaceableanimations,
  syncabilityanimations, synclargecreatures1..5, synccombatanims, halfroundcombatanims, halfroundcombatanimsb.
- Crafting: crafting_recipes, crafting_recipe_types, temp_crafting. The craft flow is not built.
- Items: iprp_* (15, NWN-style item property cost tables), itempropdef, itemprops, armordata.
- Approval and party: gift_values, motivations, heroicpartystats, party_picker (approval is not audited, gaps-sim F12).
- World map: rand_* (7 random-encounter area tables), enc_trips_sp, enc_trips_sp_cli, location_states. Random encounters exist in worldmap.rs;
  whether these tables are read through scripts is unverified.
- Sound: sound_settings, internal_soundsettings, sound_projects_to_load, playersoundsets, ss_probabilitymodif, sound_geometry_mat_types (the footstep
  code reads the material row from the .arl grid; whether it reads this table by id is unverified).
- Appearance and chargen: *_heads, *_hair, human_beard, *_tints, *_tint_masks, raceicons, background_* (may be queried by chargen.gfx by id; unverified).
- GUI, tutorials and plots: trainingsteps, tutorials, camera_selection, dialog_tokens, stringtokens, main_plot_screenshots(2), plotactionsets, plotactionstates.
- Rules: autobalance, diffsettings, resistances, creaturetypes, simplecreature, lock_difficulty, trap_difficulty, starting_points, var_* (12 toolset variable tables),
  ai_movetypes, personality, commanddatatype(s), commandtargettypes, commagetacticpreset, abilityusetype, crusts.
- Army battle (Denerim finale): cli_army_* (8).
- Toolset and test: toolset_*, test_casting, mb_test* (5).

The full grouped list of core tables with no static reference (by family):

### Core tables, class "worksheet-unref", by family

- **AI** (140): name families aip_* ×130, archertacticpreset* ×1, buffertacticpreset* ×1, crowdctacticpreset* ×1, damagertacticspreset* ×1, debuffertacticpreset* ×1, follower_* ×1, healertacticpreset* ×1, nukertacticpreset* ×1, tanktacticspreset* ×1, wardogtacticspreset* ×1
- **other** (59): name families target_* ×5, shale_* ×3, conversation_* ×2, dog_* ×2, abomination_* ×1, arcane_* ×1, areascalegroup* ×1, arl_* ×1, characters* ×1, datetimelocalization* ×1, deathhints* ×1, dialog_* ×1, engineevents* ×1, epilogue_* ×1, eyes* ×1, globals* ×1, golem_* ×1, greatsword_* ×1, heraldry* ×1, heroicstats* ×1, high_* ×1, kite_* ×1, lady_* ×1, longbow_* ×1, longsword_* ×1, look_* ×1, lround_* ×1, mace_* ×1, map_* ×1, maul_* ×1, naked_* ×1, ogre_* ×1, origin_* ×1, passive_* ×1, pdemon_* ×1, popups* ×1, post_* ×1, revenant_* ×1, shortbow_* ×1, shriek_* ×1, skeleton_* ×1, sround_* ×1, staff_* ×1, succubus_* ×1, survival* ×1, sylvan_* ×1, tower_* ×1, wand_* ×1, waraxe_* ×1, waypointtypes* ×1, werewolf_* ×1
- **appearance and chargen** (25): background_desc, background_icons, background_names, backgrounds, body_tint_masks, bodyparts, chargen_preload, child_heads, dwarf_hair, dwarf_heads, elf_hair, elf_heads, eye_tints, hair_tints, head_tint_masks, human_beard, human_hair, human_heads, qunari_hair, qunari_heads, raceicons, shale_body_variation, skin_tints, tattoo_tints, tint_override
- **items** (24): armor_heavy_variation, armor_light_variation, armor_massive_variation, armor_medium_variation, armordata, arrow_variation, battleaxe_variation, bolt_variation, boots_heavy_variation, boots_light_variation, boots_massive_variation, boots_medium_variation, clothing_variation, crossbow_variation, dagger_variation, gloves_heavy_variation, gloves_light_variation, gloves_massive_variation, gloves_medium_variation, helmet_heavy_variation, helmet_light_variation, helmet_mage_variation, helmet_massive_variation, helmet_medium_variation
- **approval and party** (21): alalistair, alarcher_default, aldog, alirving, aljowan, alleliana, alloghain, almorrigan, almouse, aloghren, alrogue_default, alshale, alsten, alwarrior_default, alwizard_default, alwynne, alzevran, gift_values, heroicpartystats, motivations, party_picker
- **animation and gestures** (21): meleering1, meleering2, meleering3, meleering4, meleering5, meleering6, meleering7, meleering8, meleering9, nosynccombatanims, pose_transitions, poses, synccreatureanimations, syncdeathanimations, synclargecreatures1, synclargecreatures2, synclargecreatures3, synclargecreatures4, synclargecreatures5, syncplaceableanimations, unsyncdeathanimations
- **abilities** (10): cli_army_ds_alienage, cli_army_ds_fortroof, cli_army_ds_gatesatt, cli_army_ds_gatesdef, cli_army_ds_market, cli_army_ds_palace, cli_army_ds_redcliffe, cli_army_good_guys, spellconjuringspeeds, syncabilityanimations
- **sound** (6): internal_soundsettings, playersoundsets, sound_geometry_mat_types, sound_projects_to_load, sound_settings, ss_probabilitymodif
- **area resources and load hints** (5): highleveloadhints, loadhints, lowlevelloadhints, midleveloadhints, vlowlevelloadhints
- **placeables and states** (3): genlock_heads, hurlock_heads, placeable_interactions
- **plots, codex, journal** (3): main_plot_screenshots, main_plot_screenshots2, plotactionsets
- **toolset/test/debug** (3): toolset_dlgicons, toolset_groups, toolset_henchmen
- **crafting** (2): crafting_recipe_types, crafting_recipes
- **treasure and loot** (1): corpse_heads
- **GUI and tutorials** (1): trainingsteps

### Core tables, class "no-worksheet", by family

- **sound** (688): name families sse_* ×343, sst_* ×343, res_* ×1, sound_* ×1
- **area resources and load hints** (352): name families res_* ×351, temploadhints* ×1
- **treasure and loot** (196): name families ts_* ×190, mb_* ×5, var_* ×1
- **animation and gestures** (29): gestures_core_settings, gst_armscrossed, gst_armscrossed_left, gst_armscrossed_right, gst_crouch, gst_dog, gst_female_neutral, gst_hand_on_hip, gst_none, gst_servant, gst_sitting_chair, gst_sitting_chair_hand_tbl, gst_sitting_ground, gst_standing_180, gst_standing_back, gst_standing_drunk, gst_standing_forward, gst_standing_left, gst_standing_neutral, gst_standing_right, gst_succubus, gst_wall_lean_left, gst_wall_lean_right, gst_werewolf, halfroundcombatanims, halfroundcombatanimsb, pose_generation, synccombatanims, twitch_core_settings
- **placeables and states** (23): lock_difficulty, placeable_basetypes, statecnt_aoe, statecnt_areatransition, statecnt_bbase, statecnt_bridge, statecnt_cage, statecnt_container_static, statecnt_door_secret, statecnt_flipcover, statecnt_furniture, statecnt_informational, statecnt_nonselectable_trap, statecnt_puzzle, statecnt_selectable_trap, statecnt_static, statecnt_topballista, statecnt_trap_trigger, statecnt_trigger, trap_difficulty, useable_states, var_placeable, var_placeable_react
- **other** (21): 2da_base, autobalance, creaturetypes, diffsettings, packages_types, resistances, resource_state, simplecreature, starting_points, state_image_suffixes, terrain_types, var_ambient_wp, var_area, var_creature, var_item, var_merchant, var_module, var_trigger, var_trigger_ambient, var_trigger_talk, waypoint
- **items** (17): iprp_abilities, iprp_aligngrp, iprp_ammocost, iprp_ammotype, iprp_costtable, iprp_damagecost, iprp_damagetype, iprp_immuncost, iprp_immunity, iprp_meleecost, iprp_monstcost, iprp_paramtable, iprp_penalty, iprp_terraintype, iprp_visual, itempropdef, itemprops
- **world map and travel** (10): enc_trips_sp, enc_trips_sp_cli, location_states, rand_climax, rand_denerim, rand_forest, rand_highway, rand_mountains, rand_plains, rand_underground
- **AI** (8): ai_movetypes, aip_deepstalker_jumper, commagetacticpreset, commanddatatype, commanddatatypes, commandtargettypes, follower_ani_hack_shale, personality
- **toolset/test/debug** (6): test_casting, toolset_dlgemotions, toolset_dlglang, toolset_dlgparms, toolset_shake_settings, toolset_vars
- **GUI and tutorials** (4): camera_selection, dialog_tokens, stringtokens, tutorials
- **abilities** (1): abilityusetype
- **vfx** (1): crusts
- **plots, codex, journal** (1): plotactionstates
- **appearance and chargen** (1): statecnt_bodybag
- **crafting** (1): temp_crafting

## Resource types (extensions)

Counts are `thedas corpus` entries (each archive copy counted). "typed" means a typed reader exists and passes on the whole
corpus; "GFF only" means it parses as generic GFF3/GFF4 but nothing interprets it.

| ext | entries | parser | consumer in the engine |
| --- | --- | --- | --- |
| fxe | 308,332 | none | none (FaceFX lip-sync and facial animation, inferred; conversations have no lip sync) |
| fxa | 37 | none | none (FaceFX actors, inferred) |
| met | 2,364 | none (almost all encrypted) | none |
| dlb | 731 | GFF only | none |
| pwk | 774 | GFF only | none (placeable walkmesh, inferred; the sim uses .phy and the .arl grid) |
| lst | 320 | GFF3 only | none |
| mal | 347 | none | only named as the emitters' material library default |
| cre | 135 | none | none |
| cur | 70 | none | none (the original's mouse cursors) |
| ttf | 2 | none | none (fonts come from gfxfontlib/fonts_en-us.gfx) |
| cns, ncc, nsc, nss | 26, 24, 19, 30 | none | none (console scripts, compiled/source script debug files) |
| tbi | 11 | GFF only | none |
| fxs, fxr | 7, 7 | none (encrypted) | none |
| fdl | 4 | GFF (1) | none |
| bic | 3 | GFF3 only | none (character files) |
| map | 14 | GFF (9) | worldmap.rs reads .map names |
| spt | 688 | typed (688/688) | not drawn; billboards stand in (#311) |
| mtx | 961 | typed | used |
| wnd, gff | 116, 202 | GFF | gff: minimap layouts (`_mmi.gff`); wnd: none found |
| bik | 85 loose | header only | decoded by an external ffmpeg (see 07) |
| .das saves | n/a | none | the original's save games cannot be loaded (gaps-sim F16) |
| encrypted DLC | 45,962 entries | needs keys | DLC content without its key is not readable |

Types with typed readers that pass the whole corpus: anb, ani, are, arl, cut, dds, dlg, evt, fev, fsb, fxo, gad, gda, gfx, ldf, mao, mmh,
mop, mor, msh, mtx, ncs, plo, rml, spt, tlk, tnt, xml. GFF only but consumed through generic GFF reads: utc, uti, utp, utt, utm, stg, phy, cub, cif, wnd.

Corpus failures: 5,058 "script/undeclared-action" (non-fatal), all from #859–#862 or the two-argument CommandFly in DLC and Awakening scripts, plus old test scripts.
