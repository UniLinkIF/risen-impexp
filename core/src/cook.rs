//! Our PhysX 2.8 triangle-mesh "cooker": triangles → the stream `nxs::write_trimesh` writes, built
//! the way the shipped files are (see `nxs.rs` for the layout, and the tests there for how each
//! rule was measured on the game's 1087 collision streams):
//!
//! * vertices welded and unused ones dropped; indices 8/16/32-bit by vertex count;
//! * an AABB tree with at most 8 triangles per leaf (a single leaf up to 8 triangles), triangles
//!   reordered leaf by leaf, `remap` = new → original;
//! * nodes quantised exactly like the engine: coefficient = 1 / (32767 / max |centre| or max extent),
//!   centres truncated, extents truncated then grown until the box contains the real one;
//! * convex parts = regions grown across non-concave edges; flat parts = coplanar groups numbered
//!   inside their convex part, the count being the most any convex part has;
//! * edge flags: bit 3+k = edge k is convex (a boundary edge, or the neighbour's normal more than
//!   cos⁻¹ 0.995 away and the neighbour bending down); bits 0–2 set (99.99 % / 85 % of the shipped
//!   bits follow these);
//! * geometric epsilon = 2 · FLT_EPSILON · max |coordinate|; mass = |signed volume| at density 1,
//!   centre of mass, inertia about the origin (sign of the volume) — Mirtich's integrals.

use crate::nxs::{self, Model, Node, TriMesh};
use anyhow::{ensure, Result};
use std::collections::HashMap;

const LEAF: usize = 8;
const CONVEX_COS: f32 = 0.995;

type V3 = [f32; 3];
fn sub(a: V3, b: V3) -> V3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn cross(a: V3, b: V3) -> V3 { [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]] }
fn dot(a: V3, b: V3) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn unit(a: V3) -> V3 { let l = dot(a, a).sqrt(); if l > 0.0 { [a[0] / l, a[1] / l, a[2] / l] } else { [0.0; 3] } }

enum Tree { Leaf(Vec<usize>), Node(Box<Tree>, Box<Tree>) }

fn tri_box(verts: &[V3], t: &[u32; 3]) -> (V3, V3) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for &i in t { let p = verts[i as usize]; for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
    (lo, hi)
}

