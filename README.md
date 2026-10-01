# Risen ImpExp — Blender add-on for Risen 1

Import and export **Risen 1** (Piranha Bytes, 2009, Genome engine) models, characters, animations,
collision and ground in **Blender 5.1+**.

> **Status: 0.9 beta.** Every file the add-on writes is checked against the game's own files
> (most of them byte for byte, see *How it was verified*), but the whole pipeline has not yet been
> played through in the game. Keep your game backed up and report what you find.

[Українською ↓](#українською)

## What it does

**File → Import** (straight from the game's `.pak` archives, search by name within a category):

| Entry | Game files | Comes in as |
|---|---|---|
| Risen Model | `._xmsh` + `._xmat` + `._ximg` | mesh with textures (vertices welded, uv/normal seams kept) and its collision as a wireframe `<name>_COL` child. Categories: buildings, locations, inventory items, decor and furniture, nature, trees and bushes (SpeedTree stand-ins), grass and undergrowth, water, technical |
| Risen Character | `._xmac` | armature (bones as sticks) and skinned mesh, no animation. Categories: humans — body (with a head of your choice) or head, monsters, animated objects, items |
| Risen Animations | `._xmot` | body animations onto the selected Risen skeleton: one clip or a whole category — idle, movement, attacks, defence, dialogue and gestures, sitting, sleeping, interaction, death |
| Risen Collision | `._xcom` | only the collision, as a wireframe mesh |
| Risen World Layer | `.lrent` | the placed objects of a layer, to build against |
| Risen Landscape | `Levelmesh_Landscape_01` | the island's ground, to reshape |

**File → Export** (install into the game, or build a mod package with `INSTALL.bat` / `ROLLBACK.bat`):

| Entry | Writes |
|---|---|
| Risen Model → Mod | `._xmsh`, new materials and textures, collision (the `*_COL` child or selected `*_COL` objects, else the mesh itself), the engine's resource directories |
| Risen Motion | a `._xmot` clip replacing one of the game's, picked by category (by default the clip the action came from) |
| Risen Actor | a `._xmac` — your mesh on a game skeleton (new armour, bodies), weights from vertex groups |
| Risen Landscape | the ground patched in place, and the collision sectors under your edit |

The **Risen** tab in the 3D view sidebar has all of these, plus the list of mods installed from
Blender with a button to remove each.

### Where it fits

Blender makes the *things*: items, buildings, whole location geometry, characters, animations,
textures. Placing them in the world (objects, spawns), scripts and NPC routines belong to a world
editor. That is why a world layer comes into Blender only as context, and nothing here writes
object placements.

## Install

1. Blender 5.1 or newer.
2. *Edit → Preferences → Get Extensions → ⌄ → Install from Disk…* → `risen_impexp-0.9.2.zip`
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
- **Reshape ground:** Import → Risen Landscape, move vertices (proportional editing works well), do
  not add or delete any. Export → Risen Landscape.

Scale: Risen works in centimetres; the add-on imports at 0.01 (metres) and exports back the same way.

## Limits (0.9)

- Materials of new meshes are made from one game material template (diffuse + normal); no alpha
  test, no specular maps yet.
- Actor export replaces the base actor's mesh; morph targets (faces) are dropped, so heads cannot
  be replaced yet. The skeleton always comes from a game actor.
- Facial animation (the dialogue lip-sync morphs) is not imported yet; body animations are.
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

## Build from source

```sh
cd core && cargo build --release          # risen-core.exe
BLENDER=/path/to/blender.exe tools/build_release.sh   # dist/risen_impexp-<version>.zip
```

For development, junction `addon/risen_impexp` into Blender's add-ons folder; the add-on then finds
`core/target/release/risen-core.exe` on its own. Tests: `RISEN_GAME="C:\...\Risen" cargo test --release`.

## Legal

Risen is © Piranha Bytes / Deep Silver. This add-on ships no game data; it reads the game you own.

License: GPL-3.0-or-later (see `LICENSE`).

---

## Українською

**Risen ImpExp** — аддон для Blender 5.1+: імпорт і експорт моделей, персонажів, анімацій, колізії та
рельєфу **Risen 1**. Версія **0.9 — бета**: усе, що пише аддон, звірено
з файлами гри (здебільшого байт-у-байт), але повністю в грі ще не пройдено — робіть резервну копію.

- **Імпорт** (прямо з архівів гри, пошук за назвою): моделі, персонажі з анімаціями, анімація на
  наявний скелет, колізія, шар світу (як оточення), рельєф острова.
- **Експорт** (встановити в гру або зібрати пакет мода з `INSTALL.bat` / `ROLLBACK.bat`): модель
  (заміна чи нова, з матеріалами, текстурами й колізією), анімація, персонаж/броня на скелеті гри,
  змінений рельєф разом із колізією.
- Вкладка **Risen** у бічній панелі 3D-вікна: усе це, плюс список встановлених модів з кнопкою «прибрати».

У Blender створюють *речі* (предмети, будівлі, геометрію локацій, персонажів, анімації); розставляє
їх у світі, додає спавни, скрипти й рутини NPC — редактор світу.

Встановлення: *Edit → Preferences → Get Extensions → ⌄ → Install from Disk* → zip з Releases,
у налаштуваннях аддона вказати папку гри.
