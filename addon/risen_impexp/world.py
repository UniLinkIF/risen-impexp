"""The world in Blender: a placed layer (`.lrent`) as context to build against, and the island's
ground (landscape) to reshape and write back.

Placing objects in the game is a world editor's job; a layer comes in here only so new
models can be made to fit where they will stand. The landscape round trip is in place: move its
vertices (heights), keep their number — the core patches the game's mesh and the collision under it.
"""

import json
import os
import tempfile

import bpy
import numpy as np
from bpy.props import EnumProperty, FloatProperty, StringProperty
from mathutils import Matrix, Vector

from . import core
from .prefs import prefs

_layers = None


def _search_layer(self, context, edit_text):
    global _layers
    try:
        if _layers is None:
            _layers = core.run("layers", core.game_dir(), "")
    except core.CoreError:
        return []
    q = edit_text.lower()
    return [n for n in _layers if q in n.lower()][:300]


# Blender (x, y, z) = game (x, z, y) × scale: a swap of Y and Z (see the mesh importer).
_P = Matrix(((1, 0, 0), (0, 0, 1), (0, 1, 0)))


def game_matrix_to_blender(m, scale):
    """A layer matrix (row vectors: world = local · M, translation in [12..15]) → Blender's."""
    r = Matrix(((m[0], m[4], m[8]), (m[1], m[5], m[9]), (m[2], m[6], m[10])))  # column form (Mᵀ)
    rot = _P @ r @ _P
    t = (_P @ Vector((m[12], m[13], m[14]))) * scale
    out = rot.to_4x4()
    out.translation = t
    return out


class RISEN_OT_import_world(bpy.types.Operator):
    bl_idname = "risen.import_world"
    bl_label = "Risen World Layer (.lrent)"
    bl_description = "Розстановка шару світу як оточення: будуйте нове під місце, де воно стоятиме (розставляє редактор)"
    bl_options = {"REGISTER", "UNDO"}

    name: StringProperty(name="Шар", description="Напр. Monastery_Dyn, Harbour_Town_Dyn", search=_search_layer)
    scale: FloatProperty(name="Масштаб", default=0.01, min=1e-6)

    def invoke(self, context, event):
        return context.window_manager.invoke_props_dialog(self, width=420)

    def execute(self, context):
        out_dir = os.path.join(core.cache_dir(), "world")
        try:
            lay = core.run("world-layer", core.game_dir(), self.name, out_dir)
        except core.CoreError as e:
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        col = bpy.data.collections.new(self.name)
        context.scene.collection.children.link(col)
        data = {}
        for stem, obj in lay["meshes"].items():
            before = set(bpy.data.objects)
            bpy.ops.wm.obj_import(filepath=obj, global_scale=self.scale, forward_axis="NEGATIVE_Z", up_axis="Y")
            new = list(set(bpy.data.objects) - before)
            if new:
                # The OBJ importer puts its scale and axis turn on the object, not in the mesh.
                data[stem] = (new[0].data, new[0].matrix_world.copy())
                for o in new:
                    bpy.data.objects.remove(o, do_unlink=True)
        placed = markers = 0
        for e in lay["entities"]:
            me, fix = data.get(e["mesh"], (None, Matrix.Identity(4))) if e["kind"] == "mesh" else (None, Matrix.Identity(4))
            ob = bpy.data.objects.new(e["name"], me)
            if me is None:
                ob.empty_display_type = "PLAIN_AXES" if e["kind"] == "none" else "CUBE"
                ob.empty_display_size = 0.3
                markers += 1
            else:
                placed += 1
            ob.matrix_world = game_matrix_to_blender(e["matrix"], self.scale) @ fix
            ob["risen_entity"] = e["name"]
            ob["risen_kind"] = e["kind"]
            col.objects.link(ob)
        for w in lay["warnings"][:5]:
            self.report({"WARNING"}, w)
        self.report({"INFO"}, f"{self.name}: {placed} моделей ({len(data)} різних), {markers} маркерів/персонажів/дерев")
        return {"FINISHED"}