/// Median split of the triangle centres on the longest axis of their spread.
fn build(verts: &[V3], tris: &[[u32; 3]], mut ids: Vec<usize>) -> Tree {
    if ids.len() <= LEAF { return Tree::Leaf(ids); }
    let c = |t: usize| { let (lo, hi) = tri_box(verts, &tris[t]); [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5] };
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for &t in &ids { let p = c(t); for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
    let axis = (0..3).max_by(|&a, &b| (hi[a] - lo[a]).partial_cmp(&(hi[b] - lo[b])).unwrap()).unwrap();
    ids.sort_by(|&a, &b| c(a)[axis].partial_cmp(&c(b)[axis]).unwrap().then(a.cmp(&b)));
    let right = ids.split_off(ids.len() / 2);
    Tree::Node(Box::new(build(verts, tris, ids)), Box::new(build(verts, tris, right)))
}

struct Flat { order: Vec<usize>, leaves: Vec<u32>, nodes: Vec<(Option<usize>, usize, usize)> }

/// Depth first: leaves numbered and triangles ordered as visited; a node's leaf child goes first
/// (the stream only encodes "leaf, node" and "leaf, leaf" pairs besides "node, node").
fn flatten(t: &Tree, f: &mut Flat) -> Result<Option<usize>> {
    match t {
        Tree::Leaf(ids) => {
            let start = f.order.len();
            f.order.extend(ids);
            ensure!(ids.len() >= 1 && ids.len() <= 16, "leaf of {} triangles", ids.len());
            f.leaves.push(((start as u32) << 4) | (ids.len() as u32 - 1));
            Ok(None)
        }
        Tree::Node(a, b) => {
            let (a, b) = if matches!(**a, Tree::Node(..)) && matches!(**b, Tree::Leaf(_)) { (b, a) } else { (a, b) };
            let me = f.nodes.len();
            f.nodes.push((None, 0, 0));
            let first_leaf = f.leaves.len();
            let ca = flatten(a, f)?;
            let cb = flatten(b, f)?;
            let size = f.nodes.len() - me - 1;
            let a_word = match (ca, cb) {
                (Some(_), _) => 0xDEAD,
                (None, None) => 0xC000_0000 | first_leaf as u32,
                (None, Some(_)) => 0x8000_0000 | first_leaf as u32,
            };
            f.nodes[me] = (Some(me), a_word as usize, size);
            Ok(Some(me))
        }
    }
}

fn quantize(boxes: &[(V3, V3)], flat: &Flat) -> (Vec<Node>, [f32; 6]) {
    let centers: Vec<V3> = boxes.iter().map(|(l, h)| [0, 1, 2].map(|k| (l[k] + h[k]) * 0.5)).collect();
    let exts: Vec<V3> = boxes.iter().map(|(l, h)| [0, 1, 2].map(|k| (h[k] - l[k]) * 0.5)).collect();
    let (mut cq, mut eq, mut coeffs) = ([0f32; 3], [0f32; 3], [0f32; 6]);
    for k in 0..3 {
        let cmax = centers.iter().map(|c| c[k].abs()).fold(0.0, f32::max);
        let emax = exts.iter().map(|c| c[k].abs()).fold(0.0, f32::max);
        cq[k] = if cmax != 0.0 { 32767.0 / cmax } else { 0.0 };
        eq[k] = if emax != 0.0 { 32767.0 / emax } else { 0.0 };
        coeffs[k] = if cq[k] != 0.0 { 1.0 / cq[k] } else { 0.0 };
        coeffs[3 + k] = if eq[k] != 0.0 { 1.0 / eq[k] } else { 0.0 };
    }
    let nodes = (0..boxes.len()).map(|i| {
        let center = [0, 1, 2].map(|k| (centers[i][k] * cq[k]) as i16);
        let mut extents = [0, 1, 2].map(|k| (exts[i][k] * eq[k]) as u16);
        for j in 0..3 {
            // The engine compares with centre ± extent; the real min/max is stricter by a rounding step.
            let (mx, mn) = (boxes[i].1[j].max(centers[i][j] + exts[i][j]), boxes[i].0[j].min(centers[i][j] - exts[i][j]));
            let c = center[j] as f32 * coeffs[j];
            loop {
                let q = extents[j] as f32 * coeffs[3 + j];
                if c + q < mx || c - q > mn { extents[j] = extents[j].wrapping_add(1); if extents[j] == 0 { extents[j] = 0xffff; break; } } else { break; }
            }
        }
        let (_, a, b) = flat.nodes[i];
        Node { center, extents, a: a as u32, b: b as u32 }
    }).collect();
    (nodes, coeffs)
}

/// Convex regions grown across non-concave edges (seeded in triangle order), and coplanar groups
/// numbered inside each region.
fn parts(verts: &[V3], tris: &[[u32; 3]], normals: &[V3], adj: &HashMap<(u32, u32), Vec<usize>>) -> (u32, Vec<u16>, u32, Vec<u16>) {
    let nt = tris.len();
    let other = |t: usize, a: u32, b: u32| adj[&(a.min(b), a.max(b))].iter().copied().find(|&o| o != t);
    let mut convex = vec![u16::MAX; nt];
    let mut n_convex = 0u32;
    for seed in 0..nt {
        if convex[seed] != u16::MAX { continue; }
        let id = n_convex.min(u16::MAX as u32 - 1) as u16;
        n_convex += 1;
        convex[seed] = id;
        let mut stack = vec![seed];
        while let Some(t) = stack.pop() {
            for k in 0..3 {
                let (a, b) = (tris[t][k], tris[t][(k + 1) % 3]);
                let Some(o) = other(t, a, b) else { continue };
                if convex[o] != u16::MAX { continue; }
                let ov = tris[o].iter().copied().find(|&v| v != a && v != b).unwrap_or(a);
                if dot(normals[t], sub(verts[ov as usize], verts[a as usize])) <= 1e-6 { convex[o] = id; stack.push(o); }
            }
        }
    }
    let mut flat = vec![u16::MAX; nt];
    let mut per_part: HashMap<u16, u16> = HashMap::new();
    for seed in 0..nt {
        if flat[seed] != u16::MAX { continue; }
        let next = per_part.entry(convex[seed]).or_insert(0);
        let id = *next;
        *next = next.saturating_add(1);
        flat[seed] = id;
        let mut stack = vec![seed];
        while let Some(t) = stack.pop() {
            for k in 0..3 {
                let (a, b) = (tris[t][k], tris[t][(k + 1) % 3]);
                let Some(o) = other(t, a, b) else { continue };
                if flat[o] == u16::MAX && convex[o] == convex[t] && dot(normals[o], normals[t]) > 0.99999 { flat[o] = id; stack.push(o); }
            }
        }
    }
    let n_flat = per_part.values().copied().max().unwrap_or(0) as u32;
    (n_convex, convex, n_flat, flat)
}

fn edge_flags(verts: &[V3], tris: &[[u32; 3]], normals: &[V3], adj: &HashMap<(u32, u32), Vec<usize>>) -> Vec<u8> {
    tris.iter().enumerate().map(|(t, tri)| {
        let mut f = 0b111u8;
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            let others: Vec<usize> = adj[&(a.min(b), a.max(b))].iter().copied().filter(|&o| o != t).collect();
            let convex = match others.as_slice() {
                [] => true,
                [o] => {
                    let ov = tris[*o].iter().copied().find(|&v| v != a && v != b).unwrap_or(a);
                    dot(normals[t], sub(verts[ov as usize], verts[a as usize])) < 0.0 && dot(normals[t], normals[*o]) < CONVEX_COS
                }
                _ => true,
            };
            if convex { f |= 1 << (3 + k); }
        }
        f
    }).collect()
}

