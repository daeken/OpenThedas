# Mods

OpenThedas loads mods as layers on top of the game's own files. Nothing is ever copied over
the install: a mod stays where you put it, and removing it is deleting one line.

## The short version

1. Find your data folder: `Documents\OpenThedas` on Windows, `~/.openthedas` on macOS and Linux.
2. Create a text file there called `mods.txt`.
3. Put one mod per line: the path to a folder, or to the archive as you downloaded it.

```text
# Later lines win.
D:\Mods\DragonTex(4x).7z
D:\Mods\SJCs Vanilla Face Textures.7z!/Realistic Teeth/Pearly White (Default)
D:\Mods\Realistic Teeth v1-2b.7z
D:\Mods\HQ Eyebrows
```

Start the game. That is all.

## What a line can be

| Line | What happens |
| --- | --- |
| a folder | Every file in it, at any depth, is used. `.erf` and `.rim` archives inside it are opened too. |
| a `.zip` | Read where it is; nothing is unpacked. Archives inside the zip are not opened. |
| a `.7z` | Unpacked once into the mod cache (see below), then used like a folder. |
| a `.dazip` | The original's add-in package. Unpacked once into the mod cache; its add-in joins the game's DLC and its override files become a mod layer. |
| an `.erf` or `.rim` | Opened as it is. |
| `archive!/folder` | Only that folder of a `.zip` or `.7z`. Use it when a mod ships several variants with the same file names. |

Rules for the list:

- A later line beats an earlier one when both have the same file.
- Blank lines and lines starting with `#` are ignored.
- A path that is not absolute is relative to the folder `mods.txt` is in. `~/` is your home folder.
- Letter case never matters, in the list or inside a mod.
- A `.rar` has to be unpacked by hand to a folder (anywhere but the game's folder).
- A mod that is missing or broken is skipped with a line in the log; the rest still load.

## Mods you already have from the original game

If you played the original with mods, they are picked up from where the original keeps them,
with no list needed:

- `Documents\BioWare\Dragon Age\packages\core\override` (where most mods say to put files),
- `Documents\BioWare\Dragon Age\packages\core\data` and `patch`,
- `Documents\BioWare\Dragon Age\modules\Single Player\...`,
- add-ins installed under `Documents\BioWare\Dragon Age\AddIns`.

On macOS and Linux the same folders are looked for under `~/Documents/BioWare/Dragon Age`.

## What beats what

From the top:

1. your `mods.txt`, last line first;
2. the `packages\core\override` folder in your Documents;
3. the `packages\core\override` folder in the game's own folder;
4. DLC and other add-ins;
5. your Documents copies of the campaign and core folders;
6. the campaign;
7. the core game.

A mod therefore beats everything the game ships, including a level's own files.

## Seeing what is loaded

```text
thedas mods <game folder>
```

lists every mod layer in order, with how many files each has, how many of those are actually
in use, how many replace a game file and how many are hidden by a later mod. Problems (a
missing mod, an archive that could not be read) are printed first.

```text
thedas resolve <game folder> <file name>
```

shows every source of one file, the winner first.

```text
thedas addins <game folder>
```

lists the DLC and other add-ins the game knows, the one that wins shared files first, and
whether each is in use.

The game also logs one line at start saying how many files came from your Documents folders
and from the list.

## Turning mods off

Set the environment variable `THEDAS_MODS=none` to run with no mods and none of your
Documents folders, or `THEDAS_MODS=<path to another list>` to use a different list.

## The mod cache

`.7z` and `.dazip` files are unpacked once into `mod-cache` inside your data folder, and used
from there on later starts. A big texture pack takes a while and a lot of disk the first time
(a 4x texture pack of 11 GB unpacks to about 20 GB). If you change or replace the archive it
is unpacked again under a new name, and the old copy stays until you remove it:

```text
thedas mod-cache
```

lists what the cache holds, how big each folder is and whether your list still uses it, and

```text
thedas mod-cache --prune
```

deletes the ones it no longer uses. It only ever deletes its own folders inside `mod-cache`,
never a mod of yours.

## What does not work yet

- `.rar` archives, and encrypted `.7z` archives.
- Archives inside a `.zip`.
- A `.dazip` holding more than one add-in is read as if all shared the first one's settings,
  and new areas inside a `.dazip` or a Documents add-in are not found.
- The original's "offers" (its in-game DLC shop stubs) are not loaded.

The full list, with what the original does in each case, is in
[`docs/gaps/formats.md`](gaps/formats.md) under "Layering, DLC, mods".