class RISEN_OT_import_landscape(bpy.types.Operator):
    bl_idname = "risen.import_landscape"
    bl_label = "Risen Landscape"
    bl_description = "Рельєф острова (Levelmesh_Landscape_01) — змінюйте висоти, не додаючи й не видаляючи вершин"
    bl_options = {"REGISTER", "UNDO"}

    scale: FloatProperty(name="Масштаб", default=0.01, min=1e-6)

    def execute(self, context):
        out_dir = os.path.join(core.cache_dir(), "landscape")
        try:
            r = core.run("landscape-get", core.game_dir(), out_dir)
        except core.CoreError as e:
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        d = np.fromfile(r["geometry"], dtype=np.uint8)
        nv = int(d[:4].view(np.uint32)[0])
        o = 4
        pos = d[o:o + nv * 12].view(np.float32).reshape(-1, 3)
        o += nv * 12
        nt = int(d[o:o + 4].view(np.uint32)[0])
        o += 4
        tris = d[o:o + nt * 12].view(np.uint32)
        o += nt * 12
        uv = d[o:o + nt * 24].view(np.float32)
        o += nt * 24
        sub = d[o:o + nt * 4].view(np.uint32)
        me = bpy.data.meshes.new("Levelmesh_Landscape_01")
        me.vertices.add(nv)
        me.vertices.foreach_set("co", (pos[:, [0, 2, 1]] * self.scale).ravel())
        me.loops.add(nt * 3)
        me.loops.foreach_set("vertex_index", tris.astype(np.int32))
        me.polygons.add(nt)
        me.polygons.foreach_set("loop_start", np.arange(0, nt * 3, 3, dtype=np.int32))
        me.polygons.foreach_set("material_index", sub.astype(np.int32))
        layer = me.uv_layers.new(name="UVMap")
        layer.data.foreach_set("uv", uv)
        for m in r["materials"]:
            mat = bpy.data.materials.new(m["name"].split(".")[0])
            mat.use_nodes = True
            if m.get("diffuse"):
                t = mat.node_tree.nodes.new("ShaderNodeTexImage")
                t.image = bpy.data.images.load(os.path.join(out_dir, m["diffuse"]), check_existing=True)
                mat.node_tree.links.new(t.outputs["Color"], mat.node_tree.nodes["Principled BSDF"].inputs["Base Color"])
            me.materials.append(mat)
        me.update()
        me.validate(clean_customdata=False)
        ob = bpy.data.objects.new("Levelmesh_Landscape_01", me)
        ob["risen_landscape"] = nv
        context.scene.collection.objects.link(ob)
        self.report({"INFO"}, f"Рельєф: {nv} вершин, {nt} трикутників, {len(r['materials'])} матеріалів ґрунту")
        return {"FINISHED"}


class RISEN_OT_export_landscape(bpy.types.Operator):
    bl_idname = "risen.export_landscape"
    bl_label = "Risen Landscape"
    bl_description = "Змінений рельєф → гра: меш на місці і колізія під ним"

    mode: EnumProperty(name="Куди", items=(
        ("install", "Встановити в гру", "Одразу в папку гри; прибрати — панель Risen"),
        ("package", "Пакет мода", "Папка з files/, INSTALL.bat і ROLLBACK.bat"),
    ), default="install")
    folder: StringProperty(name="Папка пакета", subtype="DIR_PATH")
    scale: FloatProperty(name="Масштаб", default=0.01, min=1e-6)

    @classmethod
    def poll(cls, context):
        o = context.active_object
        return o is not None and o.type == "MESH" and "risen_landscape" in o

    def invoke(self, context, event):
        return context.window_manager.invoke_props_dialog(self, width=380)

    def execute(self, context):
        ob = context.active_object
        me = ob.data
        if len(me.vertices) != ob["risen_landscape"]:
            self.report({"ERROR"}, f"У рельєфі {len(me.vertices)} вершин, а було {ob['risen_landscape']}: рухайте вершини, не додаючи й не видаляючи")
            return {"CANCELLED"}
        co = np.empty(len(me.vertices) * 3, np.float32)
        me.vertices.foreach_get("co", co)
        co = co.reshape(-1, 3)
        world = np.array(ob.matrix_world, dtype=np.float64)
        co = (np.c_[co, np.ones(len(co))] @ world.T)[:, :3]
        game = (co / self.scale)[:, [0, 2, 1]].astype(np.float32)
        tmp = tempfile.mkdtemp(prefix="risen_land_")
        path = os.path.join(tmp, "positions.bin")
        with open(path, "wb") as f:
            f.write(np.uint32(len(game)).tobytes())
            f.write(np.ascontiguousarray(game).tobytes())
        try:
            if self.mode == "package":
                folder = bpy.path.abspath(self.folder) if self.folder else os.path.join(prefs().mods_dir or tmp, "Landscape")
                out = core.run("landscape-set", core.game_dir(), path, "package", folder, "Landscape")
            else:
                out = core.run("landscape-set", core.game_dir(), path, "install", "Landscape")
                from . import ui
                ui.refresh()
        except core.CoreError as e:
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        r = out["report"]
        self.report({"INFO"}, f"Рельєф: {r['moved_vertices']} вершин зсунуто (до {r['max_offset_cm']:.0f} см), колізія: {len(r['sectors'])} секторів")
        return {"FINISHED"}


_classes = (RISEN_OT_import_world, RISEN_OT_import_landscape, RISEN_OT_export_landscape)


def register():
    for c in _classes:
        bpy.utils.register_class(c)


def unregister():
    for c in reversed(_classes):
        bpy.utils.unregister_class(c)
