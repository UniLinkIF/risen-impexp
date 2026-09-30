//! Terrain height editing for the world-wide landscape mesh (`Levelmesh_Landscape_01._xmsh`).
//!
//! The landscape is ONE static mesh
//! (376 034 vertices, 250 885 triangles, 28 submeshes, one per ground material). Raising or lowering
//! the ground keeps its topology: only vertex positions move, so the vertex and index buffers keep
//! their size and every byte outside the touched vertices stays as shipped. What has to follow the
//! moved vertices, besides the positions themselves:
//!
//! - the vertex normals (and tangents, which the shader uses for the normal map),
//! - every box the engine culls with: each submesh's `Extends`, its culling nodes (`u16 201, u32 N,
//!   N x (bCBox, u32, u32)` right after `VertexCount`), and the mesh-wide `Boundary`.
//!
//! The file is patched in place: no field changes size, so no offset or size in the header moves.

use anyhow::{bail, ensure, Result};

/// A `bCBox` inside the file: min xyz, max xyz (f32), and where its 24 bytes sit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FileBox { pub at: usize, pub min: [f32; 3], pub max: [f32; 3] }

/// One culling node of a submesh: a box and two u32 whose meaning the probe establishes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CullNode { pub bbox: FileBox, pub a: u32, pub b: u32 }

/// The boxes of one submesh, plus the ranges they describe.
#[derive(Debug, Clone, PartialEq)]
pub struct SubmeshBoxes { pub material: String, pub extends: FileBox, pub first_index: u32, pub index_count: u32, pub first_vertex: u32, pub vertex_count: u32, pub nodes: Vec<CullNode> }

/// Every box of a mesh's property section.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshBoxes { pub submeshes: Vec<SubmeshBoxes>, pub boundary: FileBox }

fn u16_at(d: &[u8], at: usize) -> Result<u16> { d.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or_else(|| anyhow::anyhow!("u16 runs past end: offset 0x{at:x}")) }
fn u32_at(d: &[u8], at: usize) -> Result<u32> { d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| anyhow::anyhow!("u32 runs past end: offset 0x{at:x}")) }
fn f32_at(d: &[u8], at: usize) -> f32 { f32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]]) }

fn read_box(d: &[u8], at: usize) -> Result<FileBox> {
    ensure!(d.len() >= at + 24, "bCBox runs past end: offset 0x{at:x}");
    Ok(FileBox { at, min: [f32_at(d, at), f32_at(d, at + 4), f32_at(d, at + 8)], max: [f32_at(d, at + 12), f32_at(d, at + 16), f32_at(d, at + 20)] })
}

/// Finds `name` as a length-prefixed property name at or after `from`, followed by `type_name`, and
/// returns the offset of its value (after `u16 30, u32 size`). The property framing is the same as in
/// `.tple`: `[u16 len][name][u16 len][type][u16 0x1E][u32 value size][value]`.
fn find_property(d: &[u8], from: usize, end: usize, name: &str, type_name: &str) -> Option<(usize, usize)> {
    let mut pat = (name.len() as u16).to_le_bytes().to_vec();
    pat.extend_from_slice(name.as_bytes());
    pat.extend_from_slice(&(type_name.len() as u16).to_le_bytes());
    pat.extend_from_slice(type_name.as_bytes());
    pat.extend_from_slice(&[0x1e, 0]);
    let hay = d.get(from..end)?;
    let k = hay.windows(pat.len()).position(|w| w == pat.as_slice())?;
    let size_at = from + k + pat.len();
    let size = u32::from_le_bytes(d.get(size_at..size_at + 4)?.try_into().ok()?) as usize;
    Some((size_at + 4, size))
}

