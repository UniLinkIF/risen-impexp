//! A new skinned mesh on an existing actor's skeleton: builds the XAC mesh, skin and material
//! sections (layout in `xmac_write.rs`) from Blender's data and swaps them into the base actor.

use crate::xmac_write::{Actor, Layer, Map, Material, Mesh, Section, Skin, SubMesh};
use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeSet, HashMap};

/// A skinned mesh in game space (the mesh exporter's axes): `positions` per Blender vertex (the
/// actor's "final" vertices), corners referring to them with their own normal and uv, a material
/// slot per triangle, and up to 4 (bone, weight) per vertex (`bone` indexes `bone_nodes`).
pub struct SkinInput {
    pub positions: Vec<[f32; 3]>,
    pub corner_vertex: Vec<u32>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub tri_material: Vec<u32>,
    pub weights: Vec<[(u32, f32); 4]>,
    /// Shape keys by name: a game-space delta per vertex (as `positions`).
    pub morphs: Vec<(String, Vec<[f32; 3]>)>,
}

/// A material of the new mesh: one the base actor has (same name) is kept with its maps;
/// otherwise `diffuse` / `normal` name the textures (their `._ximg` are the caller's).
pub struct NewMaterial { pub name: String, pub diffuse: Option<String>, pub normal: Option<String>, pub specular: Option<String> }

