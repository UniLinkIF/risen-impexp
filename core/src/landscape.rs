//! The island's ground (`Levelmesh_Landscape_01._xmsh`, one mesh, 376 034 vertices) to Blender and
//! back, height edits only in spirit: the file keeps its topology and is patched in place
//! (`terrain.rs`), and the
//! 145 collision sectors follow (`terrain_col.rs`).
//!
//! For Blender the file's vertices are welded by exact position (the file splits them at uv
//! seams), first appearance first — the same rule on the way back, so Blender's vertex i is always
//! welded vertex i. The user may move vertices; adding or removing them is refused.
//!
//! ```text
//! get  → u32 nv · f32 pos[3·nv] (game cm) · u32 nt · u32 tri[3·nt] · f32 uv[2·3·nt] · u32 submesh[nt]
//! set  ← u32 nv · f32 pos[3·nv]
//! ```

use crate::game::GameCtx;
use crate::{terrain, terrain_col};
use anyhow::{ensure, Context, Result};
use std::collections::HashMap;
use std::path::Path;

type V3 = [f32; 3];

pub fn entry(g: &GameCtx) -> Result<String> { g.find_one(terrain::LANDSCAPE_ENTRY_SUFFIX) }

/// File vertices welded by exact position: (unique positions, file vertex → unique index).
pub fn weld(pos: &[V3]) -> (Vec<V3>, Vec<u32>) {
    let mut seen: HashMap<[u32; 3], u32> = HashMap::new();
    let mut uniq = vec![];
    let map = pos.iter().map(|p| *seen.entry(p.map(f32::to_bits)).or_insert_with(|| { uniq.push(*p); uniq.len() as u32 - 1 })).collect();
    (uniq, map)
}

#[derive(serde::Serialize)]
pub struct GetReport { pub vertices: usize, pub triangles: usize, pub materials: Vec<crate::mesh::Material>, pub geometry: String }

/// The landscape for an editor, with textures. `current = false`: the archive's (vanilla) ground;
/// `current = true`: what the game shows now (the loose file first, i.e. the installed Landscape mod).
///
/// The weld map and the triangles always come from the ARCHIVE, only the positions from the current
/// file: welding the edited file by position could merge two vertices an edit happened to put on
/// the same spot (flatten a cliff), and then welded vertex i would no longer mean the same vertex
/// on the way back. `set` is absolute against the archive too, so getting the current ground and
/// setting it back unchanged is a no-op, and edits never stack.
pub fn get(g: &GameCtx, out_dir: &Path, current: bool) -> Result<GetReport> {
    let e = entry(g)?;
    let src = g.read_archive(&e)?;
    let m = crate::xmsh_geom::decode(&src)?;
    let (mut uniq, map) = weld(&m.positions);
    let mut sub_of = crate::landscape_paint::sub_of(&m);
    if current {
        let (cur, _) = g.read(&e)?;
        // A painted file has its own vertex order and copies: map it back onto the archive's.
        let (pos, sub) = crate::landscape_paint::recover(&src, &cur).context("read the installed landscape")?;
        let mut done = vec![false; uniq.len()];
        for (i, &u) in map.iter().enumerate() { if !done[u as usize] { uniq[u as usize] = pos[i]; done[u as usize] = true; } }
        sub_of = sub;
    }
    let tex = crate::mesh::TextureIndex::build(g);
    let mut warnings = vec![];
    let materials = m.submeshes.iter().map(|s| crate::mesh::material(g, &tex, &s.material, out_dir, &mut warnings)).collect::<Result<Vec<_>>>()?;
    let mut b = (uniq.len() as u32).to_le_bytes().to_vec();
    for p in &uniq { for x in p { b.extend(x.to_le_bytes()); } }
    b.extend(((m.indices.len() / 3) as u32).to_le_bytes());
    for &i in &m.indices { b.extend(map[i as usize].to_le_bytes()); }
    for &i in &m.indices { let uv = m.uvs.get(i as usize).copied().unwrap_or([0.0; 2]); b.extend(uv[0].to_le_bytes()); b.extend((1.0 - uv[1]).to_le_bytes()); }
    for s in &sub_of { b.extend(s.to_le_bytes()); }
    std::fs::create_dir_all(out_dir)?;
    let path = out_dir.join("landscape.bin");
    std::fs::write(&path, b)?;
    Ok(GetReport { vertices: uniq.len(), triangles: m.indices.len() / 3, materials, geometry: path.to_string_lossy().into_owned() })
}

/// Height change field of an edit, for points that are not landscape vertices (collision): the
/// change at the vertices, interpolated across the ORIGINAL triangle under (x, z) whose surface
/// lies nearest to y.
struct Field { old: Vec<V3>, dy: Vec<f32>, tris: Vec<[u32; 3]>, cell: f32, grid: HashMap<(i32, i32), Vec<u32>> }