/// Reads every box of the property section. Walks the submeshes in file order the way the engine
/// stores them: `MaterialName`, `Extends`, the four longs, then the culling nodes.
pub fn read_boxes(d: &[u8]) -> Result<MeshBoxes> {
    ensure!(risen_formats::xmsh::is_xmsh(d), "not a GR01MS02 mesh");
    let end = u32_at(d, 0x10)? as usize;
    let mut at = 0x28;
    let mut submeshes = vec![];
    loop {
        let Some((mat_at, _)) = find_property(d, at, end, "MaterialName", "bCString") else { break };
        // bCString value: u16 pool/len then text; the tple framing stores `u16 len, text`.
        let n = u16_at(d, mat_at)? as usize;
        let material = String::from_utf8_lossy(&d[mat_at + 2..mat_at + 2 + n]).to_string();
        let Some((ext_at, 24)) = find_property(d, mat_at, end, "Extends", "bCBox") else { bail!("submesh {material}: no Extends after 0x{mat_at:x}") };
        let mut longs = [0u32; 4];
        let mut p = ext_at + 24;
        for (k, name) in ["FirstIndex", "IndexCount", "FirstVertex", "VertexCount"].iter().enumerate() {
            let Some((v, 4)) = find_property(d, p, (p + 64).min(end), name, "long") else { bail!("submesh {material}: no {name} at 0x{p:x}") };
            longs[k] = u32_at(d, v)?;
            p = v + 4;
        }
        ensure!(u16_at(d, p)? == 201, "submesh {material}: culling nodes do not start with u16 201 at 0x{p:x}");
        let count = u32_at(d, p + 2)? as usize;
        let mut nodes = Vec::with_capacity(count);
        let mut q = p + 6;
        for _ in 0..count {
            nodes.push(CullNode { bbox: read_box(d, q)?, a: u32_at(d, q + 24)?, b: u32_at(d, q + 28)? });
            q += 32;
        }
        submeshes.push(SubmeshBoxes { material, extends: read_box(d, ext_at)?, first_index: longs[0], index_count: longs[1], first_vertex: longs[2], vertex_count: longs[3], nodes });
        at = q;
    }
    let Some((b_at, 24)) = find_property(d, at, end, "Boundary", "bCBox") else { bail!("no Boundary after 0x{at:x}") };
    Ok(MeshBoxes { submeshes, boundary: read_box(d, b_at)? })
}

/// Where position, normal and tangent sit inside one stream-0 vertex. Tangent is optional (the
/// 98 shipped meshes without UVs have none), the other two are required for a height edit.
#[derive(Debug, Clone, Copy)]
pub struct VertexLayout { pub start: usize, pub stride: usize, pub count: usize, pub position: usize, pub normal: usize, pub tangent: Option<usize> }

/// Reads the D3D9 declaration at the data offset (layout in `xmsh_geom.rs`).
pub fn vertex_layout(d: &[u8]) -> Result<VertexLayout> {
    let base = u32_at(d, 0x10)? as usize;
    let (mut position, mut normal, mut tangent) = (None, None, None);
    for k in 0..16 {
        let e = base + k * 8;
        let (stream, offset, ty, usage, index) = (u16_at(d, e)?, u16_at(d, e + 2)? as usize, d[e + 4], d[e + 6], d[e + 7]);
        if ty == 17 { break; }
        if stream != 0 || ty != 2 || index != 0 { continue; } // FLOAT3, usage index 0
        match usage { 0 => position = Some(offset), 3 => normal = Some(offset), 6 => tangent = Some(offset), _ => {} }
    }
    let (Some(position), Some(normal)) = (position, normal) else { bail!("vertex declaration lacks a float3 position or normal: offset 0x{base:x}") };
    let vbytes = u32_at(d, base + 0x80)? as usize;
    let stride = u32_at(d, base + 0x90)? as usize;
    ensure!(stride > 0 && vbytes % stride == 0, "vertex stream size {vbytes} is not a multiple of stride {stride}");
    Ok(VertexLayout { start: base + 0xCC, stride, count: vbytes / stride, position, normal, tangent })
}