/// `base` with its first mesh (and that mesh's skin) replaced by `input`. Other meshes and their
/// skins go (they described the old model); skeleton and the rest stay. The face shapes (morph
/// targets) come from `input.morphs` by name, keeping the base's phoneme sets so lip-sync still
/// finds them; without any, they go too.
pub fn replace_mesh(base: &Actor, input: &SkinInput, materials: &[NewMaterial], bone_nodes: &[u32]) -> Result<Actor> {
    let mut a = base.clone();
    let old = a.sections.iter().find_map(|s| match s { Section::Mesh(m) => Some(m.clone()), _ => None }).context("the base actor has no mesh")?;
    let old_skin = a.sections.iter().find_map(|s| match s { Section::Skin(k) if k.node == old.node => Some(k.clone()), _ => None }).context("the base actor's mesh has no skin")?;
    let tag = old.layers.first().map(|l| l.tag).unwrap_or(0x1002);
    let layer = |kind: u32, keep: u16| old.layers.iter().find(|l| l.kind == kind).map(|l| (l.keep, l.tag)).unwrap_or((keep, tag));

    let (version, base_mats) = a.sections.iter().find_map(|s| match s { Section::Materials { version, list, .. } => Some((*version, list.clone())), _ => None }).context("the base actor has no materials")?;
    let proto_version = base_mats.first().map(|m| m.version).unwrap_or(2);
    let mut list = vec![];
    for (i, m) in materials.iter().enumerate() {
        let mut mat = match base_mats.iter().find(|b| b.name.eq_ignore_ascii_case(&m.name)) {
            Some(b) => b.clone(),
            None => {
                let maps = [(2u8, &m.diffuse), (5u8, &m.normal), (3u8, &m.specular)].into_iter()
                    .filter_map(|(kind, t)| t.as_ref().map(|t| Map { params: [1.0, 0.0, 0.0, 1.0, 1.0, 0.0], material: 0, kind, flag: 0, texture: t.clone() }))
                    .collect();
                Material { version: proto_version, colors: [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 25.0, 0.0, 1.0, 1.0], u16a: 0, transparency: 70, name: m.name.clone(), maps }
            }
        };
        for mp in &mut mat.maps { mp.material = i as u16; }
        list.push(mat);
    }
    let nmat = list.len() as u32;

    // Raw vertices: one per (vertex, normal, uv) inside each material, grouped by material.
    let nc = input.corner_vertex.len();
    ensure!(nc % 3 == 0 && input.normals.len() == nc && input.uvs.len() == nc && input.tri_material.len() == nc / 3, "corner arrays disagree");
    ensure!(input.weights.len() == input.positions.len(), "{} weight lists for {} vertices", input.weights.len(), input.positions.len());
    let (mut base_v, mut pos, mut nrm, mut uv) = (vec![], vec![], vec![], vec![]);
    let mut raw_tris: Vec<[u32; 3]> = vec![];
    let mut subs = vec![];
    for mi in 0..nmat {
        let first_raw = base_v.len() as u32;
        let mut seen = HashMap::new();
        let (mut indices, mut used) = (vec![], BTreeSet::new());
        for t in 0..nc / 3 {
            if input.tri_material[t] != mi { continue; }
            let tri = [0, 1, 2].map(|k| {
                let c = t * 3 + k;
                let v = input.corner_vertex[c];
                *seen.entry((v, input.normals[c].map(f32::to_bits), input.uvs[c].map(f32::to_bits))).or_insert_with(|| {
                    base_v.push(v);
                    pos.push(input.positions[v as usize]);
                    nrm.push(input.normals[c]);
                    uv.push(input.uvs[c]);
                    base_v.len() as u32 - 1
                })
            });
            for &r in &tri { for &(b, w) in &input.weights[base_v[r as usize] as usize] { if w > 0.0 { used.insert(bone_nodes[b as usize]); } } }
            indices.extend(tri.iter().map(|r| r - first_raw));
            raw_tris.push(tri);
        }
        if indices.is_empty() { continue; }
        subs.push(SubMesh { vertices: base_v.len() as u32 - first_raw, material: mi, indices, bones: used.into_iter().collect() });
    }
    if subs.is_empty() { bail!("the mesh has no triangles"); }
    let mut tv: Vec<crate::xmsh_write::Vertex> = (0..pos.len()).map(|i| crate::xmsh_write::Vertex { pos: pos[i], normal: nrm[i], tangent: [0.0; 3], right_handed: true, uv: uv[i] }).collect();
    crate::xmsh_write::compute_tangents(&mut tv, &raw_tris);

    let f3 = |v: &[[f32; 3]]| v.iter().flatten().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let mk = |kind: u32, size: u32, keep: u16, data: Vec<u8>| { let (k, t) = layer(kind, keep); Layer { kind, size, keep: k, tag: t, data } };
    let layers = vec![
        mk(5, 4, 0, base_v.iter().flat_map(|v| v.to_le_bytes()).collect()),
        mk(0, 12, 1, f3(&pos)),
        mk(1, 12, 1, f3(&nrm)),
        mk(3, 8, 0, uv.iter().flatten().flat_map(|x| x.to_le_bytes()).collect()),
        mk(2, 16, 1, tv.iter().flat_map(|v| { let h = if v.right_handed { 1.0f32 } else { -1.0 }; [v.tangent[0], v.tangent[1], v.tangent[2], h] }).flat_map(|x| x.to_le_bytes()).collect()),
    ];
    let mesh = Mesh { version: old.version, node: old.node, final_vertices: input.positions.len() as u32, raw_vertices: base_v.len() as u32, flags: old.flags, layers, subs };

    // Skin per final vertex: up to 4 influences, renormalised; unweighted vertices follow bone 0.
    let itag = old_skin.influences.first().map(|i| i.2).unwrap_or(0);
    let (mut influences, mut ranges, mut bones) = (vec![], vec![], BTreeSet::new());
    for w in &input.weights {
        let mut v: Vec<(u32, f32)> = w.iter().copied().filter(|x| x.1 > 0.0).collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let sum: f32 = v.iter().map(|x| x.1).sum();
        if sum <= 0.0 { v = vec![(0, 1.0)]; }
        let sum = if sum > 0.0 { sum } else { 1.0 };
        ranges.push((influences.len() as u32, v.len() as u32));
        for (b, x) in v { let node = bone_nodes[b as usize]; bones.insert(node); influences.push((x / sum, node as u16, itag)); }
    }
    let skin = Skin { version: old_skin.version, node: old.node, local_bones: bones.len() as u32, skin_index: old_skin.skin_index, influences, ranges };
    let mut new_morphs = morph_body(&a, input, &base_v, old.node)?;

    let mut out = vec![];
    let mut done = false;
    for s in a.sections.drain(..) {
        match s {
            Section::Mesh(_) if !done => { out.push(Section::Mesh(mesh.clone())); out.push(Section::Skin(skin.clone())); done = true; }
            Section::Raw { id: 12, version, .. } => if let Some(body) = new_morphs.take() { out.push(Section::Raw { id: 12, version, body }) },
            Section::Mesh(_) | Section::Skin(_) => {}
            Section::Materials { .. } => out.push(Section::Materials { version, counts: [nmat, nmat, 0], list: list.clone() }),
            other => out.push(other),
        }
    }
    a.sections = out;
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &input.positions { for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
    if let Some(p) = a.resource.section.root.props.iter_mut().find(|p| p.name == "Boundary") { p.value = crate::gr01::Value::Raw(crate::gr01::bbox_bytes(lo, hi)); }
    a.resource.filetime = crate::cache::filetime(std::time::SystemTime::now()).to_le_bytes();
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xmac_write::{read, write};

    /// A shipped actor's own mesh fed through `replace_mesh` reads back (the formats crate's parser) with the
    /// same faces, positions and uvs, and the same skeleton.
    #[test]
    fn replacing_an_actor_with_its_own_mesh_keeps_it() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        for name in ["Ani_Hero_Armor_Player", "Ani_Wolf_Monster_Wolf", "Ani_Hero_Head_Player"] {
            let d = g.read(&g.find_one(&format!("/{name}._xmac")).unwrap()).unwrap().0;
            let base = read(&d).unwrap();
            let m = base.sections.iter().find_map(|s| match s { Section::Mesh(m) => Some(m.clone()), _ => None }).unwrap();
            let sk = base.sections.iter().find_map(|s| match s { Section::Skin(k) if k.node == m.node => Some(k.clone()), _ => None }).unwrap();
            let get = |kind: u32| m.layers.iter().find(|l| l.kind == kind).unwrap().data.clone();
            let f = |b: &[u8], i: usize| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
            let (bv, p, n, t) = (get(5), get(0), get(1), get(3));
            let raw = m.raw_vertices as usize;
            let base_of: Vec<u32> = (0..raw).map(|i| u32::from_le_bytes(bv[i * 4..i * 4 + 4].try_into().unwrap())).collect();
            let mut positions = vec![[0f32; 3]; m.final_vertices as usize];
            for r in 0..raw { positions[base_of[r] as usize] = [f(&p, r * 3), f(&p, r * 3 + 1), f(&p, r * 3 + 2)]; }
            let (mut cv, mut cn, mut cu, mut tm) = (vec![], vec![], vec![], vec![]);
            let mut off = 0usize;
            for s in &m.subs {
                for tri in s.indices.chunks_exact(3) {
                    for &i in tri { let r = off + i as usize; cv.push(base_of[r]); cn.push([f(&n, r * 3), f(&n, r * 3 + 1), f(&n, r * 3 + 2)]); cu.push([f(&t, r * 2), f(&t, r * 2 + 1)]); }
                    tm.push(s.material);
                }
                off += s.vertices as usize;
            }
            let mut bone_nodes: Vec<u32> = sk.influences.iter().map(|i| i.1 as u32).collect();
            bone_nodes.sort();
            bone_nodes.dedup();
            let weights: Vec<[(u32, f32); 4]> = sk.ranges.iter().map(|&(f0, c)| {
                let mut w = [(0u32, 0f32); 4];
                for k in 0..c.min(4) as usize { let (x, node, _) = sk.influences[f0 as usize + k]; w[k] = (bone_nodes.iter().position(|&b| b == node as u32).unwrap() as u32, x); }
                w
            }).collect();
            let list = base.sections.iter().find_map(|s| match s { Section::Materials { list, .. } => Some(list.clone()), _ => None }).unwrap();
            let mats: Vec<NewMaterial> = list.iter().map(|m| NewMaterial { name: m.name.clone(), diffuse: None, normal: None, specular: None }).collect();
            // Face shapes per final vertex, from the raw vertices the game lists them on.
            let per_final = |a: &Actor| -> Vec<(String, Vec<[f32; 3]>)> {
                let mm = a.sections.iter().find_map(|s| match s { Section::Mesh(m) => Some(m.clone()), _ => None }).unwrap();
                let bv: Vec<u32> = mm.layers.iter().find(|l| l.kind == 5).unwrap().data.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
                crate::morph::per_vertex(a).unwrap().into_iter().map(|(n, d)| { let mut o = vec![[0f32; 3]; mm.final_vertices as usize]; for (r, &v) in bv.iter().enumerate() { o[v as usize] = d[r]; } (n, o) }).collect()
            };
            let morphs = per_final(&base);
            let input = SkinInput { positions, corner_vertex: cv, normals: cn, uvs: cu, tri_material: tm, weights, morphs: morphs.clone() };
            let out = write(&replace_mesh(&base, &input, &mats, &bone_nodes).unwrap());
            let back = per_final(&read(&out).unwrap());
            assert_eq!(back.len(), morphs.len(), "{name}: face shapes");
            for ((n0, d0), (n1, d1)) in morphs.iter().zip(&back) { assert_eq!(n0, n1); assert!(d0.iter().zip(d1).all(|(a, b)| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4)), "{name}: {n0} moved"); }
            let (a0, a1) = (risen_formats::xmesh_skin::parse_skinned_mesh(&d).unwrap(), risen_formats::xmesh_skin::parse_skinned_mesh(&out).unwrap());
            let nf = m.subs.iter().map(|s| s.indices.len() / 3).sum::<usize>();
            let corners = |s: &risen_formats::xmesh_skin::SkinnedMesh| {
                let mut v: Vec<(u32, [i64; 5])> = s.faces.iter().zip(&s.face_material_ids).take(nf).flat_map(|(f, &mid)| f.iter().map(move |&i| {
                    let (q, u) = (s.positions[i as usize], s.uvs[i as usize]);
                    (mid, [(q[0] * 100.0) as i64, (q[1] * 100.0) as i64, (q[2] * 100.0) as i64, (u[0] * 1e4) as i64, (u[1] * 1e4) as i64])
                })).collect();
                v.sort();
                v
            };
            assert_eq!(corners(&a0), corners(&a1), "{name}: faces differ");
            // Weights: every corner keeps the same (bone, weight) set.
            let wset = |s: &risen_formats::xmesh_skin::SkinnedMesh| { let mut v: Vec<Vec<(u32, i64)>> = s.faces.iter().take(nf).flatten().map(|&i| { let mut w: Vec<(u32, i64)> = s.skin_weights[i as usize].iter().map(|(b, x)| (*b, (x * 1000.0).round() as i64)).collect(); w.sort(); w }).collect(); v.sort(); v };
            assert_eq!(wset(&a0), wset(&a1), "{name}: weights differ");
            assert_eq!(risen_formats::xmac::parse_skeleton(&out).unwrap().len(), risen_formats::xmac::parse_skeleton(&d).unwrap().len());
            eprintln!("{name}: {nf} faces rebuilt; {} -> {} bytes", d.len(), out.len());
        }
    }
}

