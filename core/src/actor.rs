//! Skinned actor (`._xmac`) + motion clips (`._xmot`) → binary glTF for Blender's glTF importer.
//!
//! glTF is right-handed Y up, so the game (left-handed Y up) is
//! mirrored in Z — positions/normals/bone translations `(x, y, -z)`, bone rotations
//! `(-x, -y, z, w)` — the same rule as the static-mesh OBJ. Unlike `._xmsh`, an actor's skin stores
//! its faces counter-clockwise against its normals (5714 of 5744 wolf faces), so after the mirror
//! each triangle is written reversed, (0, 2, 1), to face along its normals in glTF. glTF's UV origin is
//! top-left like Direct3D: V as stored. Centimetres → metres here (glTF is metric).
//! Inverse bind matrices come from the converted bind pose, so skin(bind) = identity.

use crate::game::GameCtx;
use crate::mesh::TextureIndex;
use anyhow::{bail, ensure, Context, Result};
use risen_formats::{xmac::{self, SkeletonNode}, xmesh_skin, xmot::{self, BoneMotion}};
use serde_json::{json, Value};
use std::path::Path;

pub const UNIT: f32 = 0.01;

pub fn conv_p(p: [f32; 3], s: f32) -> [f32; 3] { [p[0] * s, p[1] * s, -p[2] * s] }
pub fn conv_q(q: [f32; 4]) -> [f32; 4] {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    let n = if n > 1e-8 { n } else { 1.0 };
    [-q[0] / n, -q[1] / n, q[2] / n, q[3] / n]
}

type M4 = [f32; 16];
fn trs(t: [f32; 3], q: [f32; 4]) -> M4 {
    let [x, y, z, w] = q;
    let (xx, yy, zz, xy, xz, yz, wx, wy, wz) = (x * x, y * y, z * z, x * y, x * z, y * z, w * x, w * y, w * z);
    [
        1.0 - 2.0 * (yy + zz), 2.0 * (xy + wz), 2.0 * (xz - wy), 0.0,
        2.0 * (xy - wz), 1.0 - 2.0 * (xx + zz), 2.0 * (yz + wx), 0.0,
        2.0 * (xz + wy), 2.0 * (yz - wx), 1.0 - 2.0 * (xx + yy), 0.0,
        t[0], t[1], t[2], 1.0,
    ]
}
fn mul(a: &M4, b: &M4) -> M4 {
    let mut r = [0.0; 16];
    for c in 0..4 { for rr in 0..4 { r[c * 4 + rr] = (0..4).map(|k| a[k * 4 + rr] * b[c * 4 + k]).sum(); } }
    r
}
fn rigid_inverse(m: &M4) -> M4 {
    let mut r = [0.0; 16];
    for c in 0..3 { for rr in 0..3 { r[c * 4 + rr] = m[rr * 4 + c]; } }
    for rr in 0..3 { r[12 + rr] = -(0..3).map(|k| r[k * 4 + rr] * m[12 + k]).sum::<f32>(); }
    r[15] = 1.0;
    r
}

fn bind_globals(nodes: &[SkeletonNode]) -> Result<Vec<M4>> {
    let mut g: Vec<M4> = Vec::with_capacity(nodes.len());
    for (i, n) in nodes.iter().enumerate() {
        let local = trs(conv_p(n.position, UNIT), conv_q(n.rotation));
        g.push(match n.parent_index { Some(p) => { ensure!(p < i, "node {i} parent {p} is not earlier"); mul(&g[p], &local) } None => local });
    }
    Ok(g)
}

fn top4(w: &[(u32, f32)]) -> ([u16; 4], [f32; 4]) {
    let mut v: Vec<(u32, f32)> = w.iter().copied().filter(|(_, x)| *x > 0.0).collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(4);
    let sum: f32 = v.iter().map(|x| x.1).sum();
    let (mut j, mut ws) = ([0u16; 4], [0f32; 4]);
    if sum <= 0.0 { ws[0] = 1.0; return (j, ws); }
    for (k, (bi, x)) in v.iter().enumerate() { j[k] = *bi as u16; ws[k] = x / sum; }
    (j, ws)
}

