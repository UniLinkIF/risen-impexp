"""File > Import > Risen Mesh: a static `._xmsh` straight from the game's archives.

risen-core writes the mesh as OBJ + MTL + PNG into the cache (Y up, right-handed, centimetres), and
Blender's own OBJ importer brings it in; this operator only picks the mesh and sets the scale.
"""

import bpy
from bpy.props import FloatProperty, StringProperty

from . import core


def _search(self, context, edit_text):
    try:
        names = core.mesh_names()
    except core.CoreError:
        return []
    q = edit_text.lower()
    return [n for n in names if q in n.lower()][:300]


class RISEN_OT_import_mesh(bpy.types.Operator):
    bl_idname = "risen.import_mesh"
    bl_label = "Risen Mesh (._xmsh)"
    bl_description = "Статична модель з архівів гри, з текстурами й матеріалами"
    bl_options = {"REGISTER", "UNDO"}

    name: StringProperty(name="Модель", description="Назва ._xmsh у грі, напр. It_Wpn_BS_RuneSword", search=_search)
    scale: FloatProperty(name="Масштаб", description="Risen рахує в сантиметрах; 0.01 = метри Blender", default=0.01, min=1e-6)

    def invoke(self, context, event):
        return context.window_manager.invoke_props_dialog(self, width=420)

    def execute(self, context):
        if not self.name:
            self.report({"ERROR"}, "Не вказано назву моделі")
            return {"CANCELLED"}
        try:
            out = core.run("mesh", core.game_dir(), self.name, core.cache_dir())
        except core.CoreError as e:
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        before = set(bpy.data.objects)
        bpy.ops.wm.obj_import(filepath=out["obj"], global_scale=self.scale, forward_axis="NEGATIVE_Z", up_axis="Y")
        stem = out["entry"].rsplit("/", 1)[-1].split(".", 1)[0]
        for ob in set(bpy.data.objects) - before:
            ob.name = stem
            ob["risen_entry"] = out["entry"]
        for w in out["warnings"]:
            self.report({"WARNING"}, w)
        self.report({"INFO"}, f"{stem}: {out['vertices']} вершин, {out['triangles']} трикутників, {len(out['materials'])} матеріалів")
        return {"FINISHED"}




class RISEN_OT_import_collision(bpy.types.Operator):
    bl_idname = "risen.import_collision"
    bl_label = "Risen Collision (._xcom)"
    bl_description = "Колізія моделі (<назва>_COL._xcom) як сітка для перегляду; назви матеріалів = матеріали поверхні (stone, wood, …)"
    bl_options = {"REGISTER", "UNDO"}

    name: StringProperty(name="Модель", description="Назва моделі, чию колізію показати", search=_search)
    scale: FloatProperty(name="Масштаб", default=0.01, min=1e-6)

    def invoke(self, context, event):
        return context.window_manager.invoke_props_dialog(self, width=420)

    def execute(self, context):
        import os
        try:
            path = os.path.join(core.cache_dir(), f"{self.name}_COL.obj")
            out = core.run("collision", core.game_dir(), self.name, path)
        except core.CoreError as e:
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        before = set(bpy.data.objects)
        bpy.ops.wm.obj_import(filepath=path, global_scale=self.scale, forward_axis="NEGATIVE_Z", up_axis="Y")
        for ob in set(bpy.data.objects) - before:
            ob.name = f"{self.name}_COL"
            ob.display_type = "WIRE"
        self.report({"INFO"}, f"{self.name}: колізія, {out['triangles']} трикутників ({', '.join(out['materials'])})")
        return {"FINISHED"}


def register():
    bpy.utils.register_class(RISEN_OT_import_mesh)
    bpy.utils.register_class(RISEN_OT_import_collision)


def unregister():
    bpy.utils.unregister_class(RISEN_OT_import_collision)
    bpy.utils.unregister_class(RISEN_OT_import_mesh)
