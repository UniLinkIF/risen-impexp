"""3D View sidebar tab "Risen": the import/export entries and the mods installed from Blender."""

import bpy
from bpy.props import StringProperty

from . import core

# Mod name → files, as the core last reported it (asking opens the game archives, ~1 s, so it is
# refreshed on demand and after every install/uninstall, never from draw()).
_installed = None


def refresh():
    global _installed
    try:
        _installed = core.run("installed", core.game_dir())
    except core.CoreError:
        _installed = None


class RISEN_OT_refresh_mods(bpy.types.Operator):
    bl_idname = "risen.refresh_mods"
    bl_label = "Оновити список"
    bl_description = "Перечитати, які моди встановлено з Blender"

    def execute(self, context):
        refresh()
        return {"FINISHED"}


class RISEN_OT_uninstall_mod(bpy.types.Operator):
    bl_idname = "risen.uninstall_mod"
    bl_label = "Прибрати мод"
    bl_description = "Видалити файли мода з гри й перебудувати довідники ресурсів"

    mod: StringProperty()

    def invoke(self, context, event):
        return context.window_manager.invoke_confirm(self, event)

    def execute(self, context):
        try:
            out = core.run("uninstall", core.game_dir(), self.mod)
        except core.CoreError as e:
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        refresh()
        self.report({"INFO"}, f"{self.mod}: прибрано ({len(out)} файлів)")
        return {"FINISHED"}


class RISEN_PT_main(bpy.types.Panel):
    bl_label = "Risen"
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "Risen"

    def draw(self, context):
        col = self.layout.column(align=True)
        col.operator("risen.import_mesh", icon="IMPORT")
        col.operator("risen.import_actor", icon="ARMATURE_DATA")
        col.operator("risen.import_motion", icon="ACTION")
        col.operator("risen.import_world", icon="WORLD")
        col.operator("risen.import_landscape", icon="MESH_GRID")
        col.operator("risen.export_mesh", icon="EXPORT")
        col.operator("risen.export_motion", icon="ACTION")
        col.operator("risen.export_actor", icon="OUTLINER_OB_ARMATURE")
        col.operator("risen.export_landscape", icon="MESH_GRID")
        box = self.layout.box()
        row = box.row()
        row.label(text="Встановлені моди", icon="PACKAGE")
        row.operator("risen.refresh_mods", text="", icon="FILE_REFRESH")
        if _installed is None:
            box.label(text="Натисніть ⟳, щоб прочитати")
        elif not _installed:
            box.label(text="Немає")
        else:
            for name, files in _installed:
                r = box.row()
                r.label(text=f"{name} ({len(files)})")
                r.operator("risen.uninstall_mod", text="", icon="X").mod = name


_classes = (RISEN_OT_refresh_mods, RISEN_OT_uninstall_mod, RISEN_PT_main)


def register():
    for c in _classes:
        bpy.utils.register_class(c)


def unregister():
    for c in reversed(_classes):
        bpy.utils.unregister_class(c)
