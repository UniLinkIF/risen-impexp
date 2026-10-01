"""File > Import / Export entries, one per Risen format."""

import bpy

IMPORTS = (
    "risen.import_mesh",
    "risen.import_character",
    "risen.import_motion",
    "risen.import_lipsync",
    "risen.import_collision",
    "risen.import_world",
    "risen.import_landscape",
)
EXPORTS = (
    "risen.export_mesh",
    "risen.export_motion",
    "risen.export_actor",
    "risen.export_landscape",
)


def _menu_import(self, context):
    self.layout.separator()
    for idname in IMPORTS:
        self.layout.operator(idname)


def _menu_export(self, context):
    self.layout.separator()
    for idname in EXPORTS:
        self.layout.operator(idname)


def register():
    bpy.types.TOPBAR_MT_file_import.append(_menu_import)
    bpy.types.TOPBAR_MT_file_export.append(_menu_export)


def unregister():
    bpy.types.TOPBAR_MT_file_export.remove(_menu_export)
    bpy.types.TOPBAR_MT_file_import.remove(_menu_import)
