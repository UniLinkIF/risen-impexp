//! Painting the island's ground: which of its 28 ground materials each triangle wears.
//!
//! `Levelmesh_Landscape_01._xmsh` draws one material per submesh, and a submesh is a contiguous
//! index range that owns a contiguous vertex range of its own (no vertex is shared between two
//! submeshes in the shipped file; measured). Moving a triangle to another material therefore
//! rebuilds both buffers: per material, its triangles (archive order, then spatially sorted for
//! the culling tree) and a private copy of every vertex they use — the 60-byte records copied as
//! they are, so positions, normals, tangents, colours, uvs and lightmap uvs never change. A vertex
//! whose triangles end up in two materials is simply copied twice; welding by position (what the
//! editors see) is unchanged. Everything else in the property section stays; per submesh the four
//! longs, `Extends` and the culling tree are written new:
//!
//! ```text
//! tree   u16 201 · u32 n · n × (bCBox 24 · u32 first index (relative to the submesh) · u32 subtree size)
//!        preorder; a node covers indices [a, a of the node after its subtree), internal nodes have
//!        up to 8 children in the shipped file (we write 4), leaves ~12 triangles.
//! ```
//!
//! Empty materials are dropped from the table (no submesh draws nothing in the shipped files).
//!
//! The way back (`recover`): a painted file no longer has the archive's vertex order, so a current
//! file is mapped onto the archive through each vertex record with its height, normal and tangent
//! masked out (x, z, colours, uvs and lightmap uvs never change under any edit).
//!
//! Collision follows in `landscape::set`: each sector's PhysX streams are one per shape material
//! (grass, sand, stone… — footsteps and sounds); a sector with a repainted triangle is re-cooked
//! with its triangles regrouped by their new shape material (`cook.rs`).

use crate::gr01::{bbox_bytes, Resource, Value};
use crate::xmsh_geom::MeshGeometry;
use anyhow::{bail, ensure, Context, Result};
use std::collections::HashMap;
use std::path::Path;

type V3 = [f32; 3];

const V_HEADER: usize = 0x80;
const V_STRIDE: usize = 0x90;
const I_HEADER: usize = 0xBC;
const I_FORMAT: usize = 0xC4;
const V_START: usize = 0xCC;
/// Triangles per culling leaf: the shipped landscape's median leaf holds 12, p90 31.
const LEAF: usize = 16;

fn u32_at(d: &[u8], at: usize) -> Result<u32> { d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| anyhow::anyhow!("u32 past end at 0x{at:x}")) }

/// `paint.bin`: `u32 nt · u32 submesh[nt]`, archive submesh index per triangle in archive order
/// (the order and numbering of `landscape-get`'s `landscape.bin`).
pub fn read_paint(path: &Path) -> Result<Vec<u32>> {
    let d = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    ensure!(d.len() >= 4, "paint file is empty");
    let n = u32::from_le_bytes(d[..4].try_into()?) as usize;
    ensure!(d.len() == 4 + n * 4, "paint file is {} bytes, expected {} for {n} triangles", d.len(), 4 + n * 4);
    Ok((0..n).map(|i| u32::from_le_bytes(d[4 + i * 4..8 + i * 4].try_into().unwrap())).collect())
}

/// Submesh of every triangle of a decoded mesh (file order).
pub fn sub_of(m: &MeshGeometry) -> Vec<u32> {
    let mut s = vec![0u32; m.indices.len() / 3];
    for (k, r) in m.submeshes.iter().enumerate() { for t in r.first_index / 3..(r.first_index + r.index_count) / 3 { s[t as usize] = k as u32; } }
    s
}

/// One culling node: triangles [first, first + count) of the submesh's new order, and the size of
/// its subtree (itself included).
struct Node { first: usize, count: usize, size: usize }

