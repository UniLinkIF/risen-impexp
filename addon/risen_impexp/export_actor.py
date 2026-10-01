"""File > Export > Risen Actor: a skinned mesh on a Risen skeleton becomes an actor (`._xmac`).

Select the mesh(es) bound to a skeleton imported as Risen Actor (Armature modifier; weights in
vertex groups named after the bones). The core keeps the base actor's skeleton and everything
else, and swaps in this mesh, its skin and its materials — a new armour or body. Same name as
the base = replace it, a new name = a new actor beside it.

A head brought in with a character is its own actor: exported alone, it replaces that head, and
its shape keys (BS_Blink, the mouth shapes) go back as its face shapes, so lip-sync still works.
"""

import json
import os
import tempfile

import bpy
import numpy as np
from bpy.props import EnumProperty, FloatProperty, StringProperty

from . import core
from .export_mesh import _material_spec
from .prefs import prefs


def _armature_of(ob):
    for m in ob.modifiers:
        if m.type == "ARMATURE" and m.object is not None and "risen_actor" in m.object:
            return m.object
    p = ob.parent
    return p if p is not None and p.type == "ARMATURE" and "risen_actor" in p else None


def gather_skinned(context, objects, scale, tmp):
    """Rest-pose geometry of `objects` (armature deformation off, other modifiers on) in game
    space, per-vertex bone weights (top 4) and materials."""
    arms = [m for o in objects for m in o.modifiers if m.type == "ARMATURE"]
    saved = [(m, m.show_viewport) for m in arms]
    for m, _ in saved:
        m.show_viewport = False
    # Shape keys at rest: the mesh goes out as its basis, each key as a delta from it.
    keyed = [(o, o.show_only_shape_key, o.active_shape_key_index) for o in objects if o.data.shape_keys]
    for o, _, _ in keyed:
        o.show_only_shape_key, o.active_shape_key_index = True, 0
    shapes, counts = {}, []
    try:
        dg = context.evaluated_depsgraph_get()
        dg.update()
        pos, cv, nrm, uv, mat, weights = [], [], [], [], [], []
        bones, bone_of = [], {}
        materials, slot_of = [], {}
        vbase = 0
        for ob in objects:
            ev = ob.evaluated_get(dg)
            me = ev.to_mesh()
            try:
                me.transform(ob.matrix_world)
                me.calc_loop_triangles()
                nt, nv = len(me.loop_triangles), len(me.vertices)
                if nt == 0:
                    continue
                loops = np.empty(nt * 3, np.int32)
                me.loop_triangles.foreach_get("loops", loops)
                mi = np.empty(nt, np.int32)
                me.loop_triangles.foreach_get("material_index", mi)
                vidx = np.empty(len(me.loops), np.int32)
                me.loops.foreach_get("vertex_index", vidx)
                co = np.empty(nv * 3, np.float32)
                me.vertices.foreach_get("co", co)
                cn = np.empty(len(me.loops) * 3, np.float32)
                me.corner_normals.foreach_get("vector", cn)
                u = np.zeros(len(me.loops) * 2, np.float32)
                if me.uv_layers.active:
                    me.uv_layers.active.data.foreach_get("uv", u)
                co, cn, u = co.reshape(-1, 3), cn.reshape(-1, 3), u.reshape(-1, 2)
                pos.append((co / scale)[:, [0, 2, 1]])
                cv.append(vidx[loops] + vbase)
                nrm.append(cn[loops][:, [0, 2, 1]])
                t = u[loops]
                uv.append(np.stack([t[:, 0], 1.0 - t[:, 1]], axis=1))
                slots = [s.material for s in ob.material_slots] or [None]
                remap = []
                for m in slots:
                    key = m.name if m else "Default"
                    if key not in slot_of:
                        slot_of[key] = len(materials)
                        materials.append(_material_spec(m, tmp))
                    remap.append(slot_of[key])
                mat.append(np.array(remap, np.uint32)[np.clip(mi, 0, len(remap) - 1)])
                # Weights: the evaluated mesh keeps vertex groups when its vertex count is the original's.
                groups = {g.index: g.name for g in ob.vertex_groups}
                src = me if len(me.vertices) == nv else ob.data
                for v in src.vertices:
                    w = sorted(((g.weight, groups.get(g.group)) for g in v.groups if g.weight > 0 and groups.get(g.group)), reverse=True)[:4]
                    row = []
                    for weight, name in w:
                        if name not in bone_of:
                            bone_of[name] = len(bones)
                            bones.append(name)
                        row += [bone_of[name], weight]
                    while len(row) < 8:
                        row += [0, 0.0]
                    weights.append(row)
                counts.append(nv)
                keys = ob.data.shape_keys
                if keys and len(ob.data.vertices) == nv:
                    rot = np.array(ob.matrix_world.to_3x3(), np.float32)
                    basis = np.empty(nv * 3, np.float32)
                    keys.reference_key.data.foreach_get("co", basis)
                    for kb in keys.key_blocks:
                        if kb == keys.reference_key:
                            continue
                        c = np.empty(nv * 3, np.float32)
                        kb.data.foreach_get("co", c)
                        d = ((c - basis).reshape(-1, 3) @ rot.T) / scale
                        shapes.setdefault(kb.name, {})[len(counts) - 1] = d[:, [0, 2, 1]]
                vbase += nv
            finally:
                ev.to_mesh_clear()
    finally:
        for m, show in saved:
            m.show_viewport = show
        for o, only, idx in keyed:
            o.show_only_shape_key, o.active_shape_key_index = only, idx
    if not pos:
        return None
    wb = np.zeros((len(weights), 4), np.uint32)
    ww = np.zeros((len(weights), 4), np.float32)
    for i, row in enumerate(weights):
        for k in range(4):
            wb[i, k], ww[i, k] = row[2 * k], row[2 * k + 1]
    packed = np.empty((len(weights), 8), np.uint32)
    packed[:, 0::2] = wb
    packed[:, 1::2] = ww.view(np.uint32)
    return (np.concatenate(pos).astype(np.float32), np.concatenate(cv).astype(np.uint32), np.concatenate(nrm).astype(np.float32),
            np.concatenate(uv).astype(np.float32), np.concatenate(mat).astype(np.uint32), packed, bones, materials,
            {n: np.concatenate([per.get(i, np.zeros((c, 3), np.float32)) for i, c in enumerate(counts)]).astype(np.float32) for n, per in shapes.items()})