impl Field {
    fn new(old: Vec<V3>, new: &[V3], tris: Vec<[u32; 3]>) -> Field {
        let dy = old.iter().zip(new).map(|(a, b)| b[1] - a[1]).collect();
        let cell = 2000.0;
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (t, tri) in tris.iter().enumerate() {
            let ps = tri.map(|i| old[i as usize]);
            let (x0, x1) = (ps.iter().map(|p| p[0]).fold(f32::MAX, f32::min), ps.iter().map(|p| p[0]).fold(f32::MIN, f32::max));
            let (z0, z1) = (ps.iter().map(|p| p[2]).fold(f32::MAX, f32::min), ps.iter().map(|p| p[2]).fold(f32::MIN, f32::max));
            for cx in (x0 / cell).floor() as i32..=(x1 / cell).floor() as i32 { for cz in (z0 / cell).floor() as i32..=(z1 / cell).floor() as i32 { grid.entry((cx, cz)).or_default().push(t as u32); } }
        }
        Field { old, dy, tris, cell, grid }
    }
    /// The ORIGINAL triangle under (x, z) whose surface lies nearest to y.
    fn tri_at(&self, x: f32, y: f32, z: f32) -> Option<u32> {
        let cands = self.grid.get(&((x / self.cell).floor() as i32, (z / self.cell).floor() as i32))?;
        let mut best: Option<(f32, u32)> = None;
        for &t in cands {
            let [a, b, c] = self.tris[t as usize].map(|i| i as usize);
            let (pa, pb, pc) = (self.old[a], self.old[b], self.old[c]);
            let d = (pb[2] - pc[2]) * (pa[0] - pc[0]) + (pc[0] - pb[0]) * (pa[2] - pc[2]);
            if d.abs() < 1e-6 { continue; }
            let wa = ((pb[2] - pc[2]) * (x - pc[0]) + (pc[0] - pb[0]) * (z - pc[2])) / d;
            let wb = ((pc[2] - pa[2]) * (x - pc[0]) + (pa[0] - pc[0]) * (z - pc[2])) / d;
            let wc = 1.0 - wa - wb;
            if wa < -1e-3 || wb < -1e-3 || wc < -1e-3 { continue; }
            let dist = (wa * pa[1] + wb * pb[1] + wc * pc[1] - y).abs();
            if best.map_or(true, |(bd, _)| dist < bd) { best = Some((dist, t)); }
        }
        best.map(|b| b.1)
    }
    fn at(&self, x: f32, y: f32, z: f32) -> f32 {
        let Some(cands) = self.grid.get(&((x / self.cell).floor() as i32, (z / self.cell).floor() as i32)) else { return 0.0 };
        let mut best: Option<(f32, f32)> = None;
        for &t in cands {
            let [a, b, c] = self.tris[t as usize].map(|i| i as usize);
            let (pa, pb, pc) = (self.old[a], self.old[b], self.old[c]);
            let d = (pb[2] - pc[2]) * (pa[0] - pc[0]) + (pc[0] - pb[0]) * (pa[2] - pc[2]);
            if d.abs() < 1e-6 { continue; }
            let wa = ((pb[2] - pc[2]) * (x - pc[0]) + (pc[0] - pb[0]) * (z - pc[2])) / d;
            let wb = ((pc[2] - pa[2]) * (x - pc[0]) + (pa[0] - pc[0]) * (z - pc[2])) / d;
            let wc = 1.0 - wa - wb;
            if wa < -1e-4 || wb < -1e-4 || wc < -1e-4 { continue; }
            let sy = wa * pa[1] + wb * pb[1] + wc * pc[1];
            let dist = (sy - y).abs();
            if best.map_or(true, |(bd, _)| dist < bd) { best = Some((dist, wa * self.dy[a] + wb * self.dy[b] + wc * self.dy[c])); }
        }
        best.map(|b| b.1).unwrap_or(0.0)
    }
}

#[derive(serde::Serialize, Default)]
pub struct SetReport {
    pub moved_vertices: usize, pub max_offset_cm: f32, pub boxes_rewritten: usize, pub sectors: Vec<String>,
    /// Triangles whose ground material changed (0 without a paint file), and the rebuilt submesh table.
    pub repainted_triangles: usize,
    pub paint: Option<crate::landscape_paint::PaintReport>,
    /// Ground material → the shape material its collision gets (footsteps, sounds), when painted.
    pub shape_materials: Vec<(String, String)>,
}

/// A collision sector of the landscape: entity name, translation (cm), archive entry and bytes.
struct Sector { name: String, p: V3, entry: String, src: Vec<u8>, rot_ok: bool }

