# Testing with mod mode

Mod mode is a set of panels over the running game. Use it to check animations frame by frame
and to play any conversation from any point. Everything here works on the game as you are
playing it: no editor, no reload.

## Opening it

- Press **F10**, or choose **Mod Mode** in the game menu (Escape).
- Press F10 again to close it. Closing lets go of anything you were holding.
- While a panel has the mouse, clicks and keys go to the panel, not the game.

## Picking what to look at

- **Ctrl+click** a creature or object in the world, or press **Pick** and then click it.
- The panel shows what it is (tag, template, area, position, conversation) and what it is
  animating: its animation tree, the action, the `.ani` file and the frame.
- **Fly** frees the camera: WASD to move, Q/E down and up, Shift to go faster, the right mouse
  button to look. **Fly to** puts the camera in front of the picked creature.

## Checking an animation

1. Pick the creature.
2. Press **Hold this animation**. The creature freezes on the animation it is playing. Only
   what you see stops; the game itself carries on.
3. To check another of its animations, open **Play another animation**, press **List its
   animations**, type part of a name in **Find** and click one. The creature plays it from
   the start.
4. Step through it:
   - `|<` and `>|` jump to the first and last frame.
   - `<` and `>` (or the **,** and **.** keys) go one frame back or on.
   - **Play** plays it at the chosen **speed** (1/10, 1/4, 1/2 or 1).
   - The slider jumps to any frame.
   - **Loop from here** then **End here** repeats a stretch.
5. **Let go** (or Escape) returns the creature to the game.

## Reporting bad frames

- **Report this frame** files a report about the frame on screen. The report names the
  creature, its template, the area, the animation file, the frame and the action, and has
  a screenshot.
- To report several frames at once, press **Mark frame** on each bad one, then **Report N
  marked frames**. That files one report listing every marked frame. **Clear marks** starts
  over.
- Frame numbers count 30 frames a second.

## Playing a conversation out of order

1. Press **Conversations** in the Mod mode panel.
2. Type part of a conversation's name in **Find** (for example `alistair`) and click it.
3. Choose who speaks it in **owner**. The list starts with the creatures in this area whose
   conversation it is, plus whatever you have picked. **Any speaker** adds every creature
   here.
4. **Start** plays it from its normal beginning, as if you had talked to the speaker.
5. To start somewhere else, click **Start here** on any line. The conversation begins at
   that line even if its condition says no. Lines after it check their conditions as usual.
6. Each line shows whether it can be used now (`+` or `-`) and the plot flags its condition
   checks. Click **set** or **clear** next to a flag to change it, then **Check again**.
7. **History** lists what you changed. **Undo** puts a flag back.

**Show** chooses which lines are listed: the starts and lines with conditions (the default),
only the starts, or every line.

## Looking at the world's state

**World state** shows the game's plot flags and variables: the module's (the game's
globals), the area's and the picked creature's. Type in **Filter** to find one. You can
change, add or remove values there, and undo them from **History**. Use it to set up a
situation (a quest flag, a variable a script checks) before testing a scene.

Under **Inspected**, a picked creature also shows its properties (strength, health and the
rest: base, current, what effects add, and the total) and the effects on it. **edit** changes
a property's base, or the current value of health, mana or stamina. Undo puts it back.

## Good to know

- Holding or playing an animation changes only what is drawn, never the game.
- Flags and variables you change, conversations you start and objects you move are real
  changes to the game you are playing. They are saved with it, and undo is there for them.
- If something looks wrong, a report from mod mode is the quickest way to show exactly
  where.