def _base_of(objects):
    """The head actor when every object is a head brought in with a character (its own actor in
    the game, face shapes and all), else None: the armature's actor is the base."""
    heads = {o.get("risen_head") for o in objects}
    return heads.pop() if len(heads) == 1 and None not in heads else None


class RISEN_OT_export_actor(bpy.types.Operator):
    bl_idname = "risen.export_actor"
    bl_label = "Risen Actor (._xmac)"
    bl_description = "Виділені меші на скелеті Risen → актор гри (новий обладунок, тіло); скелет і решта — з базового актора"

    name: StringProperty(name="Назва актора", description="Назва базового актора — заміна (напр. Ani_Hero_Armor_Player); нова назва — новий актор")
    mode: EnumProperty(name="Куди", items=(
        ("install", "Встановити в гру", "Одразу в папку гри; прибрати — панель Risen"),
        ("package", "Пакет мода", "Папка з files/, INSTALL.bat і ROLLBACK.bat"),
    ), default="install")
    folder: StringProperty(name="Папка пакета", subtype="DIR_PATH")
    title: StringProperty(name="Назва мода")
    scale: FloatProperty(name="Масштаб", default=0.01, min=1e-6)

    @classmethod
    def poll(cls, context):
        return any(o.type == "MESH" and _armature_of(o) for o in context.selected_objects)

    def invoke(self, context, event):
        arm = next(_armature_of(o) for o in context.selected_objects if o.type == "MESH" and _armature_of(o))
        if not self.name:
            self.name = _base_of([o for o in context.selected_objects if o.type == "MESH" and _armature_of(o)]) or arm["risen_actor"]
        return context.window_manager.invoke_props_dialog(self, width=480)

    def draw(self, context):
        col = self.layout.column()
        col.prop(self, "name")
        col.prop(self, "mode", expand=True)
        if self.mode == "package":
            col.prop(self, "folder")
            col.prop(self, "title")

    def execute(self, context):
        objects = [o for o in context.selected_objects if o.type == "MESH" and _armature_of(o)]
        arms = {_armature_of(o)["risen_actor"] for o in objects}
        if len(arms) != 1:
            self.report({"ERROR"}, f"Виділені меші мають бути на одному скелеті Risen (зараз: {', '.join(sorted(arms)) or 'жодного'})")
            return {"CANCELLED"}
        base = _base_of(objects) or arms.pop()
        tmp = tempfile.mkdtemp(prefix="risen_actor_")
        g = gather_skinned(context, objects, self.scale, tmp)
        if g is None:
            self.report({"ERROR"}, "У виділених мешах немає трикутників")
            return {"CANCELLED"}
        pos, cv, nrm, uv, mat, packed, bones, materials, shapes = g
        if not bones:
            self.report({"ERROR"}, "Жодна вершина не має ваг кісток (групи вершин з назвами кісток)")
            return {"CANCELLED"}
        geo = os.path.join(tmp, "skin.bin")
        with open(geo, "wb") as f:
            for a in (pos, cv, nrm, uv, mat, packed):
                f.write(np.ascontiguousarray(a).tobytes())
        spec = os.path.join(tmp, "spec.json")
        with open(spec, "w", encoding="utf-8") as f:
            morphs = []
            for i, (n, d) in enumerate(shapes.items()):
                p = os.path.join(tmp, f"shape_{i}.bin")
                np.ascontiguousarray(d).tofile(p)
                morphs.append({"name": n, "file": p})
            json.dump({"base": base, "name": self.name, "geometry": geo, "vertices": len(pos), "corners": len(cv), "bones": bones, "materials": materials, "morphs": morphs}, f)
        try:
            if self.mode == "package":
                folder = bpy.path.abspath(self.folder) if self.folder else os.path.join(prefs().mods_dir or tmp, self.name)
                out = core.run("export-actor", core.game_dir(), spec, "package", folder, self.title or self.name)
            else:
                out = core.run("export-actor", core.game_dir(), spec, "install", self.name)
                from . import ui
                ui.refresh()
        except core.CoreError as e:
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        r = out["report"]
        what = "замінено" if r["replaced"] else "новий"
        self.report({"INFO"}, f"{r['actor']} ({what}): {r['vertices']} вершин, {r['triangles']} трикутників, {r['bones']} кісток; {'; '.join(r['materials'])}")
        return {"FINISHED"}


def register():
    bpy.utils.register_class(RISEN_OT_export_actor)


def unregister():
    bpy.utils.unregister_class(RISEN_OT_export_actor)