/// A round hill (positive `height`) or hollow (negative), in the mesh's own space: centimetres,
/// Y up, centre given as world X/Z (the landscape sits at identity).
#[derive(Debug, Clone, Copy, PartialEq)]
/// `level`: None = a hill of `height` added to the ground; Some(y) = a PLATEAU: inside 60% of the radius the ground
/// is raised to exactly y (cm, world; never lowered), then it blends back into the untouched ground at the rim.
#[cfg(test)]
pub struct Bump { pub x: f32, pub z: f32, pub radius: f32, pub height: f32, pub level: Option<f32> }

#[cfg(test)]
impl Bump {
    /// Height offset at (x, z). `(1 - t²)²` is flat at the top and meets the untouched ground with
    /// zero slope at the rim, so the edge of the edit leaves no crease in the shading.
    pub fn offset(&self, x: f32, y: f32, z: f32) -> f32 {
        let t2 = ((x - self.x).powi(2) + (z - self.z).powi(2)) / (self.radius * self.radius);
        if t2 >= 1.0 { return 0.0; }
        let Some(level) = self.level else { return self.height * (1.0 - t2) * (1.0 - t2) };
        // Plateau: flat top out to 60% of the radius, then the same crease-free (1-u²)² blend to the rim.
        let t = t2.sqrt();
        let w = if t <= 0.6 { 1.0 } else { let u2 = ((t - 0.6) / 0.4).powi(2); (1.0 - u2) * (1.0 - u2) };
        (w * (level - y)).max(0.0)
    }
}

/// What an edit touched, for the log and for the tests' "nothing else changed" proof.
#[derive(Debug, Default, Clone)]
pub struct EditReport { pub moved_vertices: usize, pub renormalised_vertices: usize, pub boxes_rewritten: usize, pub boundary_changed: bool, pub max_offset: f32, pub changed_submeshes: Vec<String> }

type V3 = [f32; 3];
fn sub(a: V3, b: V3) -> V3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn cross(a: V3, b: V3) -> V3 { [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]] }
fn dot(a: V3, b: V3) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn unit(a: V3) -> V3 { let l = dot(a, a).sqrt(); if l > 0.0 { [a[0] / l, a[1] / l, a[2] / l] } else { a } }
fn read_v3(d: &[u8], at: usize) -> V3 { [f32_at(d, at), f32_at(d, at + 4), f32_at(d, at + 8)] }
fn write_v3(d: &mut [u8], at: usize, v: V3) { for k in 0..3 { d[at + 4 * k..at + 4 * k + 4].copy_from_slice(&v[k].to_le_bytes()); } }
fn write_box(d: &mut [u8], at: usize, min: V3, max: V3) { write_v3(d, at, min); write_v3(d, at + 12, max); }

/// Rotation that takes unit `a` onto unit `b`, applied to `v` (Rodrigues). Used to carry the
/// shipped normal and tangent along with the change of the surface instead of recomputing them:
/// the landscape's stored normals are artist/tool normals — only ~half match any smooth average of
/// the faces (p90 error 50°, measured) — so a recompute would repaint every hard edge in the patch.
fn rotate(a: V3, b: V3, v: V3) -> V3 {
    let c = dot(a, b);
    let k = cross(a, b);
    let s = dot(k, k).sqrt();
    if s < 1e-7 { return v; }
    let k = [k[0] / s, k[1] / s, k[2] / s];
    let kv = cross(k, v);
    let kd = dot(k, v);
    [0, 1, 2].map(|i| v[i] * c + kv[i] * s + k[i] * kd * (1.0 - c))
}