struct Bin { data: Vec<u8>, views: Vec<Value>, accessors: Vec<Value> }
impl Bin {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while self.data.len() % 4 != 0 { self.data.push(0); }
        let mut v = json!({ "buffer": 0, "byteOffset": self.data.len(), "byteLength": bytes.len() });
        if let Some(t) = target { v["target"] = json!(t); }
        self.data.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }
    fn acc(&mut self, bytes: &[u8], target: Option<u32>, comp: u32, count: usize, ty: &str, minmax: Option<(Vec<f32>, Vec<f32>)>) -> usize {
        let bv = self.view(bytes, target);
        let mut a = json!({ "bufferView": bv, "componentType": comp, "count": count, "type": ty });
        if let Some((mn, mx)) = minmax { a["min"] = json!(mn); a["max"] = json!(mx); }
        self.accessors.push(a);
        self.accessors.len() - 1
    }
    fn f32s<const N: usize>(&mut self, v: &[[f32; N]], target: Option<u32>, ty: &str, minmax: bool) -> usize {
        let bytes: Vec<u8> = v.iter().flatten().flat_map(|x| x.to_le_bytes()).collect();
        let mm = minmax.then(|| {
            let mut mn = vec![f32::MAX; N];
            let mut mx = vec![f32::MIN; N];
            for e in v { for k in 0..N { mn[k] = mn[k].min(e[k]); mx[k] = mx[k].max(e[k]); } }
            (mn, mx)
        });
        self.acc(&bytes, target, 5126, v.len(), ty, mm)
    }
}

/// glTF samplers need strictly increasing times; keys that go backwards are dropped.
fn keys_monotonic<const N: usize>(keys: &[[f32; N]]) -> Vec<[f32; N]> {
    let mut out: Vec<[f32; N]> = vec![];
    for k in keys { if out.last().map_or(true, |l| k[N - 1] > l[N - 1]) { out.push(*k); } }
    out
}

/// Body clips only (entries of the `animations` archive — the speech archive lists `._xmot` too).
/// Lowercase stem → (stem, entry).
pub struct ClipIndex { by_stem: std::collections::BTreeMap<String, (String, String)> }

impl ClipIndex {
    pub fn build(g: &GameCtx) -> ClipIndex {
        let data = g.root.join("data");
        let mut by_stem = std::collections::BTreeMap::new();
        for e in g.entries_with_suffix("._xmot") {
            let Ok(p) = g.physical_path(&e) else { continue };
            let Ok(rel) = p.strip_prefix(&data) else { continue };
            let archive = rel.components().nth(1).map(|c| c.as_os_str().to_string_lossy().to_lowercase());
            if archive.as_deref() != Some("animations") { continue; }
            let stem = e.rsplit('/').next().unwrap().split('.').next().unwrap().to_string();
            by_stem.entry(stem.to_lowercase()).or_insert((stem, e));
        }
        ClipIndex { by_stem }
    }

    /// Clip names start with the creature: `Hero_…` for every human actor (`Ani_Hero_*`),
    /// `Wolf_…` for `Ani_Wolf_*`, … There is no file linking an actor to its clips; this is convention.
    pub fn token(actor_stem: &str) -> Option<String> {
        let s = actor_stem.to_lowercase();
        let t = s.strip_prefix("ani_")?.split('_').next().filter(|t| !t.is_empty())?;
        Some(t.to_string())
    }

    /// Clips of this actor whose name contains every word of `query` (case-insensitive).
    pub fn for_actor(&self, actor_stem: &str, query: &str) -> Vec<(String, String)> {
        let Some(t) = Self::token(actor_stem) else { return vec![] };
        let pre = format!("{t}_");
        let words: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
        self.by_stem.iter().filter(|(k, _)| k.starts_with(&pre) && words.iter().all(|w| k.contains(w))).map(|(_, v)| v.clone()).collect()
    }

    /// The idle loop: humans stand in `Hero_Stand_None_None_P0_Ambient_Loop…`, creatures in
    /// `<Creature>_Stand_None_None_P0_Ambient_Loop…`, else any `<Creature>_…Ambient_Loop…`.
    /// (clip name, entry) of a body clip by name.
    pub fn get(&self, clip: &str) -> Option<(String, String)> { self.by_stem.get(&clip.to_lowercase()).cloned() }

