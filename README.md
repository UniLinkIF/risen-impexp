# Risen ImpExp — Blender add-on for Risen 1

Import and export **Risen 1** (Piranha Bytes, 2009, Genome engine) models, characters, animations,
collision and ground in **Blender 5.1+**.

> **Status: 0.9 beta.** Every file the add-on writes is checked against the game's own files
> (most of them byte for byte, see *How it was verified*), but the whole pipeline has not yet been
> played through in the game. Keep your game backed up and report what you find.

**English** · [Українська](README.uk.md)

## What it does

**File → Import** (straight from the game's `.pak` archives, search by name within a category):

| Entry | Game files | Comes in as |
|---|---|---|
| Risen Model | `._xmsh` + `._xmat` + `._ximg` | mesh with textures (vertices welded, uv/normal seams kept) and its collision as a wireframe `<name>_COL` child. Categories: buildings, locations, inventory items, decor and furniture, nature, trees and bushes (SpeedTree stand-ins), grass and undergrowth, water, technical |
| Risen Character | `._xmac` | armature (bones as sticks) and skinned mesh, no animation; a human's head is its own mesh with its nine face shapes as shape keys (blink and eight mouth shapes). Categories: humans — body (with a head of your choice) or head, monsters, animated objects, items |
| Risen Animations | `._xmot` | body animations onto the selected Risen skeleton: one clip or a whole category — idle, movement, attacks, defence, dialogue and gestures, sitting, sleeping, interaction, death |
| Risen Lip-sync | `._xmot` + `._xsnd` under `infos/` | a line of dialogue on the head's shape keys (an action), its voice in the Video Sequencer, blinks added if asked; any of the four languages |
| Risen Collision | `._xcom` | only the collision, as a wireframe mesh |
| Risen World Layer | `.lrent` | the placed objects of a layer, to build against |
| Risen Landscape | `Levelmesh_Landscape_01` | the island's ground, to reshape |

**File → Export** (install into the game, or build a mod package with `INSTALL.bat` / `ROLLBACK.bat`):

| Entry | Writes |
|---|---|
| Risen Model → Mod | `._xmsh`, new materials and textures, collision (the `*_COL` child or selected `*_COL` objects, else the mesh itself), the engine's resource directories |
| Risen Motion | a `._xmot` clip replacing one of the game's, picked by category (by default the clip the action came from) |
| Risen Actor | a `._xmac` — your mesh on a game skeleton (new armour, bodies), weights from vertex groups; a head exported alone replaces that head and keeps its shape keys as face shapes, so lip-sync still works |
| Risen Landscape | the ground patched in place, and the collision sectors under your edit |

The **Risen** tab in the 3D view sidebar has all of these, plus the list of mods installed from
Blender with a button to remove each. With a Risen armature selected it also offers an **animation
rig**: bone colours and collections, shapes to grab, and IK for arms and legs (a target and a pole
per limb). The rig only adds bones that deform nothing, so the game's skeleton stays exact: IK starts
off, switching a limb on snaps it to the current pose, motion export bakes the result, and *Bake*
puts it onto the game's bones.

### Where it fits

Blender makes the *things*: items, buildings, whole location geometry, characters, animations,
textures. Placing them in the world (objects, spawns), scripts and NPC routines belong to a world
editor. That is why a world layer comes into Blender only as context, and nothing here writes
object placements.

## Install

1. Blender 5.1 or newer.
2. *Edit → Preferences → Get Extensions → ⌄ → Install from Disk…* → `risen_impexp-0.9.5.zip`
   (from the Releases page).
3. In the add-on's preferences set **Risen game folder** (the one with `bin\Risen.exe`; the Steam
   default is filled in). Optional: a cache folder and a folder for mod packages.

The zip carries `risen-core.exe`, the native part (Windows x64). Nothing else is needed — no PhysX
SDK, no helper tools.

Mods installed from Blender set `NoPhysical=false` in `bin\mountlist_packed.ini` (a copy of the
original is kept) so the game reads loose files. Archives are never modified.

## Quick start

- **Replace a sword:** Import → Risen Model → `It_Wpn_BS_RuneSword`. Edit it, keep the grip at the
  origin (blade along +Z in game space, it shows as +Z up in Blender). Export → Risen Model → Mod,
  same name, *Install*. Start the game.
