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

/// The archive's landscape (never a loose copy: edits must not stack) for Blender, with textures.
pub fn get(g: &GameCtx, out_dir: &Path) -> Result<GetReport> {
    let src = g.read_archive(&entry(g)?)?;
    let m = crate::xmsh_geom::decode(&src)?;
    let (uniq, map) = weld(&m.positions);
    let tex = crate::mesh::TextureIndex::build(g);
    let mut warnings = vec![];
    let materials = m.submeshes.iter().map(|s| crate::mesh::material(g, &tex, &s.material, out_dir, &mut warnings)).collect::<Result<Vec<_>>>()?;
    let mut sub_of = vec![0u32; m.indices.len() / 3];
    for (k, s) in m.submeshes.iter().enumerate() { for t in s.first_index / 3..(s.first_index + s.index_count) / 3 { sub_of[t as usize] = k as u32; } }
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
pub struct SetReport { pub moved_vertices: usize, pub max_offset_cm: f32, pub boxes_rewritten: usize, pub sectors: Vec<String> }

/// Blender's welded positions → the patched landscape and every collision sector it moves.
pub fn set(g: &GameCtx, positions_bin: &Path) -> Result<(SetReport, Vec<(String, Vec<u8>)>)> {
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
    let (bytes, rep) = terrain::apply_positions(&src, &new_file)?;
    let mut report = SetReport { moved_vertices: rep.moved_vertices, max_offset_cm: rep.max_offset, boxes_rewritten: rep.boxes_rewritten, sectors: vec![] };
    let mut files = vec![(g.rel(&e)?, bytes)];
    if rep.boundary_changed {
        // Higher than the island's top or past its edge: the landscape entity's boxes and its layer's
        // ContextBox grow with it, or the engine culls the new ground.
        let (lo, hi) = aabb(&new_file);
        files.push(grow_layer(g, lo, hi)?);
    }
    if rep.moved_vertices == 0 { return Ok((report, files)); }

    let tris: Vec<[u32; 3]> = m.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    let field = Field::new(m.positions.clone(), &new_file, tris);
    let (cd, _) = g.read(terrain_col::COL_LRENT)?;
    let doc = risen_formats::lrent::LrentDoc::parse(&cd).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    for en in doc.entities() {
        let mx = &en.matrix;
        let p = [mx[12], mx[13], mx[14]];
        let Ok(ce) = g.find_one(&format!("/{}_COL._xcom", en.name)) else { continue };
        let csrc = g.read_archive(&ce)?;
        let off = |x: f32, y: f32, z: f32| field.at(p[0] + x * 100.0, p[1] + y * 100.0, p[2] + z * 100.0) / 100.0;
        let (cbytes, crep) = terrain_col::apply(&csrc, &off)?;
        if crep.moved_vertices == 0 { continue; }
        let rot_ok = (mx[0] - 1.0).abs() < 1e-5 && (mx[5] - 1.0).abs() < 1e-5 && (mx[10] - 1.0).abs() < 1e-5 && [mx[1], mx[2], mx[4], mx[6], mx[8], mx[9]].iter().all(|x| x.abs() < 1e-5);
        ensure!(rot_ok, "{}: collision sector is rotated or scaled; not supported", en.name);
        report.sectors.push(format!("{} ({} vertices, up to {:.0} cm)", en.name, crep.moved_vertices, crep.max_offset_m * 100.0));
        files.push((g.rel(&ce)?, cbytes));
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
        let r = get(&g, &tmp).unwrap();
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
        let (rep, files) = set(&g, &pos).unwrap();
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
