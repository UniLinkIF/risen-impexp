"""Add-on preferences.

Risen keeps everything in .pak archives that the core reads directly, so one game folder is enough;
the directory list only remains for your own PNGs that should override game textures.
"""

import bpy
from bpy.props import BoolProperty, CollectionProperty, IntProperty, StringProperty

ADDON_ID = __package__


class RISEN_PG_TextureDir(bpy.types.PropertyGroup):
    path: StringProperty(name="Path", subtype="DIR_PATH")


class RISEN_UL_TextureDirs(bpy.types.UIList):
    def draw_item(self, context, layout, data, item, icon, active_data, active_propname, index):
        layout.prop(item, "path", text="", emboss=False)


class RISEN_OT_TextureDirAdd(bpy.types.Operator):
    bl_idname = "risen.texture_dir_add"
    bl_label = "Додати папку"

    def execute(self, context):
        p = prefs(context)
        p.texture_dirs.add()
        p.texture_dir_index = len(p.texture_dirs) - 1
        return {"FINISHED"}


class RISEN_OT_TextureDirRemove(bpy.types.Operator):
    bl_idname = "risen.texture_dir_remove"
    bl_label = "Прибрати папку"

    def execute(self, context):
        p = prefs(context)
        if 0 <= p.texture_dir_index < len(p.texture_dirs):
            p.texture_dirs.remove(p.texture_dir_index)
            p.texture_dir_index = max(0, p.texture_dir_index - 1)
        return {"FINISHED"}


class RisenImpExpPreferences(bpy.types.AddonPreferences):
    bl_idname = ADDON_ID

    game_dir: StringProperty(
        name="Папка гри Risen",
        description="Папка з bin\\Risen.exe; текстури, матеріали й моделі читаються з архівів гри напряму",
        subtype="DIR_PATH",
        default=r"C:\Program Files (x86)\Steam\steamapps\common\Risen",
    )
    core_exe: StringProperty(
        name="Ядро (risen-core.exe)",
        description="Нативне ядро, що читає й пише формати Risen",
        subtype="FILE_PATH",
    )
    cache_dir: StringProperty(
        name="Кеш",
        description="Куди складати розпаковані текстури й моделі, щоб повторний імпорт був миттєвим",
        subtype="DIR_PATH",
    )
    mods_dir: StringProperty(
        name="Папка модів",
        description="Куди «Зібрати мод» кладе пакет з INSTALL.bat / ROLLBACK.bat",
        subtype="DIR_PATH",
    )
    write_log: BoolProperty(name="Писати лог", default=True)
    search_textures: BoolProperty(name="Шукати текстури при кожному імпорті", default=True)
    texture_dirs: CollectionProperty(type=RISEN_PG_TextureDir)
    texture_dir_index: IntProperty()

    def draw(self, context):
        col = self.layout.column()
        col.prop(self, "game_dir")
        col.prop(self, "core_exe")
        col.prop(self, "cache_dir")
        col.prop(self, "mods_dir")
        col.separator()
        col.prop(self, "write_log")
        col.prop(self, "search_textures")
        col.separator()
        col.label(text="Додаткові папки текстур (перекривають текстури гри):")
        row = col.row()
        row.template_list("RISEN_UL_TextureDirs", "", self, "texture_dirs", self, "texture_dir_index", rows=4)
        ops = row.column(align=True)
        ops.operator("risen.texture_dir_add", icon="ADD", text="")
        ops.operator("risen.texture_dir_remove", icon="REMOVE", text="")


def prefs(context=None):
    context = context or bpy.context
    return context.preferences.addons[ADDON_ID].preferences


_classes = (RISEN_PG_TextureDir, RISEN_UL_TextureDirs, RISEN_OT_TextureDirAdd, RISEN_OT_TextureDirRemove, RisenImpExpPreferences)


def register():
    for c in _classes:
        bpy.utils.register_class(c)


def unregister():
    for c in reversed(_classes):
        bpy.utils.unregister_class(c)