- **New armour:** Import → Risen Character → `Ani_Hero_Armor_Player`. Sculpt or
  replace the mesh, keep it skinned to the armature (vertex groups named after the bones).
  Export → Risen Actor, same name.
- **Change an animation:** Import → Risen Character → the creature, select its armature, Import →
  Risen Animations → a category (say *Attacks*). Edit an action, Export → Risen Motion: the clip it
  came from is preselected.
- **Animate by hand:** select the armature, *Risen* tab → *Animation rig*, switch on IK for the limbs
  you want, key the `RIG_IK_*` targets. Export → Risen Motion as usual.
- **Make a character talk:** select the character, Import → Risen Lip-sync, pick a line (words of
  its name, e.g. `PC_Hero` for the player). Play with sound on.
- **New head:** change the head mesh (and its shape keys if you like), select only the head,
  Export → Risen Actor with the head's name.
- **Reshape ground:** Import → Risen Landscape, move vertices (proportional editing works well), do
  not add or delete any. Export → Risen Landscape.

Scale: Risen works in centimetres; the add-on imports at 0.01 (metres) and exports back the same way.

## Limits (0.9.3)

- Materials of new meshes are made from game material templates: opaque, opaque with specular, or
  alpha test (cut-off from the Alpha link); no other shader effects.
- A new actor material always gets a skinned shader (the hero's, with specular or alpha test); missing normal or
  specular maps are written flat.
- The skeleton always comes from a game actor. A replaced head keeps the face shapes it has keys
  for; normal changes of the shapes are not stored (the game's own heads have none either).
- Lip-sync is imported onto the face; writing new lip-sync clips is not supported yet.
- Trees and bushes are SpeedTree recipes the game grows itself: they come in as stand-ins of the
  right size and look, for building around, and are not written back.
- Collision is written as triangle meshes (no convex hulls). Console (big-endian) files are not read.
- A new model or actor shows in the game only once something places or references it (the editor).
- Only the Steam/GOG PC version has been used (`risen-core` reads its `.pak` archives).

## How it was verified

Without the game running, the game's own files are the test (`cargo test` with `RISEN_GAME` set):

- the generic property reader/writer re-writes all 3 523 meshes, materials and textures byte for byte;
- every record of the engine's resource directories (meshes, materials, images, collision, motions)
  is re-created identically from its file;
- our `._xmsh` writer reproduces a mesh already proven in the game byte for byte;
- 635 of 637 collision files (1 087 PhysX streams) re-write byte for byte; the cooker's trees,
  bounds and mass properties match the engine's, convex-edge flags 99.99 %;
- all 106 little-endian actors re-write byte for byte; an actor re-built through the exporter keeps
  its faces, UVs and weights; a clip goes game → Blender → game within 1e-6;
- the landscape path gives the same bytes as the height edit that already runs in the game.
- the face shapes of all 14 heads read and re-write within the 16-bit steps; a head re-built through
  the exporter keeps all nine; a hand moved by IK comes back from the exported clip where it was put.

## Build from source

```sh
cd core && cargo build --release          # risen-core.exe
BLENDER=/path/to/blender.exe tools/build_release.sh   # dist/risen_impexp-<version>.zip
```

For development, junction `addon/risen_impexp` into Blender's add-ons folder; the add-on then finds
`core/target/release/risen-core.exe` on its own. Tests: `RISEN_GAME="C:\...\Risen" cargo test --release`.

## License

Copyright © 2026 UniLinkIF. Risen ImpExp is free software under the **GNU GPL 3.0 or later** (`LICENSE`) **with
additional terms** under its section 7 (`NOTICE`), which go with every copy and every modified version:

- **Attribution:** keep `NOTICE`, the copyright line and the attribution "Risen ImpExp by UniLinkIF" with the link to
  this repository (in the add-on's preferences and in the documentation).
- **Origin:** a modified version must say it is modified and by whom, carry a different name and version, and must
  not be presented as the original or as made or endorsed by UniLinkIF.
- **Names:** no rights to the names "Risen ImpExp" or "UniLinkIF" for forks, products or publicity.

Risen is © Piranha Bytes / Deep Silver. This add-on ships no game data and is not affiliated with them; it reads the
game you own.