fn sectors(g: &GameCtx) -> Result<Vec<Sector>> {
    let (cd, _) = g.read(terrain_col::COL_LRENT)?;
    let doc = risen_formats::lrent::LrentDoc::parse(&cd).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let mut out = vec![];
    for en in doc.entities() {
        let mx = &en.matrix;
        let Ok(ce) = g.find_one(&format!("/{}_COL._xcom", en.name)) else { continue };
        let rot_ok = (mx[0] - 1.0).abs() < 1e-5 && (mx[5] - 1.0).abs() < 1e-5 && (mx[10] - 1.0).abs() < 1e-5 && [mx[1], mx[2], mx[4], mx[6], mx[8], mx[9]].iter().all(|x| x.abs() < 1e-5);
        out.push(Sector { name: en.name.clone(), p: [mx[12], mx[13], mx[14]], src: g.read_archive(&ce)?, entry: ce, rot_ok });
    }
    Ok(out)
}

/// The render triangle (archive index) under every collision triangle of a sector, per stream:
/// the triangle whose ORIGINAL surface is nearest to the collision triangle's centre.
fn matched(field: &Field, s: &Sector, x: &crate::nxs::Xcom) -> Vec<Vec<Option<u32>>> {
    x.meshes.iter().map(|m| m.tris.iter().map(|t| {
        let c = [0, 1, 2].map(|k| (m.verts[t[0] as usize][k] + m.verts[t[1] as usize][k] + m.verts[t[2] as usize][k]) / 3.0);
        field.tri_at(s.p[0] + c[0] * 100.0, s.p[1] + c[1] * 100.0, s.p[2] + c[2] * 100.0)
    }).collect()).collect()
}

/// Shape material per ground material, as the shipped collision wears it (the most common shape
/// over the collision triangles lying on that material), else by the material's name.
fn shape_table(names: &[String], arch_sub: &[u32], all: &[&(crate::nxs::Xcom, Vec<Vec<Option<u32>>>)]) -> Vec<u8> {
    let mut counts = vec![HashMap::<u8, usize>::new(); names.len()];
    for (x, m) in all.iter().map(|a| (&a.0, &a.1)) {
        for (si, tris) in m.iter().enumerate() {
            let shape = x.shape_materials.get(si).map(|s| s.material).unwrap_or(0);
            for t in tris.iter().flatten() { *counts[arch_sub[*t as usize] as usize].entry(shape).or_default() += 1; }
        }
    }
    counts.iter().enumerate().map(|(k, c)| c.iter().max_by_key(|(s, n)| (**n, std::cmp::Reverse(**s))).map(|(s, _)| *s).unwrap_or_else(|| crate::landscape_paint::shape_by_name(&names[k]))).collect()
}

/// A sector re-cooked with its triangles regrouped by shape material (one stream each, ordered
/// by the material's name as the shipped sectors are), heights moved by `off` (metres, local).
fn rebuild_sector(x: &crate::nxs::Xcom, shapes: &[Vec<u8>], off: &dyn Fn(f32, f32, f32) -> f32) -> Result<Vec<u8>> {
    use crate::nxs::{ShapeMaterial, SHAPE_MATERIALS};
    let mut groups: std::collections::BTreeMap<&str, (u8, Vec<V3>, Vec<[u32; 3]>, Vec<u16>)> = Default::default();
    for (si, m) in x.meshes.iter().enumerate() {
        let verts: Vec<V3> = m.verts.iter().map(|v| [v[0], v[1] + off(v[0], v[1], v[2]), v[2]]).collect();
        for (ti, t) in m.tris.iter().enumerate() {
            let s = shapes[si][ti];
            let e = groups.entry(SHAPE_MATERIALS.get(s as usize).copied().unwrap_or("none")).or_insert((s, vec![], vec![], vec![]));
            let base = e.1.len() as u32;
            for &i in t { e.1.push(verts[i as usize]); }
            e.2.push([base, base + 1, base + 2]);
            e.3.push(m.materials.as_ref().and_then(|ms| ms.get(ti).copied()).unwrap_or(0));
        }
    }
    let mut meshes = vec![];
    let mut shape_materials = vec![];
    let mut boxes = vec![];
    for (s, verts, tris, mats) in groups.values() {
        let mesh = crate::cook::cook(verts, tris, mats)?;
        boxes.extend(crate::gr01::bbox_bytes([0, 1, 2].map(|k| mesh.aabb[k] * 100.0 - 0.01), [0, 1, 2].map(|k| mesh.aabb[3 + k] * 100.0 + 0.01)));
        meshes.push(mesh);
        shape_materials.push(x.shape_materials.iter().find(|o| o.material == *s).copied().unwrap_or(ShapeMaterial { material: *s, ignored_by_trace_ray: 0, no_collision: 0, no_response: 0 }));
    }
    let mut resource = x.resource.clone();
    let sb = resource.section.root.props.iter_mut().find(|p| p.name == "SubBoundaries").context("collision sector without SubBoundaries")?;
    let crate::gr01::Value::Raw(raw) = &mut sb.value else { anyhow::bail!("SubBoundaries is not raw") };
    let head = raw.first().copied().unwrap_or(1);
    let mut v = vec![head];
    v.extend((meshes.len() as u32).to_le_bytes());
    v.extend(boxes);
    *raw = v;
    Ok(crate::nxs::write_xcom(&crate::nxs::Xcom { resource, meshes, shape_materials }))
}

