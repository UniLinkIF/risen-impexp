"""Blender material set-up from what the game's `._xmat` says (the OBJ/glTF importers know only
diffuse and normal): alpha test with its threshold, alpha blend, additive, and the specular map."""

import os

import bpy


def _bsdf(mat):
    return next((n for n in mat.node_tree.nodes if n.type == "BSDF_PRINCIPLED"), None) if mat and mat.use_nodes else None


def _base_image_node(mat, bsdf):
    s = bsdf.inputs.get("Base Color")
    if s and s.is_linked and s.links[0].from_node.type == "TEX_IMAGE":
        return s.links[0].from_node
    return None


def apply(mat, info, tex_dir):
    """`info` = one material from risen-core (`blend`, `mask`, `specular`)."""
    bsdf = _bsdf(mat)
    if bsdf is None:
        return
    nt = mat.node_tree
    img = _base_image_node(mat, bsdf)
    blend, mask = info.get("blend", 0), info.get("mask", 0)
    mat["risen_blend"] = blend
    if img is not None and blend in (1, 2, 7):
        alpha = bsdf.inputs["Alpha"]
        if blend == 1:
            # Alpha test: the game cuts at MaskReference/255.
            cmp = nt.nodes.new("ShaderNodeMath")
            cmp.operation = "GREATER_THAN"
            cmp.inputs[1].default_value = (mask or 128) / 255.0
            cmp.location = (img.location.x + 250, img.location.y - 250)
            nt.links.new(img.outputs["Alpha"], cmp.inputs[0])
            nt.links.new(cmp.outputs[0], alpha)
            mat["risen_mask"] = mask or 128
        else:
            nt.links.new(img.outputs["Alpha"], alpha)
        if hasattr(mat, "surface_render_method"):
            mat.surface_render_method = "DITHERED" if blend == 1 else "BLENDED"
    spec = info.get("specular")
    if spec:
        path = os.path.join(tex_dir, spec)
        if os.path.isfile(path):
            t = nt.nodes.new("ShaderNodeTexImage")
            t.image = bpy.data.images.load(path, check_existing=True)
            t.image.colorspace_settings.name = "Non-Color"
            t.location = (bsdf.location.x - 600, bsdf.location.y - 500)
            target = bsdf.inputs.get("Specular IOR Level") or bsdf.inputs.get("Specular")
            if target is not None:
                nt.links.new(t.outputs["Color"], target)
            t["risen_specular"] = True


def apply_to_objects(objects, infos, tex_dir):
    by_name = {i["name"].split(".")[0].lower(): i for i in infos}
    done = set()
    for ob in objects:
        for slot in ob.material_slots:
            m = slot.material
            if m is None or m in done:
                continue
            info = by_name.get(m.name.split(".")[0].lower())
            if info:
                apply(m, info, tex_dir)
                done.add(m)
