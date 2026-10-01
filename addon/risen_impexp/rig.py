"""Risen panel > Animation rig: controls on top of a Risen armature, for animating by hand.

Nothing of the game's skeleton changes: no rest pose, no names, no parents. The rig only adds
  * bone colours and collections (body, left, right, fingers, slots),
  * shapes to grab (circles on the spine and head, a square on the root),
  * IK for arms and legs: a target (`RIG_IK_<bone>`) and a pole (`RIG_Pole_<bone>`) per limb.
    These are extra bones that deform nothing; motion export samples the final pose of the game's
    bones and skips any bone the actor does not have, so the IK goes into the clip as plain keys.
IK starts switched off, so imported clips play as they are. Switching a limb on snaps its target
and pole to where the limb is now; "Bake" writes the result onto the game's bones.
"""

import bpy
from mathutils import Vector

PREFIX_IK = "RIG_IK_"
PREFIX_POLE = "RIG_Pole_"
SHAPES = "RisenRigShapes"

GROUPS = (  # collection, colour, test on the bone name
    ("Пальці", "THEME03", lambda n: "finger" in n),
    ("Слоти", "THEME10", lambda n: n.startswith("slot")),
    ("Ліва сторона", "THEME04", lambda n: "left" in n),
    ("Права сторона", "THEME01", lambda n: "right" in n),
    ("Тіло", "THEME09", lambda n: n.startswith("bone")),
)


def is_rig_armature(ob):
    return ob is not None and ob.type == "ARMATURE" and "risen_actor" in ob


def _shape(name, verts, edges):
    ob = bpy.data.objects.get(name)
    if ob:
        return ob
    me = bpy.data.meshes.new(name)
    me.from_pydata(verts, edges, [])
    ob = bpy.data.objects.new(name, me)
    coll = bpy.data.collections.get(SHAPES)
    if coll is None:
        coll = bpy.data.collections.new(SHAPES)
        coll.hide_viewport = True
        coll.hide_render = True
    coll.objects.link(ob)
    return ob


def _circle(name, r=1.0, n=24):
    import math
    v = [(r * math.cos(2 * math.pi * i / n), 0.5, r * math.sin(2 * math.pi * i / n)) for i in range(n)]
    return _shape(name, v, [(i, (i + 1) % n) for i in range(n)])


def _box(name, s=1.0):
    v = [(x * s, y * s, z * s) for x in (-1, 1) for y in (-1, 1) for z in (-1, 1)]
    e = [(a, b) for a in range(8) for b in range(a + 1, 8) if bin(a ^ b).count("1") == 1]
    return _shape(name, v, e)