/// A sphere around every vertex: Ritter's, grown to fit.
fn bounding_sphere(verts: &[V3]) -> [f32; 4] {
    let far = |from: V3| *verts.iter().max_by(|a, b| dot(sub(**a, from), sub(**a, from)).partial_cmp(&dot(sub(**b, from), sub(**b, from))).unwrap()).unwrap();
    let a = far(verts[0]);
    let b = far(a);
    let mut c = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5];
    let mut r = dot(sub(b, a), sub(b, a)).sqrt() * 0.5;
    for &p in verts {
        let d = dot(sub(p, c), sub(p, c)).sqrt();
        if d > r { let nr = (r + d) * 0.5; let k = (nr - r) / d; c = [c[0] + (p[0] - c[0]) * k, c[1] + (p[1] - c[1]) * k, c[2] + (p[2] - c[2]) * k]; r = nr; }
    }
    // Float rounding must never leave a vertex outside.
    let r = verts.iter().map(|p| dot(sub(*p, c), sub(*p, c)).sqrt()).fold(r, f32::max) * (1.0 + 1e-6);
    [c[0], c[1], c[2], r]
}

/// Triangles (game axes, metres) with one shape-material index each → a cooked stream.
pub fn cook(in_verts: &[V3], in_tris: &[[u32; 3]], materials: &[u16]) -> Result<TriMesh> {
    // Weld exact duplicates, drop unused vertices and degenerate triangles.
    let mut remapv: HashMap<[u32; 3], u32> = HashMap::new();
    let mut verts: Vec<V3> = vec![];
    let mut tris = vec![];
    let mut src = vec![];
    for (ti, t) in in_tris.iter().enumerate() {
        let w = t.map(|i| { let p = in_verts[i as usize]; *remapv.entry(p.map(f32::to_bits)).or_insert_with(|| { verts.push(p); verts.len() as u32 - 1 }) });
        if w[0] == w[1] || w[1] == w[2] || w[0] == w[2] { continue; }
        if dot(cross(sub(verts[w[1] as usize], verts[w[0] as usize]), sub(verts[w[2] as usize], verts[w[0] as usize])), [1.0; 3]).is_nan() { continue; }
        tris.push(w);
        src.push(ti);
    }
    ensure!(!tris.is_empty(), "collision mesh has no triangles");

    let tree = build(&verts, &tris, (0..tris.len()).collect());
    let mut flat = Flat { order: vec![], leaves: vec![], nodes: vec![] };
    flatten(&tree, &mut flat)?;
    let tris: Vec<[u32; 3]> = flat.order.iter().map(|&i| tris[i]).collect();
    let src: Vec<u32> = flat.order.iter().map(|&i| src[i] as u32).collect();

    let model = if flat.nodes.is_empty() {
        // A single leaf is implicit: the stream stores no leaf word for it.
        Model { code: 4, nodes: vec![], coeffs: [0.0; 6], leaves: vec![0] }
    } else {
        // Node boxes bottom-up from their leaves (children always come after their node).
        let leaf_box = |l: u32| { let (s, n) = ((l >> 4) as usize, (l & 15) as usize + 1); let mut b = ([f32::MAX; 3], [f32::MIN; 3]); for t in &tris[s..s + n] { let (lo, hi) = tri_box(&verts, t); for k in 0..3 { b.0[k] = b.0[k].min(lo[k]); b.1[k] = b.1[k].max(hi[k]); } } b };
        let nodes0: Vec<Node> = flat.nodes.iter().map(|&(_, a, b)| Node { center: [0; 3], extents: [0; 3], a: a as u32, b: b as u32 }).collect();
        let mut model = Model { code: 3, nodes: nodes0, coeffs: [0.0; 6], leaves: flat.leaves.clone() };
        let mut boxes = vec![([0f32; 3], [0f32; 3]); model.nodes.len()];
        let ch = children(&model.nodes);
        for i in (0..model.nodes.len()).rev() {
            let mut b = ([f32::MAX; 3], [f32::MIN; 3]);
            for c in &ch[i] { let cb = match c { Child::Node(j) => boxes[*j], Child::Leaf(l) => leaf_box(model.leaves[*l]) }; for k in 0..3 { b.0[k] = b.0[k].min(cb.0[k]); b.1[k] = b.1[k].max(cb.1[k]); } }
            boxes[i] = b;
        }
        let (nodes, coeffs) = quantize(&boxes, &flat);
        model.nodes = nodes;
        model.coeffs = coeffs;
        model
    };

    let normals: Vec<V3> = tris.iter().map(|t| unit(cross(sub(verts[t[1] as usize], verts[t[0] as usize]), sub(verts[t[2] as usize], verts[t[0] as usize])))).collect();
    let mut adj: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() { for k in 0..3 { let (a, b) = (t[k], t[(k + 1) % 3]); adj.entry((a.min(b), a.max(b))).or_default().push(ti); } }
    let (convex_parts, convex_part, flat_parts, flat_part) = parts(&verts, &tris, &normals, &adj);
    let edge_flags = edge_flags(&verts, &tris, &normals, &adj);

    let maxabs = verts.iter().flatten().fold(0f32, |m, x| m.max(x.abs()));
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &verts { for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
    let (vol, com, ic) = nxs::mass_properties(&verts, &tris);
    let s = if vol < 0.0 { -1.0 } else { 1.0 };
    let m = vol.abs();
    let d = com[0] * com[0] + com[1] * com[1] + com[2] * com[2];
    let inertia = [0, 1, 2, 3, 4, 5, 6, 7, 8].map(|k| { let (r, q) = (k / 3, k % 3); (ic[k] * s + m * (if r == q { d } else { 0.0 } - com[r] * com[q])) as f32 });

    let nv = verts.len();
    let flags = nxs::F_MATERIALS | nxs::F_REMAP | if nv <= 256 { nxs::F_8BIT } else if nv <= 65536 { nxs::F_16BIT } else { 0 };
    let materials = src.iter().map(|&i| materials.get(i as usize).copied().unwrap_or(0)).collect();
    Ok(TriMesh {
        flags, edge_threshold: 0.001, sphere: bounding_sphere(&verts), aabb: [lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]],
        verts, tris, materials: Some(materials), remap: Some(src), convex_parts, flat_parts,
        convex_part: if convex_parts > 0 { convex_part } else { vec![] }, flat_part: if flat_parts > 0 { flat_part } else { vec![] },
        model, geom_epsilon: 2.0 * f32::EPSILON * maxabs, mass: m as f32, inertia, com: com.map(|x| x as f32), edge_flags,
    })
}

