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
pub struct GetReport {
    pub vertices: usize, pub triangles: usize, pub materials: Vec<crate::mesh::Material>, pub geometry: String,
    /// `topology.bin` next to the geometry when the installed ground has a topology edit
    /// (`landscape_topo`): its vertex and triangle counts then differ from the archive's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topology: Option<String>,
}

/// Where the installed topology edit lives (next to the landscape, relative to the game folder).
pub fn sidecar(g: &GameCtx, e: &str) -> Result<String> {
    let rel = g.rel(e)?;
    Ok(format!("{}.topology", rel.strip_suffix("._xmsh").unwrap_or(&rel)))
}

/// The topology edit installed with the current landscape, if any.
pub fn installed_topology(g: &GameCtx, e: &str) -> Result<Option<crate::landscape_topo::Spec>> {
    let p = g.root.join(sidecar(g, e)?);
    if !p.is_file() { return Ok(None); }
    Ok(Some(crate::landscape_topo::read(&std::fs::read(&p)?).with_context(|| format!("read {}", p.display()))?))
}

/// `get current` when the installed ground carries a topology edit: the archive with the edit
/// replayed is the reference the installed file is read against.
fn get_topology(g: &GameCtx, e: &str, src: &[u8], spec: &crate::landscape_topo::Spec, out_dir: &Path) -> Result<GetReport> {
    let base = crate::landscape_topo::apply(src, spec)?;
    let (cur, _) = g.read(e)?;
    let (pos, sub) = crate::landscape_paint::recover(&base.bytes, &cur).context("read the installed landscape")?;
    let mut uniq = base.welded.clone();
    let mut done = vec![false; uniq.len()];
    for (i, &u) in base.map.iter().enumerate() { if !done[u as usize] { uniq[u as usize] = pos[i]; done[u as usize] = true; } }
    let mut wsub = base.welded_sub.clone();
    for (ft, &wt) in base.tri_welded.iter().enumerate() { wsub[wt as usize] = sub[ft]; }
    let m = crate::xmsh_geom::decode(src)?;
    let tex = crate::mesh::TextureIndex::build(g);
    let mut warnings = vec![];
    let materials = m.submeshes.iter().map(|s| crate::mesh::material(g, &tex, &s.material, out_dir, &mut warnings)).collect::<Result<Vec<_>>>()?;
    let mut b = (uniq.len() as u32).to_le_bytes().to_vec();
    for p in &uniq { for x in p { b.extend(x.to_le_bytes()); } }
    b.extend((base.welded_tris.len() as u32).to_le_bytes());
    for t in &base.welded_tris { for i in t { b.extend(i.to_le_bytes()); } }
    for t in &base.welded_uv { for c in t { b.extend(c[0].to_le_bytes()); b.extend(c[1].to_le_bytes()); } }
    for s in &wsub { b.extend(s.to_le_bytes()); }
    std::fs::create_dir_all(out_dir)?;
    let path = out_dir.join("landscape.bin");
    std::fs::write(&path, b)?;
    let tp = out_dir.join("topology.bin");
    std::fs::write(&tp, crate::landscape_topo::write(spec))?;
    Ok(GetReport { vertices: uniq.len(), triangles: base.welded_tris.len(), materials, geometry: path.to_string_lossy().into_owned(), topology: Some(tp.to_string_lossy().into_owned()) })
}

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
    if current {
        if let Some(spec) = installed_topology(g, &e)? { return get_topology(g, &e, &src, &spec, out_dir); }
        // A stale topology file from another tool would be ignored here; the archive's ground is read.
    }
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
    Ok(GetReport { vertices: uniq.len(), triangles: m.indices.len() / 3, materials, geometry: path.to_string_lossy().into_owned(), topology: None })
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
    /// Sectors whose collision was rebuilt from the edited ground where the edit bends it.
    pub dense_sectors: usize,
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
    crate::landscape_col::rebuild(x, shapes, off, None, &[])
}

/// Blender's welded positions (and, optionally, the editor's paint: archive submesh per triangle)
/// → the patched landscape and every collision sector the edit moves or repaints. Where an edit
/// bends the ground inside a coarse collision triangle, the collision there is rebuilt from the
/// edited render triangles (`landscape_col`).
pub fn set(g: &GameCtx, positions_bin: &Path, paint_bin: Option<&Path>) -> Result<(SetReport, Vec<(String, Vec<u8>)>)> {
    set_with(g, positions_bin, paint_bin, true)
}