/// Angle-weighted face normal sum per welded position (positions keyed by their exact bits), for
/// the positions in `keys` only. Winding is clockwise against the stored normals (249 584 of
/// 250 885 landscape triangles), hence `cross(c - a, b - a)`.
fn geo_normals(pos: &[V3], indices: &[u32], keys: &std::collections::HashSet<[u32; 3]>, key_of: &dyn Fn(usize) -> [u32; 3]) -> std::collections::HashMap<[u32; 3], V3> {
    let mut out: std::collections::HashMap<[u32; 3], V3> = std::collections::HashMap::new();
    for t in indices.chunks_exact(3) {
        let ix = [t[0] as usize, t[1] as usize, t[2] as usize];
        if !ix.iter().any(|&i| keys.contains(&key_of(i))) { continue; }
        let p = ix.map(|i| pos[i]);
        let n = unit(cross(sub(p[2], p[0]), sub(p[1], p[0])));
        for k in 0..3 {
            let kk = key_of(ix[k]);
            if !keys.contains(&kk) { continue; }
            let ang = dot(unit(sub(p[(k + 1) % 3], p[k])), unit(sub(p[(k + 2) % 3], p[k]))).clamp(-1.0, 1.0).acos();
            let e = out.entry(kk).or_insert([0.0; 3]);
            for c in 0..3 { e[c] += n[c] * ang; }
        }
    }
    for v in out.values_mut() { *v = unit(*v); }
    out
}

/// Raises/lowers the ground under `bumps` in a `._xmsh`, in place: same size, same topology. Moves
/// vertex Y, turns the normal and tangent of every vertex whose surrounding faces changed, and
/// rewrites every box (submesh `Extends`, culling nodes, `Boundary`) whose triangles moved.
#[cfg(test)]
pub fn apply_bumps(src: &[u8], bumps: &[Bump]) -> Result<(Vec<u8>, EditReport)> {
    let geo = crate::xmsh_geom::decode(src)?;
    let new: Vec<V3> = geo.positions.iter().map(|p| { let dy: f32 = bumps.iter().map(|b| b.offset(p[0], p[1], p[2])).sum(); [p[0], p[1] + dy, p[2]] }).collect();
    apply_positions(src, &new)
}