/// Blender's welded positions (and, optionally, the editor's paint: archive submesh per triangle)
/// → the patched landscape and every collision sector the edit moves or repaints.
pub fn set(g: &GameCtx, positions_bin: &Path, paint_bin: Option<&Path>) -> Result<(SetReport, Vec<(String, Vec<u8>)>)> {
    let d = std::fs::read(positions_bin).with_context(|| format!("read {}", positions_bin.display()))?;
    let n = u32::from_le_bytes(d[..4].try_into()?) as usize;
    ensure!(d.len() == 4 + n * 12, "positions file is {} bytes, expected {}", d.len(), 4 + n * 12);
    let f = |i: usize| f32::from_le_bytes(d[4 + i * 4..8 + i * 4].try_into().unwrap());
    let new_uniq: Vec<V3> = (0..n).map(|i| [f(i * 3), f(i * 3 + 1), f(i * 3 + 2)]).collect();
    let e = entry(g)?;
    let src = g.read_archive(&e)?;
    let m = crate::xmsh_geom::decode(&src)?;
    let (uniq, map) = weld(&m.positions);
    ensure!(new_uniq.len() == uniq.len(), "the landscape in Blender has {} vertices, the game's {}: move vertices, do not add or delete them", new_uniq.len(), uniq.len());
    // Blender's metres → centimetres round trip is not exact: moves under half a millimetre are noise.
    let new_file: Vec<V3> = map.iter().enumerate().map(|(i, &u)| { let (a, b) = (m.positions[i], new_uniq[u as usize]); if (0..3).all(|k| (a[k] - b[k]).abs() < 0.05) { a } else { b } }).collect();
    let (mut bytes, rep) = terrain::apply_positions(&src, &new_file)?;
    let arch_sub = crate::landscape_paint::sub_of(&m);
    let paint = match paint_bin {
        Some(p) => {
            let v = crate::landscape_paint::read_paint(p)?;
            ensure!(v.len() == arch_sub.len(), "paint has {} triangles, the landscape {}", v.len(), arch_sub.len());
            Some(v).filter(|v| *v != arch_sub)
        }
        None => None,
    };
    let mut report = SetReport { moved_vertices: rep.moved_vertices, max_offset_cm: rep.max_offset, boxes_rewritten: rep.boxes_rewritten, ..Default::default() };
    if let Some(p) = &paint {
        let (b, pr) = crate::landscape_paint::repaint(&bytes, p)?;
        bytes = b;
        report.repainted_triangles = pr.repainted_triangles;
        report.paint = Some(pr);
    }
    let mut files = vec![(g.rel(&e)?, bytes)];
    if rep.boundary_changed {
        // Higher than the island's top or past its edge: the landscape entity's boxes and its layer's
        // ContextBox grow with it, or the engine culls the new ground.
        let (lo, hi) = aabb(&new_file);
        files.push(grow_layer(g, lo, hi)?);
    }
    if rep.moved_vertices == 0 && paint.is_none() { return Ok((report, files)); }

    let tris: Vec<[u32; 3]> = m.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    let field = Field::new(m.positions.clone(), &new_file, tris);
    let secs = sectors(g)?;
    // With paint: every sector's triangles matched to the render triangles they lie on, once.
    let mut parsed: Vec<(crate::nxs::Xcom, Vec<Vec<Option<u32>>>)> = vec![];
    let mut table = vec![];
    if paint.is_some() {
        for s in &secs { let x = crate::nxs::read_xcom(&s.src)?; let mt = matched(&field, s, &x); parsed.push((x, mt)); }
        let names: Vec<String> = m.submeshes.iter().map(|s| s.material.clone()).collect();
        table = shape_table(&names, &arch_sub, &parsed.iter().collect::<Vec<_>>());
        let shape_name = |s: u8| crate::nxs::SHAPE_MATERIALS.get(s as usize).copied().unwrap_or("none").to_string();
        report.shape_materials = names.iter().zip(&table).map(|(n, &s)| (n.split('.').next().unwrap_or(n).to_string(), shape_name(s))).collect();
    }
    for (si, s) in secs.iter().enumerate() {
        let p = s.p;
        let off = |x: f32, y: f32, z: f32| field.at(p[0] + x * 100.0, p[1] + y * 100.0, p[2] + z * 100.0) / 100.0;
        if let (Some(paint), Some((x, mt))) = (&paint, parsed.get(si)) {
            let mut changed = 0;
            let shapes: Vec<Vec<u8>> = (0..x.meshes.len()).map(|k| {
                let own = x.shape_materials.get(k).map(|s| s.material).unwrap_or(0);
                mt[k].iter().map(|t| match t {
                    Some(t) if paint[*t as usize] != arch_sub[*t as usize] => { let n = table[paint[*t as usize] as usize]; if n != own { changed += 1; } n }
                    _ => own,
                }).collect()
            }).collect();
            if changed > 0 {
                ensure!(s.rot_ok, "{}: collision sector is rotated or scaled; not supported", s.name);
                files.push((g.rel(&s.entry)?, rebuild_sector(x, &shapes, &off)?));
                report.sectors.push(format!("{} ({} triangles get a new surface)", s.name, changed));
                continue;
            }
        }
        if rep.moved_vertices == 0 { continue; }
        let (cbytes, crep) = terrain_col::apply(&s.src, &off)?;
        if crep.moved_vertices == 0 { continue; }
        ensure!(s.rot_ok, "{}: collision sector is rotated or scaled; not supported", s.name);
        report.sectors.push(format!("{} ({} vertices, up to {:.0} cm)", s.name, crep.moved_vertices, crep.max_offset_m * 100.0));
        files.push((g.rel(&s.entry)?, cbytes));
    }
    Ok((report, files))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hill made through the Blender path (welded positions) patches the mesh like the in-game-
    /// proven bump, and moves the collision sectors under it by the same height.
    #[test]
    fn a_hill_through_the_blender_path() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        let tmp = std::env::temp_dir().join(format!("rc-land-{}", std::process::id()));
        let r = get(&g, &tmp, false).unwrap();
        let d = std::fs::read(&r.geometry).unwrap();
        let n = u32::from_le_bytes(d[..4].try_into().unwrap()) as usize;
        let bump = terrain::Bump { x: -18130.0, z: -14887.0, radius: 2000.0, height: 500.0, level: None };
        let mut out = (n as u32).to_le_bytes().to_vec();
        for i in 0..n {
            let p = [0, 1, 2].map(|k| f32::from_le_bytes(d[4 + (i * 3 + k) * 4..8 + (i * 3 + k) * 4].try_into().unwrap()));
            let y = p[1] + bump.offset(p[0], p[1], p[2]);
            for x in [p[0], y, p[2]] { out.extend(x.to_le_bytes()); }
        }
        let pos = tmp.join("new.bin");
        std::fs::write(&pos, out).unwrap();
        let (rep, files) = set(&g, &pos, None).unwrap();
        eprintln!("{} vertices moved (max {} cm), {} boxes, sectors {:?}", rep.moved_vertices, rep.max_offset_cm, rep.boxes_rewritten, rep.sectors);
        let src = g.read_archive(&entry(&g).unwrap()).unwrap();
        // The bump path, with the Blender path's rule that moves under 0.05 cm are rounding noise.
        let geo = crate::xmsh_geom::decode(&src).unwrap();
        let moved: Vec<V3> = geo.positions.iter().map(|p| { let dy = bump.offset(p[0], p[1], p[2]); if dy.abs() < 0.05 { *p } else { [p[0], p[1] + dy, p[2]] } }).collect();
        let (want, _) = terrain::apply_positions(&src, &moved).unwrap();
        assert!(files[0].1 == want, "Blender path differs from the bump path");
        assert!(!rep.sectors.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The game's archives hard-linked under a temp folder: a game whose loose files a test may
    /// write (never the real one). Hard links, not copies: meshes.pak alone is 130 MB.
    fn scratch_game(g: &GameCtx, tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("rc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin").join("Risen.exe"), b"stub").unwrap();
        for a in risen_formats::gamepath::discover_archives(&g.root).unwrap() {
            let to = root.join(a.path.strip_prefix(&g.root).unwrap());
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::hard_link(&a.path, &to).unwrap();
        }
        root
    }

    fn open(root: &Path) -> GameCtx { GameCtx::open(&root.join("bin").join("Risen.exe")).unwrap() }

    fn read_positions(bin: &str) -> Vec<V3> {
        let d = std::fs::read(bin).unwrap();
        let n = u32::from_le_bytes(d[..4].try_into().unwrap()) as usize;
        (0..n).map(|i| [0, 1, 2].map(|k| f32::from_le_bytes(d[4 + (i * 3 + k) * 4..8 + (i * 3 + k) * 4].try_into().unwrap()))).collect()
    }

    fn write_positions(p: &Path, v: &[V3]) {
        let mut out = (v.len() as u32).to_le_bytes().to_vec();
        for q in v { for x in q { out.extend(x.to_le_bytes()); } }
        std::fs::write(p, out).unwrap();
    }

    /// An editor reads the CURRENT ground (an earlier edit installed), changes it and sets it: the
    /// result must be what setting the same final ground on the vanilla game gives — render mesh,
    /// layer and every collision sector (patched from the archive with the total offset, never
    /// stacked on the installed ones). Getting the current ground and setting it back unchanged
    /// reproduces the installed files.
    #[test]
    fn edits_on_the_current_ground_do_not_stack() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        let root = scratch_game(&g, "stack");
        let vanilla = open(&root);
        let r = get(&vanilla, &root.join("get0"), true).unwrap();
        let base = read_positions(&r.geometry);
        let a_bump = terrain::Bump { x: -18130.0, z: -14887.0, radius: 2000.0, height: 500.0, level: None };
        let b_bump = terrain::Bump { x: -17630.0, z: -14887.0, radius: 1500.0, height: -300.0, level: None };
        let a: Vec<V3> = base.iter().map(|p| [p[0], p[1] + a_bump.offset(p[0], p[1], p[2]), p[2]]).collect();
        // B = A's hill with a dent in its side, as a second stroke would leave it.
        let b: Vec<V3> = a.iter().map(|p| [p[0], p[1] + b_bump.offset(p[0], p[1], p[2]), p[2]]).collect();
        write_positions(&root.join("a.bin"), &a);
        write_positions(&root.join("b.bin"), &b);
        let (_, files_a) = set(&vanilla, &root.join("a.bin"), None).unwrap();
        let (_, files_b_vanilla) = set(&vanilla, &root.join("b.bin"), None).unwrap();
        // "Install" A into the scratch game only.
        crate::export::write_all(&root, &files_a).unwrap();
        let modded = open(&root);
        let r = get(&modded, &root.join("get1"), true).unwrap();
        let cur = read_positions(&r.geometry);
        assert_eq!(cur.len(), a.len());
        let worst = cur.iter().zip(&a).map(|(c, w)| (0..3).map(|k| (c[k] - w[k]).abs()).fold(0.0, f32::max)).fold(0.0, f32::max);
        assert!(worst < 0.05, "current ground differs from what was set by {worst} cm");
        let (_, files_b_on_a) = set(&modded, &root.join("b.bin"), None).unwrap();
        let names = |f: &[(String, Vec<u8>)]| f.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
        assert_eq!(names(&files_b_on_a), names(&files_b_vanilla));
        for (x, y) in files_b_on_a.iter().zip(&files_b_vanilla) { assert!(x.1 == y.1, "{} differs when set on top of an installed edit", x.0); }
        write_positions(&root.join("cur.bin"), &cur);
        let (_, again) = set(&modded, &root.join("cur.bin"), None).unwrap();
        assert_eq!(names(&again), names(&files_a));
        for (x, y) in again.iter().zip(&files_a) { assert!(x.1 == y.1, "{} changed by a get-current/set round trip", x.0); }
        drop((vanilla, modded));
        let _ = std::fs::remove_dir_all(&root);
    }
}

