"""File > Export > Risen Motion: the active action of a Risen skeleton becomes a `._xmot` clip.

Blender's own glTF exporter samples the action (every frame, bone space); risen-core converts the
keys back to the game's axes and writes the clip shaped like one of the actor's own (its info,
events and per-bone headers), with its record in compiled_motions.bin. Same clip name = replace
(e.g. the hero's run), a new name = add a clip.
"""

import json
import os
import tempfile

import bpy
from bpy.props import EnumProperty, StringProperty

from . import core
from .prefs import prefs


class RISEN_OT_export_motion(bpy.types.Operator):
    bl_idname = "risen.export_motion"
    bl_label = "Risen Motion (._xmot)"
    bl_description = "Активна анімація скелета Risen → кліп гри (заміна за тією ж назвою або новий)"

    clip: StringProperty(name="Назва кліпу", description="Напр. Hero_Stand_None_None_P0_Move_Run_N_Fwd_00_%_00_P0_400 — заміна; нова назва — новий кліп")
    mode: EnumProperty(name="Куди", items=(
        ("install", "Встановити в гру", "Одразу в папку гри; прибрати — панель Risen"),
        ("package", "Пакет мода", "Папка з files/, INSTALL.bat і ROLLBACK.bat"),
    ), default="install")
    folder: StringProperty(name="Папка пакета", subtype="DIR_PATH")
    title: StringProperty(name="Назва мода")

    @classmethod
    def poll(cls, context):
        o = context.active_object
        return o is not None and o.type == "ARMATURE" and "risen_actor" in o and o.animation_data and o.animation_data.action

    def invoke(self, context, event):
        if not self.clip:
            self.clip = context.active_object.animation_data.action.name.split(".")[0]
        return context.window_manager.invoke_props_dialog(self, width=560)

    def draw(self, context):
        col = self.layout.column()
        col.prop(self, "clip")
        col.prop(self, "mode", expand=True)
        if self.mode == "package":
            col.prop(self, "folder")
            col.prop(self, "title")

    def execute(self, context):
        arm = context.active_object
        action = arm.animation_data.action
        tmp = tempfile.mkdtemp(prefix="risen_motion_")
        glb = os.path.join(tmp, "motion.glb")
        sel = [o for o in context.selected_objects]
        scene = context.scene
        fps = (scene.render.fps, scene.render.fps_base)
        try:
            # Risen clips run at 30 frames per second.
            scene.render.fps, scene.render.fps_base = 30, 1.0
            for o in sel:
                o.select_set(False)
            arm.select_set(True)
            bpy.ops.export_scene.gltf(
                filepath=glb, export_format="GLB", use_selection=True, export_animations=True,
                export_animation_mode="ACTIVE_ACTIONS", export_force_sampling=True, export_frame_step=1,
                export_anim_slide_to_zero=True, export_optimize_animation_size=False, export_skins=True,
                export_materials="NONE", export_yup=True, export_def_bones=False,
            )
        finally:
            scene.render.fps, scene.render.fps_base = fps
            arm.select_set(False)
            for o in sel:
                o.select_set(True)
        spec = os.path.join(tmp, "spec.json")
        with open(spec, "w", encoding="utf-8") as f:
            json.dump({"actor": arm["risen_actor"], "clip": self.clip, "glb": glb, "animation": None}, f)
        try:
            if self.mode == "package":
                folder = bpy.path.abspath(self.folder) if self.folder else os.path.join(prefs().mods_dir or tmp, self.clip)
                out = core.run("export-motion", core.game_dir(), spec, "package", folder, self.title or self.clip)
            else:
                out = core.run("export-motion", core.game_dir(), spec, "install", self.clip)
                from . import ui
                ui.refresh()
        except core.CoreError as e:
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        r = out["report"]
        what = "замінено" if r["replaced"] else f"новий (за зразком {r['template']})"
        self.report({"INFO"}, f"{r['clip']} ({what}): {r['bones']} кісток, {r['keys']} ключів, {r['duration']:.2f} с — {action.name}")
        return {"FINISHED"}


def register():
    bpy.utils.register_class(RISEN_OT_export_motion)


def unregister():
    bpy.utils.unregister_class(RISEN_OT_export_motion)
