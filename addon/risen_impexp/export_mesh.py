"""File > Export > Risen Mesh → Mod: the selected objects become one Risen static mesh.

Geometry goes to risen-core as raw arrays already in game space; the core welds vertices, computes
tangents, writes `._xmsh`, textures and materials, the resource directories, and either a mod
package (INSTALL.bat / ROLLBACK.bat) or an install straight into the game.

Axes: Blender (x, y, z) in metres = game (x, z, y) in centimetres ÷ scale — one reflection, the
inverse of the importer's, so triangle order stays as it is (Blender counter-clockwise → Direct3D
clockwise). V is flipped to Direct3D's V down.
"""

import os
import tempfile

import bpy
import numpy as np
from bpy.props import EnumProperty, FloatProperty, StringProperty

from . import core
from .prefs import prefs

SHAPE_MATERIALS = ("none", "wood", "metal", "water", "stone", "earth", "ice", "leather", "clay", "glass", "flesh", "snow", "debris", "foliage", "magic", "grass", "sand")


def _linked_image(socket):
    """The image feeding `socket`, through a Normal Map node if there is one."""
    if socket is None or not socket.is_linked:
        return None
    node = socket.links[0].from_node
    if node.type == "NORMAL_MAP":
        return _linked_image(node.inputs.get("Color"))
    if node.type == "TEX_IMAGE":
        return node.image
    return None


def _image_png(img, tmp):
    """A PNG on disk for `img`: its own file when it is one, otherwise a saved copy."""
    if img is None:
        return None
    path = bpy.path.abspath(img.filepath) if img.filepath else ""
    if path and os.path.isfile(path) and not img.packed_file and not img.is_dirty and path.lower().endswith(".png"):
        return path
    out = os.path.join(tmp, bpy.path.clean_name(img.name) + ".png")
    c = img.copy()
    try:
        c.filepath_raw = out
        c.file_format = "PNG"
        c.save()
    finally:
        bpy.data.images.remove(c)
    return out


def _material_spec(mat, tmp):
    spec = {"name": mat.name if mat else "Default", "diffuse": None, "normal": None}
    if mat and mat.use_nodes:
        bsdf = next((n for n in mat.node_tree.nodes if n.type == "BSDF_PRINCIPLED"), None)
        if bsdf:
            spec["diffuse"] = _image_png(_linked_image(bsdf.inputs.get("Base Color")), tmp)
            spec["normal"] = _image_png(_linked_image(bsdf.inputs.get("Normal")), tmp)
    return spec


def gather(context, objects, scale, tmp):
    """Corner arrays of all `objects` (modifiers applied, world space) in game space, plus materials."""
    dg = context.evaluated_depsgraph_get()
    pos, nrm, uv, mat = [], [], [], []
    materials, slot_of = [], {}
    for ob in objects:
        ev = ob.evaluated_get(dg)
        me = ev.to_mesh()
        try:
            me.transform(ob.matrix_world)
            if ob.matrix_world.determinant() < 0:
                me.flip_normals()
            me.calc_loop_triangles()
            nt = len(me.loop_triangles)
            if nt == 0:
                continue
            loops = np.empty(nt * 3, np.int32)
            me.loop_triangles.foreach_get("loops", loops)
            mi = np.empty(nt, np.int32)
            me.loop_triangles.foreach_get("material_index", mi)
            vidx = np.empty(len(me.loops), np.int32)
            me.loops.foreach_get("vertex_index", vidx)
            co = np.empty(len(me.vertices) * 3, np.float32)
            me.vertices.foreach_get("co", co)
            co = co.reshape(-1, 3)
            cn = np.empty(len(me.loops) * 3, np.float32)
            me.corner_normals.foreach_get("vector", cn)
            cn = cn.reshape(-1, 3)
            if me.uv_layers.active:
                u = np.empty(len(me.loops) * 2, np.float32)
                me.uv_layers.active.data.foreach_get("uv", u)
                u = u.reshape(-1, 2)
            else:
                u = np.zeros((len(me.loops), 2), np.float32)
            p = co[vidx[loops]] / scale
            n = cn[loops]
            t = u[loops]
            pos.append(p[:, [0, 2, 1]])
            nrm.append(n[:, [0, 2, 1]])
            uv.append(np.stack([t[:, 0], 1.0 - t[:, 1]], axis=1))
            # Object material slots → one global list, shared by name.
            slots = [s.material for s in ob.material_slots] or [None]
            remap = []
            for m in slots:
                key = m.name if m else "Default"
                if key not in slot_of:
                    slot_of[key] = len(materials)
                    materials.append(_material_spec(m, tmp))
                remap.append(slot_of[key])
            mat.append(np.array(remap, np.uint32)[np.clip(mi, 0, len(remap) - 1)])
        finally:
            ev.to_mesh_clear()
    if not pos:
        return None
    return (np.concatenate(pos).astype(np.float32), np.concatenate(nrm).astype(np.float32),
            np.concatenate(uv).astype(np.float32), np.concatenate(mat).astype(np.uint32), materials)