/// Spatial order of `tris` and the preorder tree over it: split at the median along the longer
/// XZ extent of the centres, twice, so a node has up to 4 children; leaves of at most `LEAF`.
fn build_tree(tris: &mut [u32], centre: &dyn Fn(u32) -> [f32; 2], first: usize, out: &mut Vec<Node>) {
    let me = out.len();
    out.push(Node { first, count: tris.len(), size: 1 });
    if tris.len() <= LEAF { return; }
    fn split(tris: &mut [u32], centre: &dyn Fn(u32) -> [f32; 2]) -> usize {
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for &t in tris.iter() { let c = centre(t); for k in 0..2 { lo[k] = lo[k].min(c[k]); hi[k] = hi[k].max(c[k]); } }
        let axis = if hi[0] - lo[0] >= hi[1] - lo[1] { 0 } else { 1 };
        tris.sort_by(|&a, &b| centre(a)[axis].partial_cmp(&centre(b)[axis]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
        tris.len() / 2
    }
    let m = split(tris, centre);
    let (left, right) = tris.split_at_mut(m);
    let lm = split(left, centre);
    let rm = split(right, centre);
    let mut at = first;
    let (l0, l1) = left.split_at_mut(lm);
    let (r0, r1) = right.split_at_mut(rm);
    for part in [l0, l1, r0, r1] {
        if part.is_empty() { continue; }
        let n = part.len();
        build_tree(part, centre, at, out);
        at += n;
    }
    out[me].size = out.len() - me;
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct PaintReport { pub repainted_triangles: usize, pub vertices_before: usize, pub vertices_after: usize, pub materials: usize }

fn long_prop(body: &mut crate::gr01::Body, name: &str, v: u32) -> Result<()> {
    let p = body.props.iter_mut().find(|p| p.name == name).with_context(|| format!("submesh without {name}"))?;
    p.value = Value::Raw(v.to_le_bytes().to_vec());
    Ok(())
}

/// `src` (the landscape, heights already patched in place) with every triangle `t` moved to
/// submesh `new_sub[t]` (indices into `src`'s submesh table). See the module comment.
pub fn repaint(src: &[u8], new_sub: &[u32]) -> Result<(Vec<u8>, PaintReport)> {
    let geo = crate::xmsh_geom::decode(src)?;
    let nt = geo.indices.len() / 3;
    ensure!(new_sub.len() == nt, "paint has {} triangles, the landscape {nt}", new_sub.len());
    let old_sub = sub_of(&geo);
    let mut res = Resource::parse(src)?;
    let d = res.data.clone();
    let stride = u32_at(&d, V_STRIDE)? as usize;
    let vbytes = u32_at(&d, V_HEADER)? as usize;
    let ibytes = u32_at(&d, I_HEADER)? as usize;
    ensure!(u32_at(&d, I_FORMAT)? == 102, "the landscape's indices are not 32-bit");
    ensure!(V_START + vbytes + ibytes <= d.len(), "landscape buffers run past the end");
    let trailer = d[V_START + vbytes + ibytes..].to_vec();
    let root = &mut res.section.root;
    let sm = root.props.iter_mut().find(|p| p.name == "SubMeshes").context("landscape without SubMeshes")?;
    let Value::Array { elems, .. } = &mut sm.value else { bail!("SubMeshes is not an object array") };
    ensure!(elems.len() == geo.submeshes.len(), "submesh table {} vs geometry {}", elems.len(), geo.submeshes.len());
    if let Some(bad) = new_sub.iter().find(|&&s| s as usize >= elems.len()) { bail!("paint names submesh {bad}, the landscape has {}", elems.len()); }

    let mut groups: Vec<Vec<u32>> = vec![vec![]; elems.len()];
    for (t, &s) in new_sub.iter().enumerate() { groups[s as usize].push(t as u32); }
    let centre = |t: u32| -> [f32; 2] {
        let p = [0, 1, 2].map(|c| geo.positions[geo.indices[t as usize * 3 + c] as usize]);
        [(p[0][0] + p[1][0] + p[2][0]) / 3.0, (p[0][2] + p[1][2] + p[2][2]) / 3.0]
    };
    let (mut vbuf, mut ibuf) = (Vec::with_capacity(vbytes + vbytes / 8), Vec::with_capacity(ibytes));
    let mut new_elems = vec![];
    let (mut fv, mut fi) = (0u32, 0u32);
    let (mut blo, mut bhi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (k, group) in groups.iter_mut().enumerate() {
        if group.is_empty() { continue; }
        let mut nodes = vec![];
        build_tree(group, &centre, 0, &mut nodes);
        let mut map: HashMap<u32, u32> = HashMap::new();
        let mut new_pos: Vec<V3> = vec![];
        for &t in group.iter() {
            for c in 0..3 {
                let i = geo.indices[t as usize * 3 + c];
                let ni = *map.entry(i).or_insert_with(|| {
                    vbuf.extend_from_slice(&d[V_START + i as usize * stride..V_START + (i as usize + 1) * stride]);
                    new_pos.push(geo.positions[i as usize]);
                    new_pos.len() as u32 - 1
                });
                ibuf.extend((fv + ni).to_le_bytes());
            }
        }
        let tri_box = |a: usize, n: usize| -> (V3, V3) {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for &t in &group[a..a + n] { for c in 0..3 { let p = geo.positions[geo.indices[t as usize * 3 + c] as usize]; for x in 0..3 { lo[x] = lo[x].min(p[x]); hi[x] = hi[x].max(p[x]); } } }
            (lo, hi)
        };
        let mut tree = 201u16.to_le_bytes().to_vec();
        tree.extend((nodes.len() as u32).to_le_bytes());
        for n in &nodes {
            let (lo, hi) = tri_box(n.first, n.count);
            tree.extend(bbox_bytes(lo, hi));
            tree.extend((n.first as u32 * 3).to_le_bytes());
            tree.extend((n.size as u32).to_le_bytes());
        }
        let (lo, hi) = tri_box(0, group.len());
        for x in 0..3 { blo[x] = blo[x].min(lo[x]); bhi[x] = bhi[x].max(hi[x]); }
        let mut e = elems[k].clone();
        let ext = e.props.iter_mut().find(|p| p.name == "Extends").context("submesh without Extends")?;
        ext.value = Value::Raw(bbox_bytes(lo, hi));
        long_prop(&mut e, "FirstIndex", fi)?;
        long_prop(&mut e, "IndexCount", group.len() as u32 * 3)?;
        long_prop(&mut e, "FirstVertex", fv)?;
        long_prop(&mut e, "VertexCount", new_pos.len() as u32)?;
        ensure!(e.rest.len() >= 6 && e.rest[..2] == [201, 0], "submesh culling tree does not start with 201");
        e.rest = tree;
        new_elems.push(e);
        fv += new_pos.len() as u32;
        fi += group.len() as u32 * 3;
    }
    let materials = new_elems.len();
    *elems = new_elems;
    // The mesh-wide Boundary only grows, as in a height edit (the layer's copy stays valid).
    if let Some(b) = root.props.iter_mut().find(|p| p.name == "Boundary") {
        if let Value::Raw(r) = &mut b.value {
            if r.len() == 24 {
                let f = |i: usize| f32::from_le_bytes(r[i * 4..i * 4 + 4].try_into().unwrap());
                let lo = [f(0).min(blo[0]), f(1).min(blo[1]), f(2).min(blo[2])];
                let hi = [f(3).max(bhi[0]), f(4).max(bhi[1]), f(5).max(bhi[2])];
                *r = bbox_bytes(lo, hi);
            }
        }
    }
    let mut data = d[..V_START].to_vec();
    data[V_HEADER..V_HEADER + 4].copy_from_slice(&(vbuf.len() as u32).to_le_bytes());
    ensure!(ibuf.len() == ibytes, "index buffer changed size");
    data.extend(vbuf);
    data.extend(ibuf);
    data.extend(trailer);
    res.data = data;
    let report = PaintReport { repainted_triangles: (0..nt).filter(|&t| old_sub[t] != new_sub[t]).count(), vertices_before: vbytes / stride, vertices_after: fv as usize, materials };
    Ok((res.write(), report))
}

/// Every vertex record with what an edit may change (height, normal, tangent) zeroed: the key
/// that finds a vertex of a painted file in the archive.
fn vertex_keys(d: &[u8]) -> Result<Vec<Vec<u8>>> {
    let lay = crate::terrain::vertex_layout(d)?;
    Ok((0..lay.count).map(|i| {
        let mut r = d[lay.start + i * lay.stride..lay.start + (i + 1) * lay.stride].to_vec();
        for k in 0..4 { r[lay.position + 4 + k] = 0; }
        for k in 0..12 { r[lay.normal + k] = 0; }
        if let Some(t) = lay.tangent { for k in 0..12 { r[t + k] = 0; } }
        r
    }).collect())
}

/// A landscape file as the archive's vertices and triangles: (position of every archive file
/// vertex, archive submesh index of every archive triangle). Works on an unpainted file (same
/// buffers) and on a painted one (any order, copies). Archive vertices the current file no longer
/// uses keep their archive position.
pub fn recover(archive: &[u8], current: &[u8]) -> Result<(Vec<V3>, Vec<u32>)> {
    let a = crate::xmsh_geom::decode(archive)?;
    let c = crate::xmsh_geom::decode(current)?;
    if c.positions.len() == a.positions.len() && c.indices == a.indices && c.submeshes == a.submeshes {
        return Ok((c.positions, sub_of(&a)));
    }
    let (ka, kc) = (vertex_keys(archive)?, vertex_keys(current)?);
    let mut by_key: HashMap<&[u8], Vec<u32>> = HashMap::new();
    for (i, k) in ka.iter().enumerate() { by_key.entry(k.as_slice()).or_default().push(i as u32); }
    let cand: Vec<&Vec<u32>> = kc.iter().enumerate().map(|(i, k)| by_key.get(k.as_slice()).with_context(|| format!("vertex {i} of the installed landscape is not one of the archive's"))).collect::<Result<_>>()?;
    // The archive repeats 29 triangles exactly (same corners): a key lists all of them.
    let mut tri_of: HashMap<[u32; 3], Vec<u32>> = HashMap::new();
    for t in 0..a.indices.len() / 3 { tri_of.entry([a.indices[t * 3], a.indices[t * 3 + 1], a.indices[t * 3 + 2]]).or_default().push(t as u32); }
    let arch_sub = sub_of(&a);
    let sub_index: HashMap<&str, u32> = a.submeshes.iter().enumerate().map(|(k, s)| (s.material.as_str(), k as u32)).collect();
    let mut pos = a.positions.clone();
    let mut sub = arch_sub.clone();
    let mut seen = vec![false; a.indices.len() / 3];
    for (ck, s) in c.submeshes.iter().enumerate() {
        let k = *sub_index.get(s.material.as_str()).with_context(|| format!("installed landscape material {} is not one of the archive's", s.material))?;
        let _ = ck;
        for t in s.first_index / 3..(s.first_index + s.index_count) / 3 {
            let v = [0, 1, 2].map(|j| c.indices[t as usize * 3 + j] as usize);
            // The archive triangle with these corners (a key may name several archive vertices).
            let mut found = None;
            // Among identical archive triangles: one not taken yet, the one that wore this material first.
            'search: for &x in cand[v[0]] { for &y in cand[v[1]] { for &z in cand[v[2]] {
                let Some(list) = tri_of.get(&[x, y, z]) else { continue };
                let free = list.iter().copied().filter(|&t| !seen[t as usize]);
                if let Some(at) = free.clone().find(|&t| arch_sub[t as usize] == k).or_else(|| free.min()) { found = Some((at, [x, y, z])); break 'search; }
            } } }
            let (at, corners) = found.with_context(|| format!("triangle {t} of the installed landscape is not one of the archive's"))?;
            seen[at as usize] = true;
            sub[at as usize] = k;
            for j in 0..3 { pos[corners[j] as usize] = c.positions[v[j]]; }
        }
    }
    ensure!(seen.iter().all(|&s| s), "the installed landscape lacks {} of the archive's triangles", seen.iter().filter(|&&s| !s).count());
    // 6564 archive vertices belong to no triangle (a painted file drops them). Each takes the new
    // position of a used vertex at the same archive position, so welding (first appearance first)
    // never picks a stale height for its group.
    let mut used = vec![false; a.positions.len()];
    for &i in &a.indices { used[i as usize] = true; }
    let mut by_pos: HashMap<[u32; 3], V3> = HashMap::new();
    for i in 0..a.positions.len() { if used[i] { by_pos.entry(a.positions[i].map(f32::to_bits)).or_insert(pos[i]); } }
    for i in 0..a.positions.len() { if !used[i] { if let Some(p) = by_pos.get(&a.positions[i].map(f32::to_bits)) { pos[i] = *p; } } }
    Ok((pos, sub))
}

/// Risen's shape material (footsteps, sounds, `nxs::SHAPE_MATERIALS`) for a ground material that
/// no collision triangle of the shipped game wears: by its name.
pub fn shape_by_name(material: &str) -> u8 {
    let m = material.to_lowercase();
    let idx = |n: &str| crate::nxs::SHAPE_MATERIALS.iter().position(|x| *x == n).unwrap_or(0) as u8;
    if m.contains("beach") || m.contains("sand") || m.contains("sea_bottom") { idx("sand") }
    else if m.contains("rock") || m.contains("stone") || m.contains("cave") || m.contains("rift") || m.contains("lava") || m.contains("arch_floor") || m.contains("cobbled") { idx("stone") }
    else if m.contains("grass") || m.contains("field") || m.contains("forest") || m.contains("forrest") { idx("grass") }
    else { idx("earth") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landscape() -> Option<Vec<u8>> {
        let g = crate::game::test_game()?;
        Some(g.read_archive(&g.find_one(crate::terrain::LANDSCAPE_ENTRY_SUFFIX).unwrap()).unwrap())
    }

    fn centre(m: &MeshGeometry, t: usize) -> [f32; 2] {
        let p = [0, 1, 2].map(|c| m.positions[m.indices[t * 3 + c] as usize]);
        [(p[0][0] + p[1][0] + p[2][0]) / 3.0, (p[0][2] + p[1][2] + p[2][2]) / 3.0]
    }

    /// A disc of sand in the grass near Harbour, and every Rift triangle turned to rock (so one
    /// material disappears): the file decodes, keeps the shipped structure (each submesh owns its
    /// vertex range, boxes are exact AABBs), and maps back onto the archive with exactly the paint
    /// and the archive's positions; the same after a height edit.
    #[test]
    fn repaint_round_trips_through_recover() {
        let Some(src) = landscape() else { eprintln!("skip: RISEN_GAME not set"); return };
        let a = crate::xmsh_geom::decode(&src).unwrap();
        let arch = sub_of(&a);
        let idx = |name: &str| a.submeshes.iter().position(|s| s.material == name).unwrap() as u32;
        let (beach, rift, rock) = (idx("Nat_Ground_Beach_01_Diffuse_01._xmat"), idx("Nat_Ground_Rift_01_Diffuse_01._xmat"), idx("Nat_Stone_Rock_01_Diffuse_01._xmat"));
        let mut paint = arch.clone();
        for t in 0..paint.len() {
            let c = centre(&a, t);
            if (c[0] + 18130.0).hypot(c[1] + 14887.0) < 3000.0 { paint[t] = beach; }
            if paint[t] == rift { paint[t] = rock; }
        }
        let changed = (0..paint.len()).filter(|&t| paint[t] != arch[t]).count();
        assert!(changed > 100, "{changed}");
        let (out, rep) = repaint(&src, &paint).unwrap();
        eprintln!("{rep:?}");
        assert_eq!(rep.repainted_triangles, changed);
        assert_eq!(rep.materials, a.submeshes.len() - 1);
        assert!(Resource::parse(&out).unwrap().write() == out, "the painted file re-writes byte for byte");
        let b = crate::xmsh_geom::decode(&out).unwrap();
        assert_eq!(b.indices.len(), a.indices.len());
        // Shipped structure: a submesh's indices stay inside its own vertex range; ranges tile the buffer.
        let subs = risen_formats::xmsh::parse_submeshes(&out);
        let mut next_v = 0;
        for s in &subs {
            let (fv, vc, fi, ic) = (s.first_vertex.unwrap() as u32, s.vertex_count.unwrap() as u32, s.first_index.unwrap() as usize, s.index_count.unwrap() as usize);
            assert_eq!(fv, next_v);
            next_v += vc;
            assert!(b.indices[fi..fi + ic].iter().all(|&i| i >= fv && i < fv + vc), "{}", s.material);
        }
        assert_eq!(next_v as usize, b.positions.len());
        // Every box (Extends, culling nodes) is the exact AABB of what it covers, as in the shipped file.
        let boxes = crate::terrain::read_boxes(&out).unwrap();
        for s in &boxes.submeshes {
            let all = &b.indices[s.first_index as usize..(s.first_index + s.index_count) as usize];
            let aabb = |idx: &[u32]| { let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]); for &i in idx { let p = b.positions[i as usize]; for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } } (lo, hi) };
            assert_eq!(aabb(all), (s.extends.min, s.extends.max));
            assert_eq!(s.nodes[0].a, 0);
            assert_eq!(s.nodes[0].b as usize, s.nodes.len());
            for (ni, n) in s.nodes.iter().enumerate() {
                let stop = s.nodes.get(ni + n.b as usize).map(|x| x.a).unwrap_or(s.index_count) as usize;
                assert_eq!(aabb(&all[n.a as usize..stop]), (n.bbox.min, n.bbox.max), "{} node {ni}", s.material);
            }
        }
        assert!(!boxes.submeshes.iter().any(|s| s.material.contains("Rift")));
        let (pos, sub) = recover(&src, &out).unwrap();
        assert!(sub == paint, "recovered paint differs");
        assert!(pos.iter().zip(&a.positions).all(|(p, q)| p.map(f32::to_bits) == q.map(f32::to_bits)));
        // Unpainted: recover is the identity.
        let (_, same) = recover(&src, &src).unwrap();
        assert!(same == arch);
        // Heights and paint together.
        let bump = crate::terrain::Bump { x: -18130.0, z: -14887.0, radius: 2000.0, height: 500.0, level: None };
        let moved: Vec<V3> = a.positions.iter().map(|p| [p[0], p[1] + bump.offset(p[0], p[1], p[2]), p[2]]).collect();
        let (patched, _) = crate::terrain::apply_positions(&src, &moved).unwrap();
        let (both, _) = repaint(&patched, &paint).unwrap();
        let (pos, sub) = recover(&src, &both).unwrap();
        assert!(sub == paint);
        assert!(pos.iter().zip(&moved).all(|(p, q)| p == q));
    }
}