fn aabb(p: &[V3]) -> (V3, V3) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for q in p { for k in 0..3 { lo[k] = lo[k].min(q[k]); hi[k] = hi[k].max(q[k]); } }
    (lo, hi)
}

pub const LAYER: &str = "/Levelmesh_Landscape.lrent";

/// `Levelmesh_Landscape.lrent` with the landscape entity's world box, radius and centre (both
/// copies), its local box, and the layer's ContextBox (both copies) grown to hold `lo..hi`.
/// Offsets checked on the shipped file (all equal the mesh's AABB).
fn grow_layer(g: &GameCtx, lo: V3, hi: V3) -> Result<(String, Vec<u8>)> {
    let e = g.find_one(LAYER)?;
    let mut d = g.read_archive(&e)?;
    let doc = risen_formats::lrent::LrentDoc::parse(&d).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let en = doc.entities().into_iter().find(|x| x.name == "Levelmesh_Landscape_01").context("no Levelmesh_Landscape_01 in its layer")?;
    let rd = |d: &[u8], a: usize| -> V3 { [0, 1, 2].map(|k| f32::from_le_bytes(d[a + 4 * k..a + 4 * k + 4].try_into().unwrap())) };
    let put = |d: &mut Vec<u8>, a: usize, v: &[f32]| { for (k, x) in v.iter().enumerate() { d[a + 4 * k..a + 4 * k + 4].copy_from_slice(&x.to_le_bytes()); } };
    let grow = |d: &Vec<u8>, a: usize| -> (V3, V3) { let (l, h) = (rd(d, a), rd(d, a + 12)); ([0, 1, 2].map(|k| l[k].min(lo[k])), [0, 1, 2].map(|k| h[k].max(hi[k]))) };
    let s = en.start;
    let mut p = s + 88;
    let local = loop {
        ensure!(p + 66 < en.end, "landscape entity: no local matrix");
        if d[p..p + 2] == [0xd6, 0] && f32::from_le_bytes(d[p + 62..p + 66].try_into().unwrap()) == 1.0 { break p + 2; }
        p += 1;
    };
    for block in [s + 88, local + 64] {
        let (l, h) = grow(&d, block);
        let half = [0, 1, 2].map(|k| (h[k] - l[k]) * 0.5);
        let c = [0, 1, 2].map(|k| (h[k] + l[k]) * 0.5);
        put(&mut d, block, &[l[0], l[1], l[2], h[0], h[1], h[2], (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt(), c[0], c[1], c[2]]);
    }
    for a in [s + 128, 14 + 143, 14 + 170] { let (l, h) = grow(&d, a); put(&mut d, a, &[l[0], l[1], l[2], h[0], h[1], h[2]]); }
    risen_formats::lrent::LrentDoc::parse(&d).map_err(|e| anyhow::anyhow!("grown layer does not re-read: {e:?}"))?;
    Ok((g.rel(&e)?, d))
}

#[cfg(test)]
mod paint_tests {
    use super::*;

    fn read_bin(path: &str) -> (Vec<V3>, Vec<u32>) {
        let d = std::fs::read(path).unwrap();
        let u = |a: usize| u32::from_le_bytes(d[a..a + 4].try_into().unwrap());
        let f = |a: usize| f32::from_le_bytes(d[a..a + 4].try_into().unwrap());
        let nv = u(0) as usize;
        let pos = (0..nv).map(|i| [0, 1, 2].map(|k| f(4 + (i * 3 + k) * 4))).collect();
        let at = 4 + nv * 12;
        let nt = u(at) as usize;
        let sub_at = at + 4 + nt * 12 + nt * 24;
        (pos, (0..nt).map(|t| u(sub_at + t * 4)).collect())
    }

    fn write_paint(p: &Path, s: &[u32]) {
        let mut out = (s.len() as u32).to_le_bytes().to_vec();
        for x in s { out.extend(x.to_le_bytes()); }
        std::fs::write(p, out).unwrap();
    }

    fn scratch(g: &GameCtx, tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("rc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin").join("Risen.exe"), b"stub").unwrap();
        for a in risen_formats::gamepath::discover_archives(&g.root).unwrap() {
            let to = root.join(a.path.strip_prefix(&g.root).unwrap());
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::hard_link(&a.path, &to).unwrap();
        }
        root
    }

    /// The editor paints sand into grassland near Harbour and raises a hill: the set gives the
    /// painted landscape and re-cooks the sectors whose triangles change surface; their triangles
    /// away from the paint keep their shape material, the painted ones become sand. Installed
    /// into a scratch game, `get current` returns the same heights and paint, and setting them
    /// again reproduces the installed files byte for byte; unpainting gives the plain height edit.
    #[test]
    fn paint_through_set_follows_into_collision_and_back() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        let root = scratch(&g, "paint");
        let vanilla = GameCtx::open(&root.join("bin").join("Risen.exe")).unwrap();
        let r = get(&vanilla, &root.join("get0"), false).unwrap();
        let (base, arch) = read_bin(&r.geometry);
        let m = crate::xmsh_geom::decode(&vanilla.read_archive(&entry(&vanilla).unwrap()).unwrap()).unwrap();
        let beach = m.submeshes.iter().position(|s| s.material == "Nat_Ground_Beach_01_Diffuse_01._xmat").unwrap() as u32;
        let (cx, cz, rad) = (-18130.0f32, -14887.0f32, 3000.0f32);
        let (_, map) = weld(&m.positions);
        let mut paint = arch.clone();
        for t in 0..paint.len() {
            let c = [0, 2].map(|k| (0..3).map(|j| base[map[m.indices[t * 3 + j] as usize] as usize][k]).sum::<f32>() / 3.0);
            if (c[0] - cx).hypot(c[1] - cz) < rad { paint[t] = beach; }
        }
        let bump = terrain::Bump { x: cx, z: cz, radius: 2000.0, height: 300.0, level: None };
        let raised: Vec<V3> = base.iter().map(|p| [p[0], p[1] + bump.offset(p[0], p[1], p[2]), p[2]]).collect();
        let (pos_bin, paint_bin) = (root.join("pos.bin"), root.join("paint.bin"));
        let mut out = (raised.len() as u32).to_le_bytes().to_vec();
        for q in &raised { for x in q { out.extend(x.to_le_bytes()); } }
        std::fs::write(&pos_bin, out).unwrap();
        write_paint(&paint_bin, &paint);
        let (rep, files) = set(&vanilla, &pos_bin, Some(&paint_bin)).unwrap();
        eprintln!("repainted {}, sectors {:?}, shapes {:?}", rep.repainted_triangles, rep.sectors, rep.shape_materials);
        assert!(rep.repainted_triangles > 100);
        // The shipped collision under the beaches is "debris" (not "sand"): the table follows the game.
        let beach_shape = &rep.shape_materials.iter().find(|(n, _)| n == "Nat_Ground_Beach_01_Diffuse_01").unwrap().1;
        assert_eq!(beach_shape, "debris");
        let sand = crate::nxs::SHAPE_MATERIALS.iter().position(|x| x == beach_shape).unwrap() as u8;
        // Collision: every rebuilt sector against the archive's, triangle by triangle (by centre).
        let secs = sectors(&vanilla).unwrap();
        let field = Field::new(m.positions.clone(), &m.positions, m.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect());
        let mut rebuilt = 0;
        for (rel, bytes) in &files {
            let Some(s) = secs.iter().find(|s| vanilla.rel(&s.entry).unwrap() == *rel) else { continue };
            let (old, new) = (crate::nxs::read_xcom(&s.src).unwrap(), crate::nxs::read_xcom(bytes).unwrap());
            let count = |x: &crate::nxs::Xcom| x.meshes.iter().map(|m| m.tris.len()).sum::<usize>();
            assert_eq!(count(&old), count(&new), "{}", s.name);
            if old.shape_materials == new.shape_materials && new.meshes.iter().zip(&old.meshes).all(|(a, b)| a.tris == b.tris) { continue; }
            rebuilt += 1;
            let key = |x: &crate::nxs::Xcom| -> Vec<([i32; 2], u8, f32)> {
                x.meshes.iter().enumerate().flat_map(|(k, m)| m.tris.iter().map(move |t| {
                    let c = [0, 1, 2].map(|a| (m.verts[t[0] as usize][a] + m.verts[t[1] as usize][a] + m.verts[t[2] as usize][a]) / 3.0);
                    ([(c[0] * 100.0).round() as i32, (c[2] * 100.0).round() as i32], x.shape_materials[k].material, c[1] * 100.0)
                })).collect()
            };
            let before: HashMap<[i32; 2], (u8, f32)> = key(&old).into_iter().map(|(c, s, y)| (c, (s, y))).collect();
            let (mut kept, mut sanded) = (0, 0);
            for (c, shape, _) in key(&new) {
                let Some(&(was, y_old)) = before.get(&c) else { continue };
                // Exactly: a collision triangle takes the new surface iff the render triangle it lies on was repainted.
                match field.tri_at(s.p[0] + c[0] as f32, s.p[1] + y_old, s.p[2] + c[1] as f32) {
                    Some(t) if paint[t as usize] != arch[t as usize] => { assert_eq!(shape, sand, "{}: a repainted triangle did not follow", s.name); sanded += 1; }
                    _ => { assert_eq!(shape, was, "{}: an unpainted triangle changed surface", s.name); kept += 1; }
                }
            }
            eprintln!("{}: {kept} kept, {sanded} follow the paint", s.name);
            assert!(sanded > 0 || kept > 0);
        }
        assert!(rebuilt > 0, "no collision sector follows the paint");
        // Install into the scratch game; the editor reads it back.
        crate::export::write_all(&root, &files).unwrap();
        let modded = GameCtx::open(&root.join("bin").join("Risen.exe")).unwrap();
        let r = get(&modded, &root.join("get1"), true).unwrap();
        let (cur, cur_paint) = read_bin(&r.geometry);
        assert!(cur_paint == paint, "paint read back differs");
        let worst = cur.iter().zip(&raised).map(|(c, w)| (0..3).map(|k| (c[k] - w[k]).abs()).fold(0.0, f32::max)).fold(0.0, f32::max);
        assert!(worst < 0.05, "heights read back differ by {worst} cm");
        let (_, again) = set(&modded, &pos_bin, Some(&paint_bin)).unwrap();
        assert_eq!(again.len(), files.len());
        for (x, y) in again.iter().zip(&files) { assert!(x.0 == y.0 && x.1 == y.1, "{} differs when set again on top of itself", x.0); }
        // Unpainting returns to the plain height edit.
        write_paint(&paint_bin, &arch);
        let (rep0, plain) = set(&modded, &pos_bin, Some(&paint_bin)).unwrap();
        let (_, heights_only) = set(&modded, &pos_bin, None).unwrap();
        assert_eq!(rep0.repainted_triangles, 0);
        assert!(plain.len() == heights_only.len() && plain.iter().zip(&heights_only).all(|(x, y)| x == y));
        drop((vanilla, modded));
        let _ = std::fs::remove_dir_all(&root);
    }
}
