"""File > Import > Risen Actor + Motions, and Risen Motion onto the selected skeleton.

risen-core turns the `._xmac` (skeleton + skinned mesh) and chosen `._xmot` clips into a glTF,
and Blender's own glTF importer builds the armature, the skin and one action per clip.
"""

import os

import bpy
from bpy.props import IntProperty, StringProperty

from . import core

_actors = None


def _actor_names():
    global _actors
    if _actors is None:
        _actors = [f["name"] for f in core.run("actors", core.game_dir(), "", "100000")]
    return _actors


def _search_actor(self, context, edit_text):
    try:
        names = _actor_names()
    except core.CoreError:
        return []
    q = edit_text.lower()
    return [n for n in names if q in n.lower()][:300]


def _import_glb(path):
    before_obs, before_acts = set(bpy.data.objects), set(bpy.data.actions)
    bpy.ops.import_scene.gltf(filepath=path)
    return [o for o in bpy.data.objects if o not in before_obs], [a for a in bpy.data.actions if a not in before_acts]


def _glb_path(name):
    d = os.path.join(core.cache_dir(), "actors")
    os.makedirs(d, exist_ok=True)
    return os.path.join(d, f"{name}.glb")


class RISEN_OT_import_actor(bpy.types.Operator):
    bl_idname = "risen.import_actor"
    bl_label = "Risen Actor + Motions (._xmac / ._xmot)"
    bl_description = "Персонаж або істота зі скелетом, шкіркою і анімаціями з архівів гри"
    bl_options = {"REGISTER", "UNDO"}

    name: StringProperty(name="Актор", description="Назва ._xmac, напр. Ani_Wolf_Monster_Wolf або Ani_Hero_Armor_Player", search=_search_actor)
    clips: StringProperty(name="Анімації", description="Порожньо — лише стійка (idle); слова — усі кліпи актора з ними (напр. attack); * — без анімацій")
    limit: IntProperty(name="Не більше", default=40, min=1, max=2000)
    head: StringProperty(name="Голова", description="Людям (Ani_Hero_*) голова — окремий актор; порожньо — голова гравця, - — без голови", search=_search_actor)

    def invoke(self, context, event):
        return context.window_manager.invoke_props_dialog(self, width=460)

    def execute(self, context):
        if not self.name:
            self.report({"ERROR"}, "Не вказано актора")
            return {"CANCELLED"}
        try:
            args = [self.head] if self.head else []
            out = core.run("actor", core.game_dir(), self.name, _glb_path(self.name), self.clips, str(self.limit), "full", *args)
        except core.CoreError as e:
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        obs, acts = _import_glb(out["glb"])
        arm = next((o for o in obs if o.type == "ARMATURE"), None)
        if arm:
            arm["risen_actor"] = self.name
            arm.name = self.name
            if acts and arm.animation_data:
                arm.animation_data.action = acts[0]
        for a in acts:
            a.use_fake_user = True
        for w in out["warnings"]:
            self.report({"WARNING"}, w)
        self.report({"INFO"}, f"{self.name}: {out['joints']} кісток, {out['triangles']} трикутників, {len(out['clips'])} анімацій")
        return {"FINISHED"}


def _search_clip(self, context, edit_text):
    arm = context.active_object
    actor = arm.get("risen_actor") if arm else None
    if not actor:
        return []
    try:
        return core.run("clips", core.game_dir(), actor, edit_text, "300")
    except core.CoreError:
        return []


class RISEN_OT_import_motion(bpy.types.Operator):
    bl_idname = "risen.import_motion"
    bl_label = "Risen Motion на виділений скелет (._xmot)"
    bl_description = "Анімація з гри на скелет, імпортований як Risen Actor"
    bl_options = {"REGISTER", "UNDO"}

    clip: StringProperty(name="Анімація", description="Назва кліпу або слова з неї (усі збіги, до «Не більше»)", search=_search_clip)
    limit: IntProperty(name="Не більше", default=1, min=1, max=2000)

    @classmethod
    def poll(cls, context):
        o = context.active_object
        return o is not None and o.type == "ARMATURE" and "risen_actor" in o

    def invoke(self, context, event):
        return context.window_manager.invoke_props_dialog(self, width=520)

    def execute(self, context):
        arm = context.active_object
        actor = arm["risen_actor"]
        try:
            out = core.run("actor", core.game_dir(), actor, _glb_path(actor + "_motion"), self.clip or "*", str(self.limit), "skeleton", "-")
        except core.CoreError as e:
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        if not out["clips"]:
            self.report({"ERROR"}, f"Немає кліпу {self.clip!r} для {actor}")
            return {"CANCELLED"}
        mats = set(bpy.data.materials)
        obs, acts = _import_glb(out["glb"])
        # Only the actions are wanted; the helper armature, skin and materials go again.
        datas = {o.data for o in obs if o.data is not None}
        for o in obs:
            bpy.data.objects.remove(o, do_unlink=True)
        for d in datas:
            (bpy.data.meshes if isinstance(d, bpy.types.Mesh) else bpy.data.armatures if isinstance(d, bpy.types.Armature) else None) and d.users == 0 and (bpy.data.meshes.remove(d) if isinstance(d, bpy.types.Mesh) else bpy.data.armatures.remove(d))
        for m in set(bpy.data.materials) - mats:
            if m.users == 0:
                bpy.data.materials.remove(m)
        for a in acts:
            a.use_fake_user = True
            # The glTF importer names actions "<clip>_<armature>"; keep the clip name.
            for c, _ in out["clips"]:
                if a.name.startswith(c):
                    a.name = c
        if arm.animation_data is None:
            arm.animation_data_create()
        arm.animation_data.action = acts[0]
        if hasattr(arm.animation_data, "action_slot") and acts[0].slots:
            arm.animation_data.action_slot = acts[0].slots[0]
        self.report({"INFO"}, f"{actor}: {len(acts)} анімацій — {', '.join(a.name for a in acts[:3])}")
        return {"FINISHED"}


_classes = (RISEN_OT_import_actor, RISEN_OT_import_motion)


def register():
    for c in _classes:
        bpy.utils.register_class(c)


def unregister():
    for c in reversed(_classes):
        bpy.utils.unregister_class(c)