enum Child { Node(usize), Leaf(usize) }
fn children(nodes: &[Node]) -> Vec<[Child; 2]> {
    (0..nodes.len()).map(|i| {
        let n = &nodes[i];
        let first = if n.a == 0xDEAD { Child::Node(i + 1) } else { Child::Leaf((n.a & 0x3FFF_FFFF) as usize) };
        let second = if n.a != 0xDEAD && n.a & 0x4000_0000 != 0 { Child::Leaf((n.a & 0x3FFF_FFFF) as usize + 1) } else {
            match first { Child::Node(c) => Child::Node(c + 1 + nodes[c].b as usize), Child::Leaf(_) => Child::Node(i + 1) }
        };
        [first, second]
    }).collect()
}

/// A whole `._xcom` for one mesh: centimetres in, metres in the stream (the engine's unit there).
pub fn xcom(verts_cm: &[V3], tris: &[[u32; 3]], shape_material: u8) -> Result<Vec<u8>> {
    let verts: Vec<V3> = verts_cm.iter().map(|p| p.map(|x| x / 100.0)).collect();
    let mesh = cook(&verts, tris, &vec![1; tris.len()])?;
    use crate::gr01::{Body, Prop, Resource, Section, Value};
    let raw = |name: &str, ty: &str, v: Vec<u8>| Prop { name: name.into(), ty: ty.into(), tag: 30, value: Value::Raw(v) };
    let invalid = crate::gr01::bbox_bytes([f32::MAX; 3], [-f32::MAX; 3]);
    let root = Body {
        props: vec![
            raw("IsConvex", "bool", vec![0]),
            raw("Boundary", "bCBox", invalid),
            Prop { name: "SubBoundaries".into(), ty: "bTObjArray<class bCBox>".into(), tag: 30, value: Value::Array { head: 1, elems: vec![] } },
        ],
        rest: 201u16.to_le_bytes().to_vec(),
    };
    let section = Section { prefix: [1, 0, 1, 1, 0, 1], class: "eCCollisionMeshResource2".into(), mid: [1, 0, 0], root, tail: vec![] };
    let ft = crate::cache::filetime(std::time::SystemTime::now());
    let resource = Resource { magic: *b"GR01CM00", filetime: ft.to_le_bytes(), reserved: [0; 8], section, data: vec![] };
    Ok(nxs::write_xcom(&nxs::Xcom { resource, meshes: vec![mesh], shape_materials: vec![nxs::ShapeMaterial { material: shape_material, ignored_by_trace_ray: 0, no_collision: 0, no_response: 0 }] }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains(n: &Node, coeffs: &[f32; 6], lo: V3, hi: V3) -> bool {
        (0..3).all(|k| { let c = n.center[k] as f32 * coeffs[k]; let e = n.extents[k] as f32 * coeffs[3 + k]; c - e <= lo[k] && c + e >= hi[k] })
    }

    /// Cook every shipped collision mesh from its own triangles and compare with the engine's.
    #[test]
    fn cooks_like_the_engine() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        let (mut n, mut same_eps, mut same_aabb, mut mass_ok, mut edge_ok, mut edge_tot) = (0, 0, 0, 0, 0usize, 0usize);
        for e in g.entries_with_suffix("._xcom").into_iter().step_by(3) {
            if e.to_lowercase().ends_with("_cv._xcom") { continue; }
            let x = nxs::read_xcom(&g.read(&e).unwrap().0).unwrap();
            for m in &x.meshes {
                let ours = cook(&m.verts, &m.tris, m.materials.as_deref().unwrap_or(&[])).unwrap();
                n += 1;
                // Round trip through the writer and reader.
                let back = nxs::read_trimesh(&mut crate::gr01::Reader { d: &nxs::write_trimesh(&ours), at: 0 }).unwrap();
                assert_eq!(back, ours, "{e}");
                // Every triangle inside every leaf and every node that covers it.
                assert_eq!(ours.tris.len(), m.tris.len(), "{e}: triangles");
                assert_eq!(ours.verts.len(), m.verts.len(), "{e}: vertices");
                let ch = children(&ours.model.nodes);
                let mut stack: Vec<(usize, Vec<usize>)> = if ours.model.nodes.is_empty() { vec![] } else { vec![(0, vec![])] };
                let mut covered = vec![false; ours.tris.len()];
                while let Some((i, _)) = stack.pop() {
                    for c in &ch[i] {
                        match c {
                            Child::Node(j) => stack.push((*j, vec![])),
                            Child::Leaf(l) => {
                                let lf = ours.model.leaves[*l];
                                for t in (lf >> 4) as usize..(lf >> 4) as usize + (lf & 15) as usize + 1 {
                                    covered[t] = true;
                                    let (lo, hi) = tri_box(&ours.verts, &ours.tris[t]);
                                    assert!(contains(&ours.model.nodes[i], &ours.model.coeffs, lo, hi), "{e}: triangle {t} outside node {i}");
                                }
                            }
                        }
                    }
                }
                if !ours.model.nodes.is_empty() { assert!(covered.iter().all(|&c| c), "{e}: a triangle in no leaf"); }
                if ours.geom_epsilon == m.geom_epsilon { same_eps += 1; } else if same_eps < 3 { let mx = m.verts.iter().flatten().fold(0f32, |a, x| a.max(x.abs())); let ext = (0..3).map(|k| m.aabb[3 + k] - m.aabb[k]).fold(0f32, f32::max); eprintln!("{e}: eps {} ours {} maxabs {mx} max extent {ext} ratio {} {}", m.geom_epsilon, ours.geom_epsilon, m.geom_epsilon / mx / f32::EPSILON, m.geom_epsilon / ext / f32::EPSILON); }
                if ours.aabb == m.aabb { same_aabb += 1; }
                if (ours.mass - m.mass).abs() <= 1e-3 * m.mass.abs().max(1.0) && (0..9).all(|k| (ours.inertia[k] - m.inertia[k]).abs() <= 1e-3 * m.inertia[0].abs().max(m.inertia[4].abs()).max(1.0)) { mass_ok += 1; }
                // Edge convexity bits, matched through the remap (both sides map to original triangles).
                // Our input was their stored order, so our remap points straight at their triangles.
                let (ro, theirs) = (ours.remap.as_ref().unwrap(), &m.edge_flags);
                for (i, &o) in ro.iter().enumerate() {
                    // our triangle i = original o; same vertex order after welding, so edge k matches edge k
                    edge_tot += 3;
                    edge_ok += (0..3).filter(|k| (ours.edge_flags[i] >> (3 + k)) & 1 == (theirs[o as usize] >> (3 + k)) & 1).count();
                }
            }
        }
        eprintln!("{n} streams cooked: geom eps same {same_eps}, aabb same {same_aabb}, mass+inertia within 0.1 % {mass_ok}, convex edge bits {:.3}%", edge_ok as f64 * 100.0 / edge_tot as f64);
        assert!(same_aabb == n && mass_ok * 100 >= n * 95 && edge_ok * 1000 >= edge_tot * 995);
    }
}