def _diamond(name, s=1.0):
    v = [(s, 0, 0), (-s, 0, 0), (0, s, 0), (0, -s, 0), (0, 0, s), (0, 0, -s)]
    e = [(a, b) for a in range(6) for b in range(a + 1, 6) if a // 2 != b // 2]
    return _shape(name, v, e)


def limbs(arm):
    """(effector, chain bones from the top) for every hand and foot: the effector's parent and
    its ancestors named Arm/Leg (not shoulder or hip). Only the last two go into the IK (a
    quadruped's upper leg stays FK): two bones and a pole can match any pose exactly."""
    out = []
    for b in arm.data.bones:
        n = b.name.lower()
        if b.name.startswith(("RIG_",)) or "finger" in n or n.endswith("_end") or not ("hand" in n or "foot" in n):
            continue
        chain, p = [], b.parent
        while p is not None and len(chain) < 3:
            pn = p.name.lower()
            if ("arm" in pn or "leg" in pn) and "shoulder" not in pn and "hip" not in pn and "hand" not in pn and "foot" not in pn:
                chain.insert(0, p.name)
                p = p.parent
            else:
                break
        if len(chain) >= 2:
            out.append((b.name, chain[-2:]))
    return out


def _pole_position(top_head, mid_head, end_tail, length):
    line = end_tail - top_head
    t = (mid_head - top_head).dot(line) / max(line.length_squared, 1e-12)
    bend = mid_head - (top_head + line * t)
    if bend.length < 1e-4 * length:
        bend = Vector((0, -1, 0))  # straight in rest: in front
    return mid_head + bend.normalized() * length


def _pole_angle(base_head, base_tail, base_x, end_tail, pole_loc):
    """IK pole angle that leaves the chain's top bone turned as it is (all in armature space)."""
    normal = (end_tail - base_head).cross(pole_loc - base_head)
    proj = normal.cross(base_tail - base_head)
    a = base_x.angle(proj, 0.0)
    return -a if base_x.cross(proj).angle(base_tail - base_head, 0.0) < 1 else a


def setup(context, arm):
    data = arm.data
    data.show_bone_custom_shapes = True
    made = []
    found = limbs(arm)
    prev_mode = arm.mode
    context.view_layer.objects.active = arm
    bpy.ops.object.mode_set(mode="EDIT")
    try:
        eb = data.edit_bones
        for end, chain in found:
            if PREFIX_IK + end in eb:
                continue
            last, top, mid = eb[chain[-1]], eb[chain[0]], eb[chain[1]]
            length = sum(eb[c].length for c in chain)
            t = eb.new(PREFIX_IK + end)
            src = eb[end]
            t.head = last.tail.copy()
            t.tail = last.tail + (src.tail - src.head)
            t.roll = src.roll
            t.use_deform = False
            p = eb.new(PREFIX_POLE + end)
            pos = _pole_position(top.head, mid.head, last.tail, length * 0.5)
            p.head = pos
            p.tail = pos + Vector((0, 0, length * 0.1))
            p.use_deform = False
            made.append(end)
    finally:
        bpy.ops.object.mode_set(mode="POSE")

    ik_shape, pole_shape = _box("RIG_Shape_IK", 0.35), _diamond("RIG_Shape_Pole", 0.6)
    ring, root = _circle("RIG_Shape_Ring", 0.6), _box("RIG_Shape_Root", 0.5)
    colls = {}
    for title, colour, _ in GROUPS + (("Керування IK", "THEME02", None),):
        c = data.collections.get(title) or data.collections.new(title)
        colls[title] = (c, colour)
    for pb in arm.pose.bones:
        n = pb.name.lower()
        if pb.name.startswith(PREFIX_IK) or pb.name.startswith(PREFIX_POLE):
            title = "Керування IK"
            pb.custom_shape = ik_shape if pb.name.startswith(PREFIX_IK) else pole_shape
        else:
            title = next((t for t, _, f in GROUPS if f(n)), None)
            if n == "bone_root":
                pb.custom_shape = root
            elif any(k in n for k in ("spine", "neck", "head")) and title == "Тіло":
                pb.custom_shape = ring
        if title:
            c, colour = colls[title]
            c.assign(pb.bone)
            pb.color.palette = colour

    for end, chain in found:
        pb_last = arm.pose.bones[chain[-1]]
        if any(c.name == "Risen IK" for c in pb_last.constraints):
            continue
        ik = pb_last.constraints.new("IK")
        ik.name = "Risen IK"
        ik.target, ik.subtarget = arm, PREFIX_IK + end
        ik.pole_target, ik.pole_subtarget = arm, PREFIX_POLE + end
        b0, rest = data.bones[chain[0]], data.bones
        ik.pole_angle = _pole_angle(b0.head_local, b0.tail_local, b0.matrix_local.to_3x3().col[0], rest[chain[-1]].tail_local, rest[PREFIX_POLE + end].head_local)
        ik.chain_count = len(chain)
        ik.influence = 0.0
        cr = arm.pose.bones[end].constraints.new("COPY_ROTATION")
        cr.name = "Risen IK rotation"
        cr.target, cr.subtarget = arm, PREFIX_IK + end
        cr.influence = 0.0
    if prev_mode != "POSE":
        bpy.ops.object.mode_set(mode=prev_mode)
    return found, made


def ik_on(arm, end):
    c = arm.pose.bones[end].constraints.get("Risen IK rotation")
    return c is not None and c.influence > 0.5


def set_ik(context, arm, end, on):
    """Switch one limb's IK; switching on puts the target and pole where the limb is now."""
    chain = dict(limbs(arm))[end]
    pbs = arm.pose.bones
    if on:
        context.view_layer.update()
        last, mid, top = pbs[chain[-1]], pbs[chain[1]], pbs[chain[0]]
        m = pbs[end].matrix.copy()
        m.translation = last.tail
        pbs[PREFIX_IK + end].matrix = m
        context.view_layer.update()
        rest = arm.data.bones
        length = sum(rest[c].length for c in chain) * 0.5
        pole = pbs[PREFIX_POLE + end]
        pm = pole.matrix.copy()
        pm.translation = _pole_position(top.head, mid.head, last.tail, length)
        pole.matrix = pm
        ik = last.constraints.get("Risen IK")
        if ik:
            ik.pole_angle = _pole_angle(top.head, top.tail, top.matrix.to_3x3().col[0], last.tail, pm.translation)
    for b, cname in ((chain[-1], "Risen IK"), (end, "Risen IK rotation")):
        c = pbs[b].constraints.get(cname)
        if c:
            c.influence = 1.0 if on else 0.0


class RISEN_OT_rig_setup(bpy.types.Operator):
    bl_idname = "risen.rig_setup"
    bl_label = "Риг для анімування"
    bl_description = "Кольори, фігури й IK для рук і ніг поверх кісток гри (самі кістки гри не змінюються, експорт лишається точним)"
    bl_options = {"REGISTER", "UNDO"}

    @classmethod
    def poll(cls, context):
        return is_rig_armature(context.active_object)

    def execute(self, context):
        found, made = setup(context, context.active_object)
        self.report({"INFO"}, f"Риг: {len(found)} кінцівок з IK ({', '.join(e for e, _ in found) or 'немає'}); IK вмикається в панелі Risen")
        return {"FINISHED"}


class RISEN_OT_rig_ik(bpy.types.Operator):
    bl_idname = "risen.rig_ik"
    bl_label = "IK"
    bl_description = "Увімкнути/вимкнути IK кінцівки; при вмиканні ціль ставиться туди, де кінцівка зараз"
    bl_options = {"REGISTER", "UNDO"}

    bone: bpy.props.StringProperty()
    on: bpy.props.BoolProperty()

    @classmethod
    def poll(cls, context):
        return is_rig_armature(context.active_object)

    def execute(self, context):
        arm = context.active_object
        if arm.mode != "POSE":
            bpy.ops.object.mode_set(mode="POSE")
        set_ik(context, arm, self.bone, self.on)
        return {"FINISHED"}


class RISEN_OT_rig_bake(bpy.types.Operator):
    bl_idname = "risen.rig_bake"
    bl_label = "Запекти в кістки гри"
    bl_description = "Записати кінцевий рух (з IK) ключами на кістки гри в межах кадрів дії й вимкнути IK"
    bl_options = {"REGISTER", "UNDO"}

    @classmethod
    def poll(cls, context):
        o = context.active_object
        return is_rig_armature(o) and o.animation_data and o.animation_data.action

    def execute(self, context):
        arm = context.active_object
        start, end = (int(round(f)) for f in arm.animation_data.action.frame_range)
        if arm.mode != "POSE":
            bpy.ops.object.mode_set(mode="POSE")
        for pb in arm.pose.bones:
            pb.select = not pb.name.startswith("RIG_")
        bpy.ops.nla.bake(frame_start=start, frame_end=end, only_selected=True, visual_keying=True,
                         clear_constraints=False, use_current_action=True, bake_types={"POSE"})
        for e, _ in limbs(arm):
            if ik_on(arm, e):
                set_ik(context, arm, e, False)
        self.report({"INFO"}, f"Запечено кадри {start}–{end}; IK вимкнено")
        return {"FINISHED"}


def draw_panel(layout, context):
    arm = context.active_object
    if not is_rig_armature(arm):
        return
    box = layout.box()
    box.label(text="Анімування", icon="POSE_HLT")
    found = limbs(arm)
    if not any(PREFIX_IK + e in arm.data.bones for e, _ in found):
        box.operator("risen.rig_setup", icon="BONE_DATA")
        return
    for e, _ in found:
        if PREFIX_IK + e not in arm.data.bones:
            continue
        on = ik_on(arm, e)
        op = box.operator("risen.rig_ik", text=f"IK {e.replace('Bone_', '')}", icon="CHECKBOX_HLT" if on else "CHECKBOX_DEHLT", depress=on)
        op.bone, op.on = e, not on
    box.operator("risen.rig_bake", icon="REC")


_classes = (RISEN_OT_rig_setup, RISEN_OT_rig_ik, RISEN_OT_rig_bake)


def register():
    for c in _classes:
        bpy.utils.register_class(c)


def unregister():
    for c in reversed(_classes):
        bpy.utils.unregister_class(c)