/// `set`, with the dense collision rebuild on or off (off = only the coarse vertices move; for
/// comparisons).
pub fn set_with(g: &GameCtx, positions_bin: &Path, paint_bin: Option<&Path>, dense: bool) -> Result<(SetReport, Vec<(String, Vec<u8>)>)> {
    set_full(g, positions_bin, paint_bin, None, dense)
}

/// The base an edit of `n` welded vertices applies to: the given topology, else the archive, else
/// (when the counts say so) the topology installed with the current ground.
fn base_for(g: &GameCtx, e: &str, src: &[u8], n: usize, topology: Option<&crate::landscape_topo::Spec>) -> Result<crate::landscape_topo::Base> {
    if let Some(t) = topology { return crate::landscape_topo::apply(src, t); }
    let b = crate::landscape_topo::archive_base(src)?;
    if n != b.welded.len() {
        if let Some(t) = installed_topology(g, e)? { if t.result_vertices as usize == n { return crate::landscape_topo::apply(src, &t); } }
    }
    Ok(b)
}

fn read_positions_bin(positions_bin: &Path) -> Result<Vec<V3>> {
    let d = std::fs::read(positions_bin).with_context(|| format!("read {}", positions_bin.display()))?;
    ensure!(d.len() >= 4, "positions file is empty");
    let n = u32::from_le_bytes(d[..4].try_into()?) as usize;
    ensure!(d.len() == 4 + n * 12, "positions file is {} bytes, expected {}", d.len(), 4 + n * 12);
    let f = |i: usize| f32::from_le_bytes(d[4 + i * 4..8 + i * 4].try_into().unwrap());
    Ok((0..n).map(|i| [f(i * 3), f(i * 3 + 1), f(i * 3 + 2)]).collect())
}