#[derive(serde::Deserialize)]
pub struct MatSpec { pub name: String, pub diffuse: Option<String>, pub normal: Option<String>, #[serde(default)] pub specular: Option<String>, #[serde(default)] pub alpha_test: Option<u8> }

#[derive(serde::Deserialize)]
pub struct ActorSpec {
    /// The actor whose skeleton the mesh is skinned to (as imported into Blender).
    pub base: String,
    /// Actor to write: the base's own name replaces it, a new name adds one beside it.
    pub name: String,
    pub geometry: String,
    pub vertices: usize,
    pub corners: usize,
    pub bones: Vec<String>,
    pub materials: Vec<MatSpec>,
    /// Shape keys: name and a file of f32[3] per vertex, game space.
    #[serde(default)]
    pub morphs: Vec<MorphSpec>,
}

#[derive(serde::Deserialize)]
pub struct MorphSpec { pub name: String, pub file: String }

#[derive(serde::Serialize)]
pub struct ActorReport { pub actor: String, pub path: String, pub replaced: bool, pub vertices: usize, pub triangles: usize, pub bones: usize, pub materials: Vec<String> }

fn read_input(spec: &ActorSpec) -> Result<SkinInput> {
    let d = std::fs::read(&spec.geometry).with_context(|| format!("read {}", spec.geometry))?;
    let (v, n) = (spec.vertices, spec.corners);
    let want = v * 12 + n * 4 + n * 12 + n * 8 + n / 3 * 4 + v * 32;
    ensure!(d.len() == want, "skinned geometry is {} bytes, expected {want}", d.len());
    let mut at = 0;
    let mut f = || { let x = f32::from_le_bytes(d[at..at + 4].try_into().unwrap()); at += 4; x };
    let positions = (0..v).map(|_| [f(), f(), f()]).collect();
    let mut at2 = v * 12;
    let mut u = || { let x = u32::from_le_bytes(d[at2..at2 + 4].try_into().unwrap()); at2 += 4; x };
    let corner_vertex: Vec<u32> = (0..n).map(|_| u()).collect();
    let fl = |a: usize| f32::from_le_bytes(d[a..a + 4].try_into().unwrap());
    let ul = |a: usize| u32::from_le_bytes(d[a..a + 4].try_into().unwrap());
    let o = v * 12 + n * 4;
    let normals = (0..n).map(|i| [fl(o + i * 12), fl(o + i * 12 + 4), fl(o + i * 12 + 8)]).collect();
    let o = o + n * 12;
    let uvs = (0..n).map(|i| [fl(o + i * 8), fl(o + i * 8 + 4)]).collect();
    let o = o + n * 8;
    let tri_material = (0..n / 3).map(|i| ul(o + i * 4)).collect();
    let o = o + n / 3 * 4;
    let weights = (0..v).map(|i| [0, 1, 2, 3].map(|k| (ul(o + i * 32 + k * 8), fl(o + i * 32 + k * 8 + 4)))).collect();
    ensure!(corner_vertex.iter().all(|&c| (c as usize) < v), "corner refers past the vertices");
    let morphs = spec.morphs.iter().map(|m| -> Result<(String, Vec<[f32; 3]>)> {
        let b = std::fs::read(&m.file).with_context(|| format!("read {}", m.file))?;
        ensure!(b.len() == v * 12, "shape key {}: {} bytes for {v} vertices", m.name, b.len());
        Ok((m.name.clone(), b.chunks_exact(12).map(|c| [0, 4, 8].map(|k| f32::from_le_bytes(c[k..k + 4].try_into().unwrap()))).collect()))
    }).collect::<Result<_>>()?;
    let mut s = SkinInput { positions, corner_vertex, normals, uvs, tri_material, weights, morphs };
    to_skin_winding(&mut s);
    Ok(s)
}

/// Blender's faces run counter-clockwise along their normals; an actor's skin stores them the other
/// way round once mirrored into game space (see `actor.rs`), so every triangle swaps corners 1 and 2.
fn to_skin_winding(s: &mut SkinInput) {
    for t in 0..s.corner_vertex.len() / 3 {
        let (a, b) = (t * 3 + 1, t * 3 + 2);
        s.corner_vertex.swap(a, b);
        s.normals.swap(a, b);
        s.uvs.swap(a, b);
    }
}

pub fn build(g: &crate::game::GameCtx, spec: &ActorSpec) -> Result<(ActorReport, Vec<(String, Vec<u8>)>)> {
    ensure!(!spec.name.is_empty() && spec.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'), "actor name {:?}: letters, digits and _ only", spec.name);
    let entry = crate::actor::resolve_actor(g, &spec.base)?;
    let bytes = g.read(&entry)?.0;
    let base = crate::xmac_write::read(&bytes)?;
    let nodes = risen_formats::xmac::parse_skeleton(&bytes)?;
    let bone_nodes: Vec<u32> = spec.bones.iter().map(|b| nodes.iter().position(|n| &n.name == b).map(|i| i as u32).with_context(|| format!("vertex group {b} is not a bone of {}", spec.base))).collect::<Result<_>>()?;
    let input = read_input(spec)?;
    let now = crate::cache::filetime(std::time::SystemTime::now());
    let mut files = vec![];
    let base_names: Vec<String> = base.sections.iter().find_map(|s| match s { Section::Materials { list, .. } => Some(list.iter().map(|m| m.name.to_lowercase()).collect()), _ => None }).unwrap_or_default();
    let mut mats = vec![];
    let mut notes = vec![];
    for m in &spec.materials {
        let name = m.name.rsplit_once('.').filter(|(_, n)| n.len() == 3 && n.bytes().all(|c| c.is_ascii_digit())).map(|(a, _)| a).unwrap_or(&m.name).to_string();
        if base_names.contains(&name.to_lowercase()) { notes.push(format!("{name}: kept")); mats.push(NewMaterial { name, diffuse: None, normal: None, specular: None }); continue; }
        // Texture names follow a game material template (its stem length), and that template is
        // written as the material's ._xmat too, so the shader knows alpha test and specular.
        // Skinned meshes need a skinned shader: the hero's cloth (alpha test) or the hero's hands (specular). The opaque
        // template is a weapon's static shader, on which a body does not move; a missing specular or normal map is
        // written flat instead.
        let t = if m.alpha_test.is_some() { &crate::export::ALPHA_TEST } else { &crate::export::OPAQUE_SPECULAR };
        let stem = crate::export::stem_for(&crate::export::material_key(&name, &[&m.diffuse, &m.normal, &m.specular]), t.stem.len());
        let mut tex = |png: &Option<String>, suffix: &str, kind: u8| -> Result<Option<String>> {
            let Some(p) = png else { return Ok(None) };
            let (px, w, h) = crate::ximg_write::load_png(std::path::Path::new(p)).with_context(|| format!("material {name}: {p}"))?;
            let tname = format!("{stem}{suffix}");
            let (px, k) = match kind { 5 => (crate::ximg_write::to_dxt5nm(&px), crate::ximg_write::Kind::Dxt5), 2 if t.alpha_test => (px, crate::ximg_write::Kind::Dxt5), _ => (px, crate::ximg_write::Kind::Dxt1) };
            files.push((format!("data/compiled/images/BlenderMod/{tname}._ximg"), crate::ximg_write::write(&px, w, h, k, now)?));
            Ok(Some(tname))
        };
        let diffuse = tex(&m.diffuse, t.diffuse, 2)?;
        let normal = tex(&m.normal, t.normal, 5)?;
        let specular = match t.specular { Some(sfx) => tex(&m.specular, sfx, 3)?, None => None };
        if normal.is_none() {
            let tname = format!("{stem}{}", t.normal);
            files.push((format!("data/compiled/images/BlenderMod/{tname}._ximg"), crate::ximg_write::write(&crate::ximg_write::to_dxt5nm(&[128, 128, 255, 255].repeat(16)), 4, 4, crate::ximg_write::Kind::Dxt5, now)?));
        }
        if let (Some(sfx), None) = (t.specular, &specular) {
            let tname = format!("{stem}{sfx}");
            files.push((format!("data/compiled/images/BlenderMod/{tname}._ximg"), crate::ximg_write::write(&[24, 24, 24, 255].repeat(16), 4, 4, crate::ximg_write::Kind::Dxt1, now)?));
        }
        let mat_name = format!("{stem}{}", t.diffuse);
        files.push((format!("data/common/materials/{mat_name}._xmat"), crate::export::material_from_template(g, t, &stem, m.alpha_test)?));
        notes.push(format!("{name}: new {mat_name}{}", if diffuse.is_none() { " (no Base Color image: untextured)" } else { "" }));
        let name = mat_name;
        mats.push(NewMaterial { name, diffuse, normal, specular });
    }
    let actor = replace_mesh(&base, &input, &mats, &bone_nodes)?;
    let replaced = spec.name.eq_ignore_ascii_case(&crate::actor::stem(&entry));
    let rel = g.rel(&entry)?;
    let path = if replaced { rel } else { format!("{}/{}._xmac", rel.rsplit_once('/').map(|x| x.0).unwrap_or("data/compiled/animations"), spec.name) };
    files.push((path.clone(), crate::xmac_write::write(&actor)));
    let report = ActorReport { actor: spec.name.clone(), path, replaced, vertices: input.positions.len(), triangles: input.corner_vertex.len() / 3, bones: bone_nodes.len(), materials: notes };
    Ok((report, files))
}

/// Section 12 for the new mesh: the base's targets that `input` has a shape key for (same name),
/// with the base's phoneme sets and ranges, then any new shape keys. Deltas go on the raw
/// vertices (`base_v` = each raw vertex's Blender vertex). None when there is nothing to write.
fn morph_body(base: &Actor, input: &SkinInput, base_v: &[u32], node: u32) -> Result<Option<Vec<u8>>> {
    let Some(body) = base.sections.iter().find_map(|s| match s { Section::Raw { id: 12, body, .. } => Some(body), _ => None }) else { return Ok(None) };
    if input.morphs.is_empty() { return Ok(None); }
    let m = crate::morph::read(body)?;
    for (name, d) in &input.morphs { ensure!(d.len() == input.positions.len(), "shape key {name}: {} deltas for {} vertices", d.len(), input.positions.len()); }
    let deltas = |d: &Vec<[f32; 3]>| vec![crate::morph::MeshDeltas { node, deltas: base_v.iter().enumerate().filter(|(_, &v)| d[v as usize].iter().any(|x| x.abs() > 1e-5)).map(|(r, &v)| (r as u32, d[v as usize])).collect() }];
    let mut targets: Vec<crate::morph::Target> = m.targets.iter().filter_map(|t| input.morphs.iter().find(|(n, _)| n == &t.name).map(|(_, d)| crate::morph::Target { meshes: deltas(d), ..t.clone() })).collect();
    for (name, d) in &input.morphs {
        if !m.targets.iter().any(|t| &t.name == name) {
            targets.push(crate::morph::Target { name: name.clone(), range: (0.0, 1.0), lod: m.lod, phonemes: 0, meshes: deltas(d), transforms: vec![] });
        }
    }
    Ok(Some(crate::morph::write(&crate::morph::Morphs { lod: m.lod, tangents: m.tangents, targets })))
}