/// Every vertex of the mesh at `new` (same count and order as the file), in place: positions,
/// normals/tangents turned with their surrounding faces, every box that covers a moved triangle.
pub fn apply_positions(src: &[u8], new: &[V3]) -> Result<(Vec<u8>, EditReport)> {
    let geo = crate::xmsh_geom::decode(src)?;
    let lay = vertex_layout(src)?;
    ensure!(lay.count == geo.positions.len(), "vertex count mismatch");
    ensure!(new.len() == lay.count, "{} new positions for {} vertices", new.len(), lay.count);
    let boxes = read_boxes(src)?;
    let mut out = src.to_vec();
    let mut rep = EditReport::default();
    let old = geo.positions.clone();
    let new = new.to_vec();
    let key_of = |i: usize| old[i].map(|x| x.to_bits());
    let mut moved = std::collections::HashSet::new();
    for i in 0..new.len() {
        if new[i] != old[i] {
            rep.moved_vertices += 1;
            rep.max_offset = rep.max_offset.max((0..3).map(|k| (new[i][k] - old[i][k]).abs()).fold(0.0, f32::max));
            moved.insert(i);
        }
    }
    if moved.is_empty() { return Ok((out, rep)); }
    // Every welded position that shares a triangle with a moved vertex gets a new face normal.
    let mut touched_keys = std::collections::HashSet::new();
    for t in geo.indices.chunks_exact(3) {
        if t.iter().any(|&i| moved.contains(&(i as usize))) { for &i in t { touched_keys.insert(key_of(i as usize)); } }
    }
    let before = geo_normals(&old, &geo.indices, &touched_keys, &key_of);
    let after = geo_normals(&new, &geo.indices, &touched_keys, &key_of);
    for i in 0..lay.count {
        let v = lay.start + i * lay.stride;
        if moved.contains(&i) { write_v3(&mut out, v + lay.position, new[i]); }
        let k = key_of(i);
        let (Some(&a), Some(&b)) = (before.get(&k), after.get(&k)) else { continue };
        if a == b { continue; }
        write_v3(&mut out, v + lay.normal, unit(rotate(a, b, read_v3(src, v + lay.normal))));
        if let Some(t) = lay.tangent { write_v3(&mut out, v + t, unit(rotate(a, b, read_v3(src, v + t)))); }
        rep.renormalised_vertices += 1;
    }
    // Boxes: a node covers indices [a_i, a_{i+b_i}) of its submesh (19 898/19 898 landscape nodes).
    let aabb = |idx: &[u32]| -> (V3, V3) {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for &i in idx { let p = new[i as usize]; for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
        (lo, hi)
    };
    let hits_moved = |idx: &[u32]| idx.iter().any(|&i| moved.contains(&(i as usize)));
    let (mut blo, mut bhi) = (boxes.boundary.min, boxes.boundary.max);
    for s in &boxes.submeshes {
        let all = &geo.indices[s.first_index as usize..(s.first_index + s.index_count) as usize];
        if !hits_moved(all) { continue; }
        rep.changed_submeshes.push(s.material.clone());
        let (lo, hi) = aabb(all);
        write_box(&mut out, s.extends.at, lo, hi);
        rep.boxes_rewritten += 1;
        for k in 0..3 { blo[k] = blo[k].min(lo[k]); bhi[k] = bhi[k].max(hi[k]); }
        for (ni, n) in s.nodes.iter().enumerate() {
            let stop = s.nodes.get(ni + n.b as usize).map(|x| x.a).unwrap_or(s.index_count) as usize;
            let idx = &all[n.a as usize..stop];
            if !hits_moved(idx) { continue; }
            let (lo, hi) = aabb(idx);
            write_box(&mut out, n.bbox.at, lo, hi);
            rep.boxes_rewritten += 1;
        }
    }
    // The mesh-wide Boundary only ever grows here: a lowered patch inside the world does not shrink
    // it, and leaving it as shipped keeps the entity's ContextBox (its copy in the `.lrent`) valid.
    if blo != boxes.boundary.min || bhi != boxes.boundary.max {
        write_box(&mut out, boxes.boundary.at, blo, bhi);
        rep.boundary_changed = true;
    }
    Ok((out, rep))
}

/// The landscape's archive entry. One mesh for the whole island; there are no tiles or LOD files.
pub const LANDSCAPE_ENTRY_SUFFIX: &str = "/Levelmesh_Landscape_01._xmsh";



#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::test_game;

    fn landscape() -> Option<Vec<u8>> {
        let g = test_game()?;
        let e = g.find_one("/Levelmesh_Landscape_01._xmsh").unwrap();
        Some(g.read_archive(&e).unwrap())
    }

    #[test]
    fn falloff_is_flat_at_the_top_and_zero_at_the_rim() {
        let b = Bump { x: 0.0, z: 0.0, radius: 2000.0, height: 500.0, level: None };
        assert_eq!(b.offset(0.0, 0.0, 0.0), 500.0);
        assert_eq!(b.offset(2000.0, 0.0, 0.0), 0.0);
        assert_eq!(b.offset(0.0, 0.0, 2500.0), 0.0);
        assert!((b.offset(1000.0, 0.0, 0.0) - 500.0 * 0.5625).abs() < 1e-3);
    }

    #[test]
    fn rotate_takes_a_onto_b() {
        let (a, b) = (unit([0.0, 1.0, 0.0]), unit([1.0, 1.0, 0.0]));
        let r = rotate(a, b, a);
        for k in 0..3 { assert!((r[k] - b[k]).abs() < 1e-6); }
    }

    /// Every box in the shipped landscape is the AABB of the triangles it claims, bit for bit: so
    /// rewriting a box from the moved triangles produces exactly what the engine's tool would.
    #[test]
    fn shipped_boxes_are_exact_aabbs() {
        let Some(d) = landscape() else { eprintln!("skip: RISEN_GAME not set"); return };
        let geo = crate::xmsh_geom::decode(&d).unwrap();
        let b = read_boxes(&d).unwrap();
        let aabb = |idx: &[u32]| { let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]); for &i in idx { let p = geo.positions[i as usize]; for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } } (lo, hi) };
        let (mut exact, mut total) = (0, 0);
        let (mut blo, mut bhi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for s in &b.submeshes {
            let all = &geo.indices[s.first_index as usize..(s.first_index + s.index_count) as usize];
            let (lo, hi) = aabb(all);
            assert_eq!((lo, hi), (s.extends.min, s.extends.max), "{} Extends", s.material);
            for k in 0..3 { blo[k] = blo[k].min(lo[k]); bhi[k] = bhi[k].max(hi[k]); }
            for (ni, n) in s.nodes.iter().enumerate() {
                let stop = s.nodes.get(ni + n.b as usize).map(|x| x.a).unwrap_or(s.index_count) as usize;
                total += 1;
                if aabb(&all[n.a as usize..stop]) == (n.bbox.min, n.bbox.max) { exact += 1; }
            }
        }
        assert_eq!((blo, bhi), (b.boundary.min, b.boundary.max), "Boundary = union of the Extends");
        assert_eq!(b.submeshes.len(), 28);
        assert_eq!(total, 19898);
        assert_eq!(exact, total, "culling-node boxes that are exact AABBs");
    }

    /// decode -> raise -> encode -> decode: the second decode is the first plus exactly the bump,
    /// indices/UVs untouched, and every changed byte lies in a field the edit owns.
    #[test]
    fn bump_round_trips_and_touches_only_its_fields() {
        let Some(d) = landscape() else { eprintln!("skip: RISEN_GAME not set"); return };
        let bump = Bump { x: -18130.0, z: -14887.0, radius: 2000.0, height: 500.0, level: None };
        let (out, rep) = apply_bumps(&d, &[bump]).unwrap();
        eprintln!("{rep:?}");
        assert_eq!(out.len(), d.len());
        assert!(rep.moved_vertices > 100 && rep.max_offset > 450.0 && !rep.boundary_changed);
        let a = crate::xmsh_geom::decode(&d).unwrap();
        let b = crate::xmsh_geom::decode(&out).unwrap();
        assert_eq!((&a.indices, &a.uvs, &a.submeshes), (&b.indices, &b.uvs, &b.submeshes));
        for (p, q) in a.positions.iter().zip(&b.positions) {
            let dy = bump.offset(p[0], p[1], p[2]);
            assert_eq!((q[0], q[2]), (p[0], p[2]));
            assert_eq!(q[1], p[1] + dy, "at {p:?}");
        }
        let unit_len = b.normals.iter().all(|n| (dot(*n, *n) - 1.0).abs() < 1e-4);
        assert!(unit_len);
        // Byte diff: allowed = position/normal/tangent of each vertex + the 24 bytes of each box.
        let lay = vertex_layout(&d).unwrap();
        let boxes = read_boxes(&d).unwrap();
        let mut allowed = vec![false; d.len()];
        for i in 0..lay.count {
            let v = lay.start + i * lay.stride;
            for o in [lay.position, lay.normal].into_iter().chain(lay.tangent) { for k in 0..12 { allowed[v + o + k] = true; } }
        }
        let mut box_at = vec![boxes.boundary.at];
        for s in &boxes.submeshes { box_at.push(s.extends.at); box_at.extend(s.nodes.iter().map(|n| n.bbox.at)); }
        for at in box_at { for k in 0..24 { allowed[at + k] = true; } }
        let diff: Vec<usize> = (0..d.len()).filter(|&i| d[i] != out[i]).collect();
        assert!(diff.iter().all(|&i| allowed[i]), "a byte outside the edit's fields changed");
        // Vertices far from the bump keep their exact bytes (normals included).
        let far = (0..lay.count).filter(|&i| { let p = a.positions[i]; (p[0] - bump.x).hypot(p[2] - bump.z) > bump.radius + 2000.0 }).all(|i| { let v = lay.start + i * lay.stride; d[v..v + lay.stride] == out[v..v + lay.stride] });
        assert!(far);
        // Stored boxes of the edited file are again exact AABBs of what they cover.
        let nb = read_boxes(&out).unwrap();
        for s in &nb.submeshes {
            let all = &b.indices[s.first_index as usize..(s.first_index + s.index_count) as usize];
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for &i in all { let p = b.positions[i as usize]; for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
            assert_eq!((lo, hi), (s.extends.min, s.extends.max));
        }
        eprintln!("changed bytes: {}", diff.len());
    }
}