    pub fn idle_for(&self, actor_stem: &str) -> Option<(String, String)> {
        let t = Self::token(actor_stem)?;
        let exact = format!("{t}_stand_none_none_p0_ambient_loop");
        let first = |pred: &dyn Fn(&str) -> bool| self.by_stem.iter().find(|(k, _)| pred(k)).map(|(_, v)| v.clone());
        first(&|k| k.starts_with(&exact)).or_else(|| first(&|k| k.starts_with(&format!("{t}_")) && k.contains("_ambient_loop")))
    }
}

fn check_mesh(mesh: &xmesh_skin::SkinnedMesh) -> Result<()> {
    let nv = mesh.positions.len();
    ensure!(nv > 0 && mesh.skin_weights.len() == nv, "mesh has {nv} vertices and {} weight lists", mesh.skin_weights.len());
    ensure!(mesh.normals.len() == nv && mesh.uvs.len() == nv, "streams differ: {nv} positions, {} normals, {} uvs", mesh.normals.len(), mesh.uvs.len());
    ensure!(mesh.face_material_ids.len() == mesh.faces.len(), "{} faces but {} face material ids", mesh.faces.len(), mesh.face_material_ids.len());
    ensure!(mesh.faces.iter().flatten().all(|&i| (i as usize) < nv), "face index out of range (nv {nv})");
    ensure!(mesh.positions.iter().flatten().chain(mesh.uvs.iter().flatten()).all(|v| v.is_finite()), "non-finite position or uv");
    Ok(())
}

#[derive(serde::Serialize)]
pub struct Summary { pub actor: String, pub glb: String, pub joints: usize, pub vertices: usize, pub triangles: usize, pub clips: Vec<(String, f32)>, pub warnings: Vec<String> }

/// Textures for a material name: (diffuse PNG, normal PNG) bytes.
pub type TexFn<'a> = dyn FnMut(&str, bool) -> Result<Option<Vec<u8>>> + 'a;

/// One skinned mesh on the glb skeleton: `joint_map[i]` = glb joint of the mesh's own node `i`
/// (identity for the body; by bone name for an attached head, whose bind pose equals the body's).
pub struct Part<'a> { pub name: String, pub mesh: &'a xmesh_skin::SkinnedMesh, pub joint_map: Vec<usize>, pub morphs: Vec<(String, Vec<[f32; 3]>)> }

