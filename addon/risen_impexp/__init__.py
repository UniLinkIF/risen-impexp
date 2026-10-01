"""Risen ImpExp: Blender import/export for Risen 1 (Piranha Bytes, Genome engine).

Blender side only: UI, preferences, menu entries and
the hand-over to the native core (``risen-core``, Rust; reads the game's .pak archives directly and
writes Risen formats), which converts through glTF so Blender's own importer/exporter does the
skeleton and animation work.

Works both as a legacy add-on (bl_info) and as a Blender 5.x extension
(blender_manifest.toml).
"""

bl_info = {
    "name": "Risen ImpExp",
    "author": "BlenderMod",
    "version": (0, 9, 2),
    "blender": (5, 1, 0),
    "location": "File > Import / Export > Risen",
    "description": "Import and export Risen 1 models, actors, motions, collision and ground",
    "category": "Import-Export",
}

from . import prefs, import_mesh, import_actor, export_mesh, export_motion, export_actor, world, ui, menus  # noqa: E402

_modules = (prefs, import_mesh, import_actor, export_mesh, export_motion, export_actor, world, ui, menus)


def register():
    for m in _modules:
        m.register()


def unregister():
    for m in reversed(_modules):
        m.unregister()