/// `set` with an explicit topology edit (`landscape_topo`; None: the archive's topology, or the
/// installed one when the vertex count matches it). The topology file goes into the output next to
/// the landscape, so the ground can be read back.
pub fn set_full(g: &GameCtx, positions_bin: &Path, paint_bin: Option<&Path>, topology: Option<&crate::landscape_topo::Spec>, dense: bool) -> Result<(SetReport, Vec<(String, Vec<u8>)>)> {
    let new_uniq = read_positions_bin(positions_bin)?;
    let e = entry(g)?;
    let src = g.read_archive(&e)?;
    let base = base_for(g, &e, &src, new_uniq.len(), topology)?;
    let src = &base.bytes;
    let m = crate::xmsh_geom::decode(src)?;
    let map = &base.map;
    ensure!(new_uniq.len() == base.welded.len(), "the landscape in Blender has {} vertices, the game's {}: move vertices, do not add or delete them", new_uniq.len(), base.welded.len());
    // Blender's metres → centimetres round trip is not exact: moves under half a millimetre are noise.
    let new_file: Vec<V3> = map.iter().enumerate().map(|(i, &u)| { let (a, b) = (m.positions[i], new_uniq[u as usize]); if (0..3).all(|k| (a[k] - b[k]).abs() < 0.05) { a } else { b } }).collect();
    let (mut bytes, rep) = terrain::apply_positions(src, &new_file)?;
    let arch_sub = crate::landscape_paint::sub_of(&m);
    let paint = match paint_bin {
        Some(p) => {
            let v = crate::landscape_paint::read_paint(p)?;
            ensure!(v.len() == base.welded_tris.len(), "paint has {} triangles, the landscape {}", v.len(), base.welded_tris.len());
            // Welded (editor) order -> this file's triangle order (the same for the archive).
            let v: Vec<u32> = base.tri_welded.iter().map(|&w| v[w as usize]).collect();
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
    if let Some(spec) = &base.spec { files.push((sidecar(g, &e)?, crate::landscape_topo::write(spec))); }
    if rep.boundary_changed {
        // Higher than the island's top or past its edge: the landscape entity's boxes and its layer's
        // ContextBox grow with it, or the engine culls the new ground.
        let (lo, hi) = aabb(&new_file);
        files.push(grow_layer(g, lo, hi)?);
    }
    if rep.moved_vertices == 0 && paint.is_none() && base.holes.is_empty() { return Ok((report, files)); }

    let tris: Vec<[u32; 3]> = m.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    let densify = if dense && (rep.moved_vertices > 0 || !base.holes.is_empty()) { Some(crate::landscape_col::Dense::new(&m.positions, &new_file, &tris, base.holes.clone())) } else { None };
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
        if let Some(dense) = &densify {
            let own;
            let (x, mt) = match parsed.get(si) { Some((x, mt)) => (x, Some(mt)), None => { own = crate::nxs::read_xcom(&s.src)?; (&own, None) } };
            if let Some(plan) = dense.plan(x, p, &|wx, wy, wz| field.at(wx, wy, wz)) {
                ensure!(s.rot_ok, "{}: collision sector is rotated or scaled; not supported", s.name);
                // Surfaces: the kept triangles as with paint (or their own), the added ones the
                // repainted material's, else the surface of the collision triangle they replace.
                let shapes: Vec<Vec<u8>> = (0..x.meshes.len()).map(|k| {
                    let own = x.shape_materials.get(k).map(|s| s.material).unwrap_or(0);
                    (0..x.meshes[k].tris.len()).map(|i| match (&paint, mt.and_then(|mt| mt[k][i])) {
                        (Some(paint), Some(t)) if paint[t as usize] != arch_sub[t as usize] => table[paint[t as usize] as usize],
                        _ => own,
                    }).collect()
                }).collect();
                let up = crate::landscape_col::up_winding(x);
                let extra: Vec<([V3; 3], u8)> = plan.add.iter().zip(&plan.add_from).map(|(&t, &(k, i))| {
                    let c = dense.tris[t as usize].map(|v| { let q = new_file[v as usize]; [(q[0] - p[0]) / 100.0, (q[1] - p[1]) / 100.0, (q[2] - p[2]) / 100.0] });
                    // The render file faces up as (c-a)x(b-a); match the sector's own winding.
                    let tri = if up > 0.0 { [c[0], c[2], c[1]] } else { c };
                    let shape = match &paint { Some(paint) if paint[t as usize] != arch_sub[t as usize] => table[paint[t as usize] as usize], _ => shapes[k][i] };
                    (tri, shape)
                }).collect();
                files.push((g.rel(&s.entry)?, crate::landscape_col::rebuild(x, &shapes, &off, Some(&plan.remove), &extra)?));
                report.sectors.push(format!("{} (dense: {} coarse triangles rebuilt from {} ground triangles, coarse patch was off by up to {:.0} cm)", s.name, plan.removed, plan.add.len(), plan.worst_cm));
                report.dense_sectors += 1;
                continue;
            }
        }
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

/// How well the collision follows the ground over an edit.
#[derive(serde::Serialize, Debug, Default)]
pub struct GapReport {
    /// Points checked: 4 per render triangle (not steeper than 60°) within 20 m of a vertex moved by more than 1 cm.
    pub samples: usize,
    /// Points with no collision triangle under them (e.g. under the town's shell).
    pub without_collision: usize,
    /// Edited triangles steeper than 60° (walls), not measured vertically.
    pub steep_skipped: usize,
    /// Largest and 99th-percentile |collision - ground| over the edited area (cm), after the set.
    pub max_cm: f32,
    pub p99_cm: f32,
    /// The same points on the unedited game: how far the shipped collision already was from the shipped ground.
    pub vanilla_max_cm: f32,
    /// Largest |gap after - gap in the shipped game| (cm): what the edit itself adds.
    pub added_max_cm: f32,
    /// Points where the shipped collision already followed the shipped ground (within 20 cm), and the
    /// largest gap after the set there: the walkable ground the edit must not break.
    pub followed_samples: usize,
    pub followed_max_cm: f32,
    /// Every collision triangle of the edited area, walls included: the 3D distance of its points to
    /// the nearest ground triangle (cm), after the set and in the shipped game, and the points farther
    /// than 20 cm from any ground (`apart`; in the shipped game too: `vanilla_apart`).
    pub dist_max_cm: f32,
    pub dist_vanilla_max_cm: f32,
    pub apart: usize,
    pub vanilla_apart: usize,
    pub dist_worst_at: [f32; 3],
    /// Where the largest gap is (x, z cm).
    pub worst_at: [f32; 2],
    /// The worst points: x, z, ground before, collision before, ground after, collision after (cm).
    pub worst: Vec<[f32; 6]>,
}

/// The gap between the collision and the render ground over the area `positions_bin` edits, with
/// the collision sectors of `files` (the output of `set`) in place of the shipped ones.
pub fn gap_report(g: &GameCtx, positions_bin: &Path, files: &[(String, Vec<u8>)]) -> Result<GapReport> { gap_report_full(g, positions_bin, None, files) }

/// `gap_report` for an edit with a topology (see `set_full`).
pub fn gap_report_full(g: &GameCtx, positions_bin: &Path, topology: Option<&crate::landscape_topo::Spec>, files: &[(String, Vec<u8>)]) -> Result<GapReport> {
    let new_uniq = read_positions_bin(positions_bin)?;
    let e = entry(g)?;
    let base = base_for(g, &e, &g.read_archive(&e)?, new_uniq.len(), topology)?;
    let m = crate::xmsh_geom::decode(&base.bytes)?;
    let map = &base.map;
    ensure!(new_uniq.len() == base.welded.len(), "positions do not match the landscape");
    let new_file: Vec<V3> = map.iter().map(|&u| new_uniq[u as usize]).collect();
    let secs = sectors(g)?;
    let (mut now, mut was) = (vec![], vec![]);
    for s in &secs {
        let x0 = crate::nxs::read_xcom(&s.src)?;
        crate::landscape_col::Surface::from_xcom(&x0, s.p, &mut was);
        let rel = g.rel(&s.entry)?;
        match files.iter().find(|(r, _)| *r == rel) {
            Some((_, b)) => crate::landscape_col::Surface::from_xcom(&crate::nxs::read_xcom(b)?, s.p, &mut now),
            None => crate::landscape_col::Surface::from_xcom(&x0, s.p, &mut now),
        }
    }
    let (now, was) = (crate::landscape_col::Surface::new(now), crate::landscape_col::Surface::new(was));
    let mut rep = GapReport::default();
    let mut gaps = vec![];
    // The edited area: 10 m cells holding a vertex moved by more than 1 cm, and two cells around them
    // (collision triangles reach past the moved ground).
    let key = |x: f32, z: f32| ((x / 1000.0).floor() as i32, (z / 1000.0).floor() as i32);
    let mut area = std::collections::HashSet::new();
    for (a, b) in m.positions.iter().zip(&new_file) {
        if (a[1] - b[1]).abs() > 1.0 { let (cx, cz) = key(a[0], a[2]); for dx in -2..=2 { for dz in -2..=2 { area.insert((cx + dx, cz + dz)); } } }
    }
    for t in m.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]) {
        let (a, b) = (t.map(|i| m.positions[i as usize]), t.map(|i| new_file[i as usize]));
        if !area.contains(&key((b[0][0] + b[1][0] + b[2][0]) / 3.0, (b[0][2] + b[1][2] + b[2][2]) / 3.0)) { continue; }
        // Walls and steep faces: a vertical gap means nothing there (walking happens on the rest).
        let (u, v) = ([b[1][0] - b[0][0], b[1][1] - b[0][1], b[1][2] - b[0][2]], [b[2][0] - b[0][0], b[2][1] - b[0][1], b[2][2] - b[0][2]]);
        let nrm = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        if nrm[1].abs() < 0.5 * (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt() { rep.steep_skipped += 1; continue; }
        for w in [[1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0], [0.7, 0.15, 0.15], [0.15, 0.7, 0.15], [0.15, 0.15, 0.7f32]] {
            let at = |p: &[V3; 3]| -> V3 { [0, 1, 2].map(|c| w[0] * p[0][c] + w[1] * p[1][c] + w[2] * p[2][c]) };
            let (pa, pb) = (at(&a), at(&b));
            rep.samples += 1;
            let (Some(cn), Some(cw)) = (now.height_near(pb[0], pb[2], pb[1]), was.height_near(pa[0], pa[2], pa[1])) else { rep.without_collision += 1; continue };
            let (gap, gap0) = ((cn - pb[1]).abs(), (cw - pa[1]).abs());
            if gap > rep.max_cm { rep.max_cm = gap; rep.worst_at = [pb[0], pb[2]]; }
            rep.vanilla_max_cm = rep.vanilla_max_cm.max(gap0);
            if gap0 <= 20.0 { rep.followed_samples += 1; rep.followed_max_cm = rep.followed_max_cm.max(gap); }
            rep.added_max_cm = rep.added_max_cm.max(((cn - pb[1]) - (cw - pa[1])).abs());
            gaps.push(gap);
            if gap0 <= 20.0 { rep.worst.push([pb[0], pb[2], pa[1], cw, pb[1], cn]); }
        }
    }
    let in_area = |x: f32, z: f32| area.contains(&key(x, z));
    let tri_list: Vec<[u32; 3]> = m.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    let ground_now = crate::landscape_col::Dense::new(&new_file, &new_file, &tri_list, vec![]);
    let ground_was = crate::landscape_col::Dense::new(&m.positions, &m.positions, &tri_list, vec![]);
    // Collision -> ground ("walking in the air"): every collision point near some ground triangle.
    for (surf, ground, vanilla) in [(&now, &ground_now, false), (&was, &ground_was, true)] {
        for q in surf.samples(50.0, &in_area, false) {
            let d = ground.distance(q, 300.0).unwrap_or(f32::MAX);
            if vanilla { if d > 20.0 { rep.vanilla_apart += 1; } if d < f32::MAX { rep.dist_vanilla_max_cm = rep.dist_vanilla_max_cm.max(d); } continue; }
            if d > 20.0 { rep.apart += 1; }
            if d < f32::MAX && d > rep.dist_max_cm { rep.dist_max_cm = d; rep.dist_worst_at = q; }
        }
    }
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    rep.worst.sort_by(|a, b| (b[5] - b[4]).abs().partial_cmp(&(a[5] - a[4]).abs()).unwrap());
    rep.worst.truncate(12);
    rep.p99_cm = gaps.get((gaps.len() as f32 * 0.99) as usize).copied().unwrap_or(0.0);
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ground the game has installed now (an edit the owner walks on), set again: with the dense
    /// collision the collision follows the ground over the whole edited area to within 20 cm; the
    /// coarse patch alone does not. Nothing is written into the game.
    #[test]
    fn installed_edit_collision_follows_the_ground() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        let tmp = std::env::temp_dir().join(format!("rc-gap-{}", std::process::id()));
        let r = get(&g, &tmp, true).unwrap();
        let d = std::fs::read(&r.geometry).unwrap();
        let n = u32::from_le_bytes(d[..4].try_into().unwrap()) as usize;
        let pos = tmp.join("pos.bin");
        std::fs::write(&pos, &d[..4 + n * 12]).unwrap();
        let (rc, coarse) = set_with(&g, &pos, None, false).unwrap();
        let (rd, dense) = set_with(&g, &pos, None, true).unwrap();
        if rc.moved_vertices == 0 { eprintln!("skip: no ground edit installed"); return; }
        let (gc, gd) = (gap_report(&g, &pos, &coarse).unwrap(), gap_report(&g, &pos, &dense).unwrap());
        eprintln!("coarse: {gc:?}
dense:  {gd:?}
dense sectors {} {:?}", rd.dense_sectors, rd.sectors);
        assert!(gd.samples > 0);
        assert!(gd.max_cm < 20.0, "collision is {} cm off the ground at {:?}", gd.max_cm, gd.worst_at);
        assert!(gd.dist_max_cm < 20.0 && gd.apart == 0, "collision floats {} cm off the ground at {:?} ({} points)", gd.dist_max_cm, gd.dist_worst_at, gd.apart);
        // The coarse patch alone leaves collision in the air on such edits (players run on nothing).
        eprintln!("coarse patch: {} collision points over 20 cm from the ground, up to {} cm", gc.apart, gc.dist_max_cm);
        // The render mesh is the same either way.
        assert!(coarse[0].1 == dense[0].1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Topology: a patch near Harbour subdivided, a hole cut in its middle and the ground there
    /// raised. Set on a scratch game, the landscape has the new counts and the topology file goes
    /// with it; read back (`get current`) the heights, paint and topology come out as set; a plain
    /// header-less set of those positions (what the Blender add-on sends) finds the installed
    /// topology and gives the same files; the collision has nothing over the hole and follows the
    /// raised ground. Writes a fixture for the editor's own replay (RC_TOPO_FIXTURE).
    #[test]
    fn subdivided_ground_with_a_hole_round_trips() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        let root = scratch_game(&g, "topo");
        let vanilla = open(&root);
        let e = entry(&vanilla).unwrap();
        let src = vanilla.read_archive(&e).unwrap();
        let arch = crate::landscape_topo::archive_base(&src).unwrap();
        let (cx, cz) = (-18130.0f32, -14887.0f32);
        let centre = |b: &crate::landscape_topo::Base, t: usize| { let p = b.welded_tris[t].map(|i| b.welded[i as usize]); [(p[0][0] + p[1][0] + p[2][0]) / 3.0, (p[0][2] + p[1][2] + p[2][2]) / 3.0] };
        let near = |b: &crate::landscape_topo::Base, r: f32| (0..b.welded_tris.len()).filter(|&t| { let c = centre(b, t); (c[0] - cx).hypot(c[1] - cz) < r }).map(|t| t as u32).collect::<Vec<_>>();
        let sub1 = near(&arch, 1500.0);
        assert!(sub1.len() > 10, "{}", sub1.len());
        let op1 = crate::landscape_topo::Op { kind: crate::landscape_topo::KIND_SUBDIVIDE, tris: sub1 };
        let mid = crate::landscape_topo::apply(&src, &crate::landscape_topo::seal(&src, vec![op1.clone()]).unwrap()).unwrap();
        let hole = near(&mid, 250.0);
        assert!(!hole.is_empty());
        let spec = crate::landscape_topo::seal(&src, vec![op1, crate::landscape_topo::Op { kind: crate::landscape_topo::KIND_REMOVE, tris: hole.clone() }]).unwrap();
        let base = crate::landscape_topo::apply(&src, &spec).unwrap();
        assert!(base.welded.len() > arch.welded.len() && base.welded_tris.len() != arch.welded_tris.len());
        // Fixture for the editor's C# replay (same numbering, same checksum).
        if let Some(dir) = std::env::var_os("RC_TOPO_FIXTURE") {
            let dir = Path::new(&dir);
            get(&vanilla, dir, false).unwrap();
            std::fs::write(dir.join("topology.bin"), crate::landscape_topo::write(&spec)).unwrap();
        }
        // Raise the ground around the hole by 2 m (a mound with a cellar mouth).
        let raised: Vec<V3> = base.welded.iter().map(|p| { let d = (p[0] - cx).hypot(p[2] - cz); if d < 1000.0 { [p[0], p[1] + 200.0 * (1.0 - d / 1000.0), p[2]] } else { *p } }).collect();
        let (pos_bin, paint_bin) = (root.join("pos.bin"), root.join("paint.bin"));
        write_positions(&pos_bin, &raised);
        let mut pb = (base.welded_sub.len() as u32).to_le_bytes().to_vec();
        for s in &base.welded_sub { pb.extend(s.to_le_bytes()); }
        std::fs::write(&paint_bin, pb).unwrap();
        let (rep, files) = set_full(&vanilla, &pos_bin, Some(&paint_bin), Some(&spec), true).unwrap();
        eprintln!("topology: {} -> {} vertices, {} -> {} triangles; sectors {:?}", arch.welded.len(), base.welded.len(), arch.welded_tris.len(), base.welded_tris.len(), rep.sectors);
        let side = sidecar(&vanilla, &e).unwrap();
        assert!(files.iter().any(|f| f.0 == side));
        let geo = crate::xmsh_geom::decode(&files[0].1).unwrap();
        assert_eq!(geo.indices.len() / 3, base.welded_tris.len());
        assert!(crate::gr01::Resource::parse(&files[0].1).unwrap().write() == files[0].1);
        // Install into the scratch game and read back.
        crate::export::write_all(&root, &files).unwrap();
        let modded = open(&root);
        let out = root.join("get1");
        let r = get(&modded, &out, true).unwrap();
        assert_eq!(r.vertices, base.welded.len());
        assert_eq!(r.triangles, base.welded_tris.len());
        assert!(r.topology.is_some());
        let back = read_positions(&r.geometry);
        // Vertices only the removed triangles used are in no file any more: they read back as the base had them.
        let mut used = vec![false; base.welded.len()];
        for t in &base.welded_tris { for &i in t { used[i as usize] = true; } }
        let worst = back.iter().zip(&raised).zip(&used).filter(|(_, &u)| u).map(|((c, w), _)| (0..3).map(|k| (c[k] - w[k]).abs()).fold(0.0, f32::max)).fold(0.0, f32::max);
        assert!(worst < 0.05, "heights read back differ by {worst} cm");
        assert_eq!(crate::landscape_topo::read(&std::fs::read(r.topology.as_ref().unwrap()).unwrap()).unwrap(), spec);
        // What the Blender add-on does: header-less positions of the new count, no topology given.
        let bin2 = root.join("pos2.bin");
        write_positions(&bin2, &back);
        let (_, again) = set_full(&modded, &bin2, Some(&paint_bin), None, true).unwrap();
        assert_eq!(again.len(), files.len());
        for (x, y) in again.iter().zip(&files) { assert!(x.0 == y.0 && x.1 == y.1, "{} differs when set again without the topology", x.0); }
        // No collision over the hole; the raised ground is followed.
        let secs = sectors(&modded).unwrap();
        let mut tris = vec![];
        for s in &secs {
            let rel = modded.rel(&s.entry).unwrap();
            let b = files.iter().find(|f| f.0 == rel).map(|f| f.1.clone()).unwrap_or(s.src.clone());
            crate::landscape_col::Surface::from_xcom(&crate::nxs::read_xcom(&b).unwrap(), s.p, &mut tris);
        }
        let surf = crate::landscape_col::Surface::new(tris);
        for &t in &hole {
            let p = mid.welded_tris[t as usize].map(|i| mid.welded[i as usize]);
            let c = [0, 1, 2].map(|k| (p[0][k] + p[1][k] + p[2][k]) / 3.0);
            let h = surf.height_near(c[0], c[2], c[1]);
            assert!(h.map_or(true, |h| (h - c[1]).abs() > 300.0), "collision left over the hole at {c:?}: {h:?}");
        }
        let gap = gap_report_full(&modded, &bin2, None, &files).unwrap();
        eprintln!("gap {gap:?}");
        assert!(gap.dist_max_cm < 20.0 && gap.apart == 0, "{gap:?}");
        drop((vanilla, modded));
        let _ = std::fs::remove_dir_all(&root);
    }

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
        // The raised ground, for collision triangles that are ground triangles now (dense rebuild).
        let raised_file: Vec<V3> = map.iter().map(|&u| raised[u as usize]).collect();
        let field_new = Field::new(raised_file.clone(), &raised_file, m.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect());
        let mut rebuilt = 0;
        for (rel, bytes) in &files {
            let Some(s) = secs.iter().find(|s| vanilla.rel(&s.entry).unwrap() == *rel) else { continue };
            let (old, new) = (crate::nxs::read_xcom(&s.src).unwrap(), crate::nxs::read_xcom(bytes).unwrap());
            let count = |x: &crate::nxs::Xcom| x.meshes.iter().map(|m| m.tris.len()).sum::<usize>();
            let dense = rep.sectors.iter().any(|x| x.starts_with(&format!("{} (dense", s.name)));
            if !dense { assert_eq!(count(&old), count(&new), "{}", s.name); }
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
            for (c, shape, y_new) in key(&new) {
                let Some(&(was, y_old)) = before.get(&c) else {
                    // A ground triangle brought in by the dense rebuild: the surface of its own paint.
                    assert!(dense, "{}: a collision triangle appeared in a sector that was not rebuilt", s.name);
                    if let Some(t) = field_new.tri_at(s.p[0] + c[0] as f32, s.p[1] + y_new, s.p[2] + c[1] as f32) {
                        if paint[t as usize] != arch[t as usize] { assert_eq!(shape, sand, "{}: an added repainted triangle", s.name); sanded += 1; }
                    }
                    continue
                };
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