/// `blend(material)` = the game shader's (BlendMode, MaskReference) for a material name: 1 = alpha
/// test, 2/7 = blended; written as glTF alphaMode MASK (cutoff mask/255) or BLEND.
pub fn build_glb(name: &str, nodes: &[SkeletonNode], parts: &[Part], anims: &[(String, Vec<BoneMotion>)], tex: &mut TexFn, blend: &dyn Fn(&str) -> (u32, u8)) -> Result<(Vec<u8>, Summary)> {
    let globals = bind_globals(nodes)?;
    let mut bin = Bin { data: vec![], views: vec![], accessors: vec![] };
    let (mut images, mut textures, mut materials, mut gmeshes) = (vec![], vec![], vec![], vec![]);
    let mut tex_by_name: std::collections::HashMap<(String, bool), Option<usize>> = Default::default();
    let (mut tris, mut verts) = (0, 0);

    for part in parts {
        let mesh = part.mesh;
        check_mesh(mesh)?;
        let nv = mesh.positions.len();
        verts += nv;
        let pos: Vec<[f32; 3]> = mesh.positions.iter().map(|p| conv_p(*p, UNIT)).collect();
        let nrm: Vec<[f32; 3]> = mesh.normals.iter().map(|n| {
            let c = conv_p(*n, 1.0);
            let l = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
            if l > 1e-6 { [c[0] / l, c[1] / l, c[2] / l] } else { [0.0, 1.0, 0.0] }
        }).collect();
        let (mut js, mut ws): (Vec<u8>, Vec<[f32; 4]>) = (Vec::with_capacity(nv * 8), Vec::with_capacity(nv));
        for w in &mesh.skin_weights {
            let (j, x) = top4(w);
            for i in j {
                let m = *part.joint_map.get(i as usize).with_context(|| format!("joint {i} out of range"))?;
                ensure!(m < nodes.len(), "joint {m} outside the skeleton");
                js.extend_from_slice(&(m as u16).to_le_bytes());
            }
            ws.push(x);
        }
        let a_pos = bin.f32s(&pos, Some(34962), "VEC3", true);
        let a_nrm = bin.f32s(&nrm, Some(34962), "VEC3", false);
        let a_uv = bin.f32s(&mesh.uvs, Some(34962), "VEC2", false);
        let a_j = bin.acc(&js, Some(34962), 5123, nv, "VEC4", None);
        let a_w = bin.f32s(&ws, Some(34962), "VEC4", false);
        let targets: Vec<Value> = part.morphs.iter().map(|(_, d)| {
            let d: Vec<[f32; 3]> = d.iter().map(|p| conv_p(*p, UNIT)).collect();
            json!({ "POSITION": bin.f32s(&d, Some(34962), "VEC3", true) })
        }).collect();
        let mut prims = vec![];
        for (mi, m) in mesh.materials.iter().enumerate() {
            let idx: Vec<u8> = mesh.faces.iter().zip(&mesh.face_material_ids).filter(|(_, id)| **id as usize == mi).flat_map(|(f, _)| [f[0], f[2], f[1]].into_iter().flat_map(|i| i.to_le_bytes())).collect();
            if idx.is_empty() { continue; }
            let count = idx.len() / 4;
            tris += count / 3;
            let a_i = bin.acc(&idx, Some(34963), 5125, count, "SCALAR", None);
            let mut mat = json!({ "name": m.name, "pbrMetallicRoughness": { "metallicFactor": 0.0, "roughnessFactor": 1.0 } });
            match blend(&m.name) {
                (1, mask) => { mat["alphaMode"] = json!("MASK"); mat["alphaCutoff"] = json!(if mask == 0 { 0.5 } else { mask as f32 / 255.0 }); }
                (2, _) | (7, _) => { mat["alphaMode"] = json!("BLEND"); }
                _ => {}
            }
            for (file, is_normal) in [(&m.diffuse, false), (&m.normal, true)] {
                let Some(file) = file else { continue };
                let key = (file.clone(), is_normal);
                let t = match tex_by_name.get(&key) {
                    Some(t) => *t,
                    None => {
                        let t = match tex(file, is_normal)? {
                            Some(p) => {
                                let bv = bin.view(&p, None);
                                images.push(json!({ "name": file, "bufferView": bv, "mimeType": "image/png" }));
                                textures.push(json!({ "source": images.len() - 1, "sampler": 0 }));
                                Some(textures.len() - 1)
                            }
                            None => None,
                        };
                        tex_by_name.insert(key, t);
                        t
                    }
                };
                if let Some(t) = t {
                    if is_normal { mat["normalTexture"] = json!({ "index": t }); } else { mat["pbrMetallicRoughness"]["baseColorTexture"] = json!({ "index": t }); }
                }
            }
            materials.push(mat);
            let mut prim = json!({ "attributes": { "POSITION": a_pos, "NORMAL": a_nrm, "TEXCOORD_0": a_uv, "JOINTS_0": a_j, "WEIGHTS_0": a_w }, "indices": a_i, "material": materials.len() - 1 });
            if !targets.is_empty() { prim["targets"] = json!(targets); }
            prims.push(prim);
        }
        ensure!(!prims.is_empty(), "no triangles with a material");
        let mut gm = json!({ "name": part.name, "primitives": prims });
        if !part.morphs.is_empty() {
            gm["weights"] = json!(vec![0.0; part.morphs.len()]);
            gm["extras"] = json!({ "targetNames": part.morphs.iter().map(|(n, _)| n).collect::<Vec<_>>() });
        }
        gmeshes.push(gm);
    }

    let mut gnodes: Vec<Value> = nodes.iter().map(|n| json!({ "name": n.name, "translation": conv_p(n.position, UNIT), "rotation": conv_q(n.rotation) })).collect();
    for (i, n) in nodes.iter().enumerate() {
        if let Some(p) = n.parent_index {
            let mut c: Vec<Value> = serde_json::from_value(gnodes[p].get("children").cloned().unwrap_or(json!([])))?;
            c.push(json!(i));
            gnodes[p]["children"] = json!(c);
        }
    }
    let mut root_children: Vec<usize> = nodes.iter().enumerate().filter(|(_, n)| n.parent_index.is_none()).map(|(i, _)| i).collect();
    let mut skins = vec![];
    if !gmeshes.is_empty() {
        let ibm: Vec<[f32; 16]> = globals.iter().map(rigid_inverse).collect();
        let a_ibm = bin.f32s(&ibm, None, "MAT4", false);
        skins.push(json!({ "name": format!("{name}_Skin"), "joints": (0..nodes.len()).collect::<Vec<_>>(), "inverseBindMatrices": a_ibm }));
        for (i, m) in gmeshes.iter().enumerate() {
            root_children.push(gnodes.len());
            gnodes.push(json!({ "name": if i == 0 { format!("{name}_Mesh") } else { m["name"].as_str().unwrap_or("Head").to_string() }, "mesh": i, "skin": 0 }));
        }
    }
    let root = gnodes.len();
    gnodes.push(json!({ "name": name, "children": root_children }));

    let mut gl_anims = vec![];
    let mut clips = vec![];
    for (clip, motion) in anims {
        let (mut samplers, mut channels) = (vec![], vec![]);
        let mut dur = 0f32;
        for bm in motion {
            let Some(ni) = nodes.iter().position(|n| n.name == bm.bone_name) else { continue };
            let rk = keys_monotonic(&bm.rotation_keys);
            if !rk.is_empty() {
                let mut prev = conv_q(nodes[ni].rotation);
                let vals: Vec<[f32; 4]> = rk.iter().map(|k| {
                    let mut q = conv_q([k[0], k[1], k[2], k[3]]);
                    if q[0] * prev[0] + q[1] * prev[1] + q[2] * prev[2] + q[3] * prev[3] < 0.0 { q = q.map(|x| -x); }
                    prev = q;
                    q
                }).collect();
                let t: Vec<[f32; 1]> = rk.iter().map(|k| [k[4]]).collect();
                dur = dur.max(t.last().unwrap()[0]);
                let (a_t, a_v) = (bin.f32s(&t, None, "SCALAR", true), bin.f32s(&vals, None, "VEC4", false));
                samplers.push(json!({ "input": a_t, "output": a_v, "interpolation": "LINEAR" }));
                channels.push(json!({ "sampler": samplers.len() - 1, "target": { "node": ni, "path": "rotation" } }));
            }
            for (keys, path, pos) in [(&bm.position_keys, "translation", true), (&bm.scale_keys, "scale", false)] {
                let pk = keys_monotonic(keys);
                if pk.is_empty() { continue; }
                let vals: Vec<[f32; 3]> = pk.iter().map(|k| if pos { conv_p([k[0], k[1], k[2]], UNIT) } else { [k[0], k[1], k[2]] }).collect();
                let t: Vec<[f32; 1]> = pk.iter().map(|k| [k[3]]).collect();
                dur = dur.max(t.last().unwrap()[0]);
                let (a_t, a_v) = (bin.f32s(&t, None, "SCALAR", true), bin.f32s(&vals, None, "VEC3", false));
                samplers.push(json!({ "input": a_t, "output": a_v, "interpolation": "LINEAR" }));
                channels.push(json!({ "sampler": samplers.len() - 1, "target": { "node": ni, "path": path } }));
            }
        }
        if channels.is_empty() { continue; }
        clips.push((clip.clone(), dur));
        gl_anims.push(json!({ "name": clip, "samplers": samplers, "channels": channels }));
    }

    while bin.data.len() % 4 != 0 { bin.data.push(0); }
    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "risen-core" },
        "scene": 0, "scenes": [{ "name": name, "nodes": [root] }],
        "nodes": gnodes,
        "buffers": [{ "byteLength": bin.data.len() }],
        "bufferViews": bin.views, "accessors": bin.accessors,
    });
    if !gmeshes.is_empty() { doc["meshes"] = json!(gmeshes); doc["skins"] = json!(skins); doc["materials"] = json!(materials); }
    if !gl_anims.is_empty() { doc["animations"] = json!(gl_anims); }
    if !images.is_empty() {
        doc["images"] = json!(images);
        doc["textures"] = json!(textures);
        doc["samplers"] = json!([{ "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 }]);
    }
    let mut js = serde_json::to_vec(&doc)?;
    while js.len() % 4 != 0 { js.push(b' '); }
    let total = 12 + 8 + js.len() + 8 + bin.data.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(js.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
    out.extend_from_slice(&js);
    out.extend_from_slice(&(bin.data.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E4942u32.to_le_bytes());
    out.extend_from_slice(&bin.data);
    Ok((out, Summary { actor: name.into(), glb: String::new(), joints: nodes.len(), vertices: verts, triangles: tris, clips, warnings: vec![] }))
}

pub fn resolve_actor(g: &GameCtx, name: &str) -> Result<String> {
    if name.starts_with('/') { return Ok(name.to_string()); }
    let stem = name.strip_suffix("._xmac").unwrap_or(name);
    g.find_one(&format!("/{stem}._xmac"))
}

pub fn stem(entry: &str) -> String { entry.rsplit('/').next().unwrap().split('.').next().unwrap().to_string() }

/// Clips to bring along: `query` "" = the idle loop only, "*" = none, anything else = the actor's
/// clips containing its words (at most `limit`).
pub fn pick_clips(clips: &ClipIndex, actor: &str, query: &str, limit: usize) -> Vec<(String, String)> {
    match query.trim() {
        "*" => vec![],
        "" => clips.idle_for(actor).into_iter().collect(),
        // "@<file>": exactly the clips named in that file, one per line.
        q if q.starts_with('@') => std::fs::read_to_string(&q[1..]).unwrap_or_default().lines().filter_map(|n| clips.get(n.trim())).take(limit).collect(),
        q => clips.for_actor(actor, q).into_iter().take(limit).collect(),
    }
}

/// The actor with `clips` (name, entry) as a `.glb` at `out`. `with_textures` false leaves the
/// materials bare (for motions put on a skeleton already in the scene). The skin is always there:
/// without one, Blender's glTF importer makes the joints plain empties, not an armature.
pub fn write_glb(g: &GameCtx, tex: &TextureIndex, entry: &str, head: Option<&str>, clips: &[(String, String)], with_textures: bool, out: &Path, cache: &Path) -> Result<Summary> {
    let (bytes, _) = g.read(entry)?;
    let nodes = std::panic::catch_unwind(|| xmac::parse_skeleton(&bytes)).map_err(|_| anyhow::anyhow!("skeleton parser panicked"))?.context("skeleton")?;
    if nodes.len() > u16::MAX as usize { bail!("too many nodes"); }
    let mut mesh = std::panic::catch_unwind(|| xmesh_skin::parse_skinned_mesh(&bytes)).map_err(|_| anyhow::anyhow!("mesh parser panicked"))?.context("skinned mesh")?;
    textures_from_materials(g, &mut mesh);
    let mut warnings = vec![];
    let mut parts = vec![Part { name: stem(entry), mesh: &mesh, joint_map: (0..nodes.len()).collect(), morphs: morphs_of(&bytes, &mesh, &mut warnings) }];
    let head_data;
    if let Some(h) = head {
        match load_head(g, &nodes, h) { Ok(d) => { head_data = d; parts.push(Part { name: stem(&head_data.3), mesh: &head_data.0, joint_map: head_data.1.clone(), morphs: head_data.2.clone() }); } Err(e) => warnings.push(format!("head {h} not attached: {e:#}")) }
    }
    let names: Vec<String> = nodes.iter().map(|n| n.name.clone()).collect();
    let mut anims = vec![];
    for (clip, e) in clips {
        match g.read(e).and_then(|(m, _)| xmot::parse_motion(&m, &names)) {
            Ok(m) => anims.push((clip.clone(), m)),
            Err(err) => warnings.push(format!("clip {clip} skipped: {err:#}")),
        }
    }
    let stem = stem(entry);
    let (glb, mut s) = build_glb(&stem, &nodes, &parts, &anims, &mut |file: &str, normal: bool| -> Result<Option<Vec<u8>>> {
        if !with_textures { return Ok(None); }
        match tex.texture(g, file, cache, normal)? { Some(rel) => Ok(Some(std::fs::read(cache.join(rel))?)), None => Ok(None) }
    }, &|mat: &str| material_blend(g, mat))?;
    if let Some(p) = out.parent() { std::fs::create_dir_all(p)?; }
    std::fs::write(out, &glb)?;
    s.glb = out.to_string_lossy().into_owned();
    s.warnings = warnings;
    Ok(s)
}


/// A head actor (`Ani_Hero_Head_*`) mapped onto `body` by bone name. A head node the body lacks
/// (the file's own dummy root) maps to Bone_ROOT; no vertex may be weighted to such a node.
fn load_head(g: &GameCtx, body: &[SkeletonNode], name: &str) -> Result<(xmesh_skin::SkinnedMesh, Vec<usize>, Vec<(String, Vec<[f32; 3]>)>, String)> {
    let entry = resolve_actor(g, name)?;
    let (bytes, _) = g.read(&entry)?;
    let hn = xmac::parse_skeleton(&bytes).context("head skeleton")?;
    let mut mesh = xmesh_skin::parse_skinned_mesh(&bytes).context("head mesh")?;
    textures_from_materials(g, &mut mesh);
    let fallback = body.iter().position(|n| n.name.eq_ignore_ascii_case("Bone_ROOT")).unwrap_or(0);
    let map: Vec<usize> = hn.iter().map(|n| body.iter().position(|b| b.name == n.name).unwrap_or(fallback)).collect();
    for w in mesh.skin_weights.iter().flatten().filter(|w| w.1 > 0.0) {
        let n = hn.get(w.0 as usize).with_context(|| format!("head joint {} out of range", w.0))?;
        ensure!(body.iter().any(|b| b.name == n.name), "head vertex bound to {}, which the body lacks", n.name);
    }
    let morphs = morphs_of(&bytes, &mesh, &mut vec![]);
    Ok((mesh, map, morphs, entry))
}

/// A texture the actor names that the game does not have (the eyes ask for `…_Eyes_01_Diffuse_S1`,
/// the game ships `…_Diffuse_01`) is taken from the material's `._xmat`, as the engine does.
fn textures_from_materials(g: &GameCtx, mesh: &mut xmesh_skin::SkinnedMesh) {
    let has = |t: &str| g.find_one(&format!("/{t}._ximg")).is_ok();
    for m in &mut mesh.materials {
        if m.diffuse.as_deref().map_or(false, has) && m.normal.as_deref().map_or(true, has) { continue; }
        let base = m.name.split('.').next().unwrap_or(&m.name);
        let Some(t) = g.find_one(&format!("/{base}._xmat")).ok().and_then(|e| g.read(&e).ok()).and_then(|(d, _)| risen_formats::xmat::parse(&d)) else { continue };
        if !m.diffuse.as_deref().map_or(false, has) { if let Some(d) = t.diffuse { m.diffuse = Some(d); } }
        if !m.normal.as_deref().map_or(true, has) { if let Some(n) = t.normal { m.normal = Some(n); } }
    }
}

/// The face shapes (blink, the mouth shapes of speech) of an actor, per vertex of `mesh`.
fn morphs_of(bytes: &[u8], mesh: &xmesh_skin::SkinnedMesh, warnings: &mut Vec<String>) -> Vec<(String, Vec<[f32; 3]>)> {
    let m = crate::xmac_write::read(bytes).and_then(|a| crate::morph::per_vertex(&a));
    match m {
        Ok(m) if m.iter().all(|(_, d)| d.len() == mesh.positions.len()) => m,
        Ok(m) => { warnings.push(format!("morphs left out: {} targets do not match {} vertices", m.len(), mesh.positions.len())); vec![] }
        Err(_) => vec![],
    }
}

/// Human bodies (`Ani_Hero_*`, not a head itself) come without a head; the player's is the default.
pub fn default_head(actor_stem: &str) -> Option<&'static str> {
    let s = actor_stem.to_lowercase();
    (s.starts_with("ani_hero_") && !s.contains("_head")).then_some("Ani_Hero_Head_Player")
}

/// (BlendMode, MaskReference) of the game material called `name` (an actor material has its own `._xmat`).
pub fn material_blend(g: &GameCtx, name: &str) -> (u32, u8) {
    let base = name.split('.').next().unwrap_or(name);
    let Ok(e) = g.find_one(&format!("/{base}._xmat")) else { return (0, 0) };
    let Ok((d, _)) = g.read(&e) else { return (0, 0) };
    let b = crate::mesh::shader_value(&d, "BlendMode").and_then(|v| v.get(2..6).map(|x| u32::from_le_bytes(x.try_into().unwrap()))).unwrap_or(0);
    let m = crate::mesh::shader_value(&d, "MaskReference").and_then(|v| v.first().copied()).unwrap_or(0);
    (b, m)
}
