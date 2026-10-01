"""File > Import > Risen Lip-sync: a voice line of the game on a head's face shapes.

Every human head has nine shape keys (BS_Blink and eight mouth shapes, BS_M_B_P_X, BS_AA_AO_OW, …),
brought in by Risen Character. A line of dialogue (`infos/…`, one clip per language) says how far
each mouth shape is on over time; this puts that on the shape keys as an action, and the line's
voice into the Video Sequencer, so it plays in sync. Blinks are added if asked: the game blinks on
its own, the clip has none.
"""

import os
import random

import bpy
from bpy.props import BoolProperty, EnumProperty, StringProperty

from . import core

LANGUAGES = (("English", "English", ""), ("German", "Deutsch", ""), ("Russian", "Русский", ""), ("French", "Français", ""))

_clips = None


def clips():
    global _clips
    if _clips is None:
        _clips = core.run("lipclips", core.game_dir(), "", "10000000")
    return _clips


def _search(self, context, edit_text):
    try:
        words = edit_text.lower().split()
        lang = f"_{self.language.lower()}_"
        out = []
        for c in clips():
            l = c.lower()
            if lang in l and all(w in l for w in words):
                out.append(c)
                if len(out) >= 200:
                    break
        return out
    except core.CoreError:
        return []


def face_of(ob):
    """The head mesh (with BS_* shape keys) of `ob` — itself, or a mesh of its armature."""
    def has_face(o):
        k = o.data.shape_keys if o and o.type == "MESH" else None
        return k is not None and any(b.name.startswith("BS_") for b in k.key_blocks)
    if ob is None:
        return None
    if has_face(ob):
        return ob
    arm = ob if ob.type == "ARMATURE" else ob.parent
    if arm is not None:
        return next((c for c in arm.children if has_face(c)), None)
    return None


def head_name(face):
    return face.get("risen_head") or face.name.split(".")[0]


def _strips(scene):
    se = scene.sequence_editor or scene.sequence_editor_create()
    return se.strips if hasattr(se, "strips") else se.sequences


class RISEN_OT_import_lipsync(bpy.types.Operator):
    bl_idname = "risen.import_lipsync"
    bl_label = "Risen Lip-sync (міміка)"
    bl_description = "Репліка гри на обличчі: рух губ на shape keys голови й голос у Video Sequencer"
    bl_options = {"REGISTER", "UNDO"}

    language: EnumProperty(name="Мова", items=LANGUAGES)
    clip: StringProperty(name="Репліка", description="Слова з назви, напр. PC_Hero (гравець) або ім'я персонажа", search=_search)
    sound: BoolProperty(name="З голосом", default=True)
    blink: BoolProperty(name="Кліпати", description="Додати кліпання кожні 2–5 с", default=True)
    frame: bpy.props.IntProperty(name="З кадру", description="Початок репліки (типово — поточний кадр)", default=-1)

    @classmethod
    def poll(cls, context):
        return face_of(context.active_object) is not None

    def invoke(self, context, event):
        return context.window_manager.invoke_props_dialog(self, width=520)

    def execute(self, context):
        face = face_of(context.active_object)
        if not self.clip:
            self.report({"ERROR"}, "Не вибрано репліку")
            return {"CANCELLED"}
        try:
            out = core.run("lipsync", core.game_dir(), head_name(face), self.clip, os.path.join(core.cache_dir(), "speech"))
        except core.CoreError as e:
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        scene = context.scene
        fps = scene.render.fps / scene.render.fps_base
        start = scene.frame_current if self.frame < 0 else self.frame
        key = face.data.shape_keys
        ad = key.animation_data or key.animation_data_create()
        act = bpy.data.actions.new(out["clip"])
        act["risen_lipsync"] = out["clip"]
        ad.action = act
        blocks = key.key_blocks
        for name, keys in out["channels"]:
            kb = blocks.get(name)
            if kb is None:
                continue
            for t, w in keys:
                kb.value = w
                kb.keyframe_insert("value", frame=start + t * fps)
        end = start + out["duration"] * fps
        if self.blink and "BS_Blink" in blocks:
            kb, rng, t = blocks["BS_Blink"], random.Random(out["clip"]), start + 0.5 * fps
            while t < end:
                for dt, w in ((0, 0.0), (0.08, 1.0), (0.2, 0.0)):
                    kb.value = w
                    kb.keyframe_insert("value", frame=t + dt * fps)
                t += rng.uniform(2.0, 5.0) * fps
        for kb in blocks:
            kb.value = 0.0
        if self.sound and out["sound"]:
            s = _strips(scene).new_sound(out["clip"], out["sound"], 1, int(start))
            s["risen_lipsync"] = out["clip"]
        scene.frame_end = max(scene.frame_end, int(end) + 1)
        self.report({"INFO"}, f"{out['clip']}: {len(out['channels'])} форм рота, {out['duration']:.1f} с{', з голосом' if self.sound and out['sound'] else ''} → {face.name}")
        return {"FINISHED"}


def register():
    bpy.utils.register_class(RISEN_OT_import_lipsync)


def unregister():
    bpy.utils.unregister_class(RISEN_OT_import_lipsync)