def _default_name(context):
    ob = context.active_object
    if ob is None:
        return ""
    entry = ob.get("risen_entry")
    if entry:
        return entry.rsplit("/", 1)[-1].split(".", 1)[0]
    return bpy.path.clean_name(ob.name.split(".")[0])


class RISEN_OT_export_mesh(bpy.types.Operator):
    bl_idname = "risen.export_mesh"
    bl_label = "Risen Mesh → Mod (._xmsh)"
    bl_description = "Виділені об'єкти → модель Risen: замінити наявну (та сама назва) або додати нову; пакет мода або встановлення в гру"

    name: StringProperty(name="Назва моделі", description="Назва ._xmsh у грі. Наявна назва — заміна (напр. It_Wpn_2H_Berserk), нова — додати")
    mode: EnumProperty(name="Куди", items=(
        ("install", "Встановити в гру", "Одразу в папку гри; моди додаються один до одного, прибрати — панель Risen"),
        ("package", "Пакет мода", "Папка з files/, INSTALL.bat і ROLLBACK.bat, щоб поділитися"),
    ), default="install")
    folder: StringProperty(name="Папка пакета", subtype="DIR_PATH", description="Куди скласти пакет (типово — папка модів з налаштувань)")
    title: StringProperty(name="Назва мода", description="Підпис у INSTALL.bat і README")
    scale: FloatProperty(name="Масштаб", description="Як при імпорті: 0.01 = метри Blender", default=0.01, min=1e-6)
    collision: EnumProperty(name="Колізія", items=(
        ("auto", "Так", "З виділених об'єктів *_COL, а без них — з самої моделі"),
        ("none", "Ні", "Лишити колізію гри як є (або без колізії)"),
    ), default="auto")
    shape_material: EnumProperty(name="Поверхня", items=[("keep", "Як була", "Матеріал поверхні замінюваної колізії, для нової — stone")] + [(m, m, "") for m in SHAPE_MATERIALS], default="keep")

    @classmethod
    def poll(cls, context):
        return any(o.type == "MESH" for o in context.selected_objects)

    def invoke(self, context, event):
        if not self.name:
            self.name = _default_name(context)
        return context.window_manager.invoke_props_dialog(self, width=460)

    def draw(self, context):
        col = self.layout.column()
        col.prop(self, "name")
        col.prop(self, "mode", expand=True)
        if self.mode == "package":
            col.prop(self, "folder")
            col.prop(self, "title")
        col.prop(self, "scale")
        col.prop(self, "collision")
        if self.collision != "none":
            col.prop(self, "shape_material")

    def execute(self, context):
        selected = [o for o in context.selected_objects if o.type == "MESH"]
        # Objects named *_COL are the collision shape, the rest the model.
        col_objects = [o for o in selected if "_col" in o.name.lower()]
        objects = [o for o in selected if o not in col_objects]
        if not objects or not self.name:
            self.report({"ERROR"}, "Виділіть меш і вкажіть назву моделі")
            return {"CANCELLED"}
        tmp = tempfile.mkdtemp(prefix="risen_export_")
        try:
            g = gather(context, objects, self.scale, tmp)
            if g is None:
                self.report({"ERROR"}, "У виділених об'єктах немає трикутників")
                return {"CANCELLED"}
            pos, nrm, uv, mat, materials = g
            geo = os.path.join(tmp, "geometry.bin")
            with open(geo, "wb") as f:
                for a in (pos, nrm, uv, mat):
                    f.write(np.ascontiguousarray(a).tobytes())
            import json
            spec = os.path.join(tmp, "spec.json")
            doc = {"name": self.name, "geometry": geo, "corners": len(pos), "materials": materials}
            if self.collision != "none":
                c = {"material": None if self.shape_material == "keep" else self.shape_material}
                cg = gather(context, col_objects, self.scale, tmp) if col_objects else None
                if cg is not None:
                    cgeo = os.path.join(tmp, "collision.bin")
                    with open(cgeo, "wb") as f:
                        for a in cg[:4]:
                            f.write(np.ascontiguousarray(a).tobytes())
                    c.update(geometry=cgeo, corners=len(cg[0]))
                doc["collision"] = c
            with open(spec, "w", encoding="utf-8") as f:
                json.dump(doc, f)
            if self.mode == "package":
                folder = bpy.path.abspath(self.folder) if self.folder else os.path.join(prefs().mods_dir or tmp, self.name)
                out = core.run("export", core.game_dir(), spec, "package", folder, self.title or self.name)
            else:
                out = core.run("export", core.game_dir(), spec, "install", self.name)
        except core.CoreError as e:
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        r = out["report"]
        for w in r["warnings"]:
            self.report({"WARNING"}, w)
        what = "замінено" if r["replaced"] else "додано"
        where = folder if self.mode == "package" else "гру"
        if self.mode == "install":
            from . import ui
            ui.refresh()
        if r.get("collision"):
            self.report({"INFO"}, f"колізія: {r['collision']}")
        self.report({"INFO"}, f"{self.name} ({what}): {r['vertices']} вершин, {r['triangles']} трикутників → {where}")
        return {"FINISHED"}


def register():
    bpy.utils.register_class(RISEN_OT_export_mesh)


def unregister():
    bpy.utils.unregister_class(RISEN_OT_export_mesh)
