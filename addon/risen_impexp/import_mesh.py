"""File > Import > Risen Model: a static `._xmsh` straight from the game's archives, by category,
with its collision.

risen-core writes the mesh as OBJ + MTL + PNG into the cache (Y up, right-handed, centimetres;
positions welded, every corner keeping its own uv and normal), and Blender's own OBJ importer
brings it in. The model's collision (`<name>_COL._xcom`), when the game has one, comes along as a
wireframe child named `<name>_COL`: edit or replace it, and Export > Risen Model writes it back.
"""

import os

import bpy
from bpy.props import BoolProperty, EnumProperty, FloatProperty, StringProperty

from . import catalog, core, materials


def _search(self, context, edit_text):
    try:
        return catalog.search(catalog.meshes(), self.category, edit_text)
    except core.CoreError:
        return []


def _import_obj(path, scale):
    before = set(bpy.data.objects)
    bpy.ops.wm.obj_import(filepath=path, global_scale=scale, forward_axis="NEGATIVE_Z", up_axis="Y")
    return [o for o in bpy.data.objects if o not in before]


def import_collision(name, scale, parent=None):
    """`<name>_COL` as a wireframe object (child of `parent`), or None when the game has none."""
    path = os.path.join(core.cache_dir(), f"{name}_COL.obj")
    try:
        out = core.run("collision", core.game_dir(), name, path)
    except core.CoreError:
        return None, None
    obs = _import_obj(path, scale)
    if not obs:
        return None, None
    col = obs[0]
    col.name = f"{name}_COL"
    col.display_type = "WIRE"
    col.hide_render = True
    col["risen_collision"] = ", ".join(out["materials"])
    if parent is not None:
        col.parent = parent
        col.matrix_parent_inverse = parent.matrix_world.inverted()
    return col, out


class RISEN_OT_import_mesh(bpy.types.Operator):
    bl_idname = "risen.import_mesh"
    bl_label = "Risen Model (._xmsh)"
    bl_description = "Статична модель з архівів гри (будівлі, локації, предмети, декор, оточення) з текстурами й колізією"
    bl_options = {"REGISTER", "UNDO"}

    category: EnumProperty(name="Категорія", items=catalog.enum_items(catalog.MESH_CATEGORIES))
    name: StringProperty(name="Модель", description="Назва ._xmsh у грі, напр. It_Wpn_BS_RuneSword", search=_search)
    with_collision: BoolProperty(name="Разом з колізією", description="Колізія моделі як каркасний об'єкт <назва>_COL, якщо в гри вона є", default=True)
    scale: FloatProperty(name="Масштаб", description="Risen рахує в сантиметрах; 0.01 = метри Blender", default=0.01, min=1e-6)

    def invoke(self, context, event):
        return context.window_manager.invoke_props_dialog(self, width=440)

    def execute(self, context):
        if not self.name:
            self.report({"ERROR"}, "Не вказано назву моделі")
            return {"CANCELLED"}
        try:
            if catalog.is_tree(self.name):
                return self.import_tree()
            out = core.run("mesh", core.game_dir(), self.name, core.cache_dir())
        except core.CoreError as e:
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        stem = out["entry"].rsplit("/", 1)[-1].split(".", 1)[0]
        obs = _import_obj(out["obj"], self.scale)
        for ob in obs:
            ob.name = stem
            ob["risen_entry"] = out["entry"]
        materials.apply_to_objects(obs, out["materials"], core.cache_dir())
        col_note = ""
        if self.with_collision and obs:
            col, c = import_collision(stem, self.scale, obs[0])
            col_note = f"; колізія {c['triangles']} трикутників ({', '.join(c['materials'])})" if col else "; колізії в гри немає"
        for w in out["warnings"]:
            self.report({"WARNING"}, w)
        self.report({"INFO"}, f"{stem}: {out['vertices']} вершин, {out['triangles']} трикутників, {len(out['materials'])} матеріалів{col_note}")
        return {"FINISHED"}

    def import_tree(self):
        out = core.run("tree", core.game_dir(), self.name, core.cache_dir())
        stem = out["entry"].rsplit("/", 1)[-1].split(".", 1)[0]
        for ob in _import_obj(out["obj"], self.scale):
            ob.name = stem
            ob["risen_speedtree"] = out["entry"]
        for w in out["warnings"]:
            self.report({"WARNING"}, w)
        self.report({"INFO"}, f"{stem}: заглушка SpeedTree ({out['form']}, {out['height_cm'] / 100:.1f} м) — для оточення; гра читає дерева зі своїх рецептів")
        return {"FINISHED"}


class RISEN_OT_import_collision(bpy.types.Operator):
    bl_idname = "risen.import_collision"
    bl_label = "Risen Collision (._xcom)"
    bl_description = "Лише колізія моделі (<назва>_COL._xcom) як каркасна сітка; назви матеріалів = поверхні (stone, wood, …)"
    bl_options = {"REGISTER", "UNDO"}

    category: EnumProperty(name="Категорія", items=catalog.enum_items(catalog.MESH_CATEGORIES))
    name: StringProperty(name="Модель", description="Назва моделі, чию колізію показати", search=_search)
    scale: FloatProperty(name="Масштаб", default=0.01, min=1e-6)

    def invoke(self, context, event):
        return context.window_manager.invoke_props_dialog(self, width=440)

    def execute(self, context):
        col, out = import_collision(self.name, self.scale)
        if col is None:
            self.report({"ERROR"}, f"{self.name}: у гри немає колізії-сітки для цієї моделі")
            return {"CANCELLED"}
        self.report({"INFO"}, f"{self.name}: колізія, {out['triangles']} трикутників ({', '.join(out['materials'])})")
        return {"FINISHED"}


def register():
    bpy.utils.register_class(RISEN_OT_import_mesh)
    bpy.utils.register_class(RISEN_OT_import_collision)


def unregister():
    bpy.utils.unregister_class(RISEN_OT_import_collision)
    bpy.utils.unregister_class(RISEN_OT_import_mesh)
