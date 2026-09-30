//! In-place height patch of the landscape collision sectors
//! (`Levelmesh_Landscape_Snnn_COL._xcom` = `GR01CM00` + `eCCollisionMeshResource2` + PhysX 2.8 cooked
//! `NXS\x01MESH` streams, metres, local to the sector entity). Topology is kept; every field PhysX derives
//! from the vertex positions that matters for queries is recomputed: the quantised no-leaf tree
//! (node boxes + dequantisation coefficients; the rule is bit-exact on 334/336 shipped streams), the mesh AABB, the bounding-sphere radius (grown, centre kept), and the resource's
//! `SubBoundaries` (cm). Left as shipped (not used for a static mesh, or cosmetic): the mass/inertia/COM
//! block and the per-triangle edge flags ("extra triangle data").
use anyhow::{bail, ensure, Result};

fn u32_at(d: &[u8], at: usize) -> Result<u32> { d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| anyhow::anyhow!("u32 past end at 0x{at:x}")) }
fn f32_at(d: &[u8], at: usize) -> f32 { f32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]]) }
fn put_f32(d: &mut [u8], at: usize, v: f32) { d[at..at + 4].copy_from_slice(&v.to_le_bytes()); }
fn find(d: &[u8], from: usize, pat: &[u8]) -> Option<usize> { d.get(from..)?.windows(pat.len()).position(|w| w == pat).map(|p| p + from) }

#[derive(Debug, Clone)]
pub struct Stream {
    pub start: usize,
    pub nv: usize,
    pub voff: usize,
    pub tris: Vec<[u32; 3]>,
    /// `None` when the stream has no quantised tree (OPC type != 3); such a stream is refused if touched.
    pub tree: Option<Tree>,
    /// Offset of the float block after the hybrid model: f32 ?, sphere centre+radius, AABB min/max, ...
    pub tail: Option<usize>,
}
#[derive(Debug, Clone)]
pub struct Tree { pub nodes_off: usize, pub nodes: Vec<(u32, u32)>, pub coeff_off: usize, pub leaves: Vec<u32> }

pub fn parse_streams(d: &[u8]) -> Result<Vec<Stream>> {
    let mut out = vec![];
    let mut s = find(d, 0, b"NXS\x01MESH");
    while let Some(st) = s {
        let flags = u32_at(d, st + 12)?;
        let nv = u32_at(d, st + 28)? as usize;
        let nt = u32_at(d, st + 32)? as usize;
        let voff = st + 36;
        let mut o = voff + 12 * nv;
        let isz = if flags & 8 != 0 { 1 } else if flags & 16 != 0 { 2 } else { 4 };
        let mut tris = Vec::with_capacity(nt);
        for t in 0..nt {
            let mut tri = [0u32; 3];
            for (k, v) in tri.iter_mut().enumerate() {
                let a = o + (3 * t + k) * isz;
                *v = match isz { 1 => d[a] as u32, 2 => u16::from_le_bytes([d[a], d[a + 1]]) as u32, _ => u32_at(d, a)? };
                ensure!((*v as usize) < nv, "stream @{st}: index out of range");
            }
            tris.push(tri);
        }
        o += 3 * nt * isz;
        if flags & 1 != 0 { o += 2 * nt; }
        if flags & 2 != 0 { let mx = u32_at(d, o)?; o += 4 + nt * if mx < 256 { 1 } else if mx < 65536 { 2 } else { 4 }; }
        let next = find(d, st + 8, b"NXS\x01MESH");
        let end = next.unwrap_or(d.len());
        let (mut tree, mut tail) = (None, None);
        if let Some(j) = find(d, o, b"OPC\x01").filter(|&j| j < end) {
            if u32_at(d, j + 8)? == 3 {
                let nn = u32_at(d, j + 12)? as usize;
                let nodes_off = j + 16;
                let nodes = (0..nn).map(|k| Ok((u32_at(d, nodes_off + 20 * k + 12)?, u32_at(d, nodes_off + 20 * k + 16)?))).collect::<Result<Vec<_>>>()?;
                let coeff_off = nodes_off + 20 * nn;
                let h = coeff_off + 24;
                ensure!(d.get(h..h + 4) == Some(b"HBM\x01"), "stream @{st}: HBM expected at 0x{h:x}");
                let nl = u32_at(d, h + 8)? as usize;
                let mut o2 = h + 12;
                let mut leaves = vec![];
                if nl > 1 {
                    let mx = u32_at(d, o2)?; o2 += 4;
                    let w = if mx < 256 { 1 } else if mx < 65536 { 2 } else { 4 };
                    for k in 0..nl { let a = o2 + k * w; leaves.push(match w { 1 => d[a] as u32, 2 => u16::from_le_bytes([d[a], d[a + 1]]) as u32, _ => u32_at(d, a)? }); }
                    o2 += w * nl;
                }
                let nprim = u32_at(d, o2)?; o2 += 4;
                if nprim == 0 && nl > 1 {
                    tree = Some(Tree { nodes_off, nodes, coeff_off, leaves });
                    tail = Some(o2);
                }
            }
        }
        out.push(Stream { start: st, nv, voff, tris, tree, tail });
        s = next;
    }
    Ok(out)
}

type V3 = [f32; 3];
fn verts(d: &[u8], s: &Stream) -> Vec<V3> { (0..s.nv).map(|i| { let a = s.voff + 12 * i; [f32_at(d, a), f32_at(d, a + 4), f32_at(d, a + 8)] }).collect() }

/// Float AABB of every node (min, max), children first, as the engine builds them.
fn node_boxes(v: &[V3], s: &Stream, t: &Tree) -> Result<Vec<(V3, V3)>> {
    let leaf = |li: usize| -> Result<(V3, V3)> {
        let data = *t.leaves.get(li).ok_or_else(|| anyhow::anyhow!("leaf {li} out of range"))? as usize;
        let (st, n) = (data >> 4, (data & 15) + 1);
        let mut mn = [f32::MAX; 3]; let mut mx = [f32::MIN; 3];
        for tri in s.tris.get(st..st + n).ok_or_else(|| anyhow::anyhow!("leaf triangles out of range"))? { for &i in tri { for a in 0..3 { mn[a] = mn[a].min(v[i as usize][a]); mx[a] = mx[a].max(v[i as usize][a]); } } }
        Ok((mn, mx))
    };
    let mut boxes = vec![([0f32; 3], [0f32; 3]); t.nodes.len()];
    fn rec(i: usize, t: &Tree, leaf: &dyn Fn(usize) -> Result<(V3, V3)>, boxes: &mut Vec<(V3, V3)>) -> Result<(V3, V3)> {
        let (a, _) = t.nodes[i];
        let (c0, c1) = if a == 0xDEAD {
            let l = i + 1; let r = l + t.nodes.get(l).ok_or_else(|| anyhow::anyhow!("node"))?.1 as usize + 1;
            (rec(l, t, leaf, boxes)?, rec(r, t, leaf, boxes)?)
        } else {
            let idx = (a & 0x3FFF_FFFF) as usize;
            match a >> 30 { 3 => (leaf(idx)?, leaf(idx + 1)?), 2 => (leaf(idx)?, rec(i + 1, t, leaf, boxes)?), 1 => (rec(i + 1, t, leaf, boxes)?, leaf(idx)?), _ => bail!("node {i}: unknown kind 0x{a:x}") }
        };
        let b = ([c0.0[0].min(c1.0[0]), c0.0[1].min(c1.0[1]), c0.0[2].min(c1.0[2])], [c0.1[0].max(c1.1[0]), c0.1[1].max(c1.1[1]), c0.1[2].max(c1.1[2])]);
        boxes[i] = b;
        Ok(b)
    }
    rec(0, t, &leaf, &mut boxes)?;
    Ok(boxes)
}

/// Tree quantisation (15-bit centres and extents, then the "fix quantized boxes" loop), in the
/// precision that reproduces the shipped bytes: products in double, the loop's dequantised terms in f32.
pub fn quantize(boxes: &[(V3, V3)]) -> (Vec<[u16; 6]>, [f32; 6]) {
    let c: Vec<V3> = boxes.iter().map(|(mn, mx)| [(mx[0] + mn[0]) * 0.5, (mx[1] + mn[1]) * 0.5, (mx[2] + mn[2]) * 0.5]).collect();
    let e: Vec<V3> = boxes.iter().map(|(mn, mx)| [(mx[0] - mn[0]) * 0.5, (mx[1] - mn[1]) * 0.5, (mx[2] - mn[2]) * 0.5]).collect();
    let mut cm = [0f32; 3]; let mut em = [0f32; 3];
    for i in 0..c.len() { for a in 0..3 { cm[a] = cm[a].max(c[i][a].abs()); em[a] = em[a].max(e[i][a]); } }
    let cq: V3 = std::array::from_fn(|a| if cm[a] != 0.0 { 32767.0 / cm[a] } else { 0.0 });
    let eq: V3 = std::array::from_fn(|a| if em[a] != 0.0 { 32767.0 / em[a] } else { 0.0 });
    let cc: V3 = std::array::from_fn(|a| if cq[a] != 0.0 { 1.0 / cq[a] } else { 0.0 });
    let ec: V3 = std::array::from_fn(|a| if eq[a] != 0.0 { 1.0 / eq[a] } else { 0.0 });
    let mut out = vec![];
    for i in 0..c.len() {
        let mut q = [0u16; 6];
        for a in 0..3 {
            let qc = (c[i][a] as f64 * cq[a] as f64) as i32 as i16;
            let mut qe = (e[i][a] as f64 * eq[a] as f64) as i64 as u16;
            let (mx, mn) = (c[i][a] + e[i][a], c[i][a] - e[i][a]);
            let dc = (qc as f32 * cc[a]) as f64;
            loop {
                let de = (qe as f32 * ec[a]) as f64;
                if dc + de < mx as f64 || dc - de > mn as f64 { qe = qe.wrapping_add(1); } else { break; }
                if qe == 0 { qe = 0xFFFF; break; }
            }
            q[a] = qc as u16; q[3 + a] = qe;
        }
        out.push(q);
    }
    (out, [cc[0], cc[1], cc[2], ec[0], ec[1], ec[2]])
}

/// Recomputes the stored tree of a stream from the vertices in `d`; returns (nodes, coeffs).
#[cfg(test)]
pub fn recompute_tree(d: &[u8], s: &Stream) -> Result<Option<(Vec<[u16; 6]>, [f32; 6])>> {
    let Some(t) = &s.tree else { return Ok(None) };
    Ok(Some(quantize(&node_boxes(&verts(d, s), s, t)?)))
}

fn sub_boundaries_off(d: &[u8]) -> Result<(usize, usize)> {
    let k = find(d, 0, b"SubBoundaries").ok_or_else(|| anyhow::anyhow!("no SubBoundaries"))?;
    let a = k + 13 + 2 + "bTObjArray<class bCBox>".len() + 2 + 4 + 1;
    Ok((a + 4, u32_at(d, a)? as usize))
}

#[derive(Debug, Default, Clone)]
pub struct ColReport { pub moved_vertices: usize, pub streams_touched: usize, pub nodes_rewritten: usize, pub max_offset_m: f32 }

/// `offset(x_local_m, z_local_m)` = height change in metres. Keeps the file length.
pub fn apply(src: &[u8], offset: &dyn Fn(f32, f32, f32) -> f32) -> Result<(Vec<u8>, ColReport)> {
    let streams = parse_streams(src)?;
    let (sb_off, sb_n) = sub_boundaries_off(src)?;
    ensure!(sb_n == streams.len(), "SubBoundaries {sb_n} != streams {}", streams.len());
    let mut d = src.to_vec();
    let mut rep = ColReport::default();
    for (si, s) in streams.iter().enumerate() {
        let mut moved = 0;
        for i in 0..s.nv {
            let a = s.voff + 12 * i;
            let dy = offset(f32_at(&d, a), f32_at(&d, a + 4), f32_at(&d, a + 8));
            if dy != 0.0 { let y = f32_at(&d, a + 4) + dy; put_f32(&mut d, a + 4, y); moved += 1; rep.max_offset_m = rep.max_offset_m.max(dy.abs()); }
        }
        if moved == 0 { continue; }
        let (Some(t), Some(tail)) = (&s.tree, s.tail) else { bail!("stream {si} @{} is under the brush but has no quantised tree (not supported)", s.start) };
        rep.moved_vertices += moved; rep.streams_touched += 1;
        let v = verts(&d, s);
        let (q, co) = quantize(&node_boxes(&v, s, t)?);
        for (k, n) in q.iter().enumerate() {
            let at = t.nodes_off + 20 * k;
            let old: Vec<u8> = d[at..at + 12].to_vec();
            for j in 0..6 { d[at + 2 * j..at + 2 * j + 2].copy_from_slice(&n[j].to_le_bytes()); }
            if d[at..at + 12] != old[..] { rep.nodes_rewritten += 1; }
        }
        for j in 0..6 { put_f32(&mut d, t.coeff_off + 4 * j, co[j]); }
        let mut mn = [f32::MAX; 3]; let mut mx = [f32::MIN; 3];
        for p in &v { for a in 0..3 { mn[a] = mn[a].min(p[a]); mx[a] = mx[a].max(p[a]); } }
        // tail: f32 ?, sphere centre (3) + radius, AABB min (3), max (3)
        for a in 0..3 { put_f32(&mut d, tail + 20 + 4 * a, mn[a]); put_f32(&mut d, tail + 32 + 4 * a, mx[a]); }
        let cen = [f32_at(&d, tail + 4), f32_at(&d, tail + 8), f32_at(&d, tail + 12)];
        let r2 = v.iter().map(|p| (0..3).map(|a| (p[a] - cen[a]).powi(2)).sum::<f32>()).fold(0f32, f32::max);
        let r = f32_at(&d, tail + 16).max(r2.sqrt() * 1.0001);
        put_f32(&mut d, tail + 16, r);
        // SubBoundaries (cm), a hair outward so the f32 rounding of *100 never shrinks it
        let b = sb_off + 24 * si;
        for a in 0..3 { put_f32(&mut d, b + 4 * a, mn[a] * 100.0 - 0.01); put_f32(&mut d, b + 12 + 4 * a, mx[a] * 100.0 + 0.01); }
    }
    ensure!(d.len() == src.len());
    Ok((d, rep))
}

pub const COL_LRENT: &str = "/World/_Level/Levelmesh_Landscape_COL.lrent";


#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::test_game;

    fn sectors() -> Option<Vec<(String, Vec<u8>)>> {
        let g = test_game()?;
        Some(g.entries_with_suffix("_COL._xcom").into_iter().filter(|e| e.contains("Levelmesh_Landscape_S")).map(|e| { let d = g.read_archive(&e).unwrap(); (e, d) }).collect())
    }

    /// The quantisation rule re-derives the stored tree bytes from the stored vertices.
    #[test]
    fn shipped_trees_rederive_bit_exact() {
        let Some(all) = sectors() else { return };
        let (mut ok, mut bad, mut coeff_ok, mut none) = (0, 0, 0, 0);
        for (e, d) in &all {
            for s in parse_streams(d).unwrap() {
                let Some((q, co)) = recompute_tree(d, &s).unwrap() else { none += 1; continue };
                let t = s.tree.as_ref().unwrap();
                let got: Vec<[u16; 6]> = (0..t.nodes.len()).map(|k| std::array::from_fn(|j| u16::from_le_bytes([d[t.nodes_off + 20 * k + 2 * j], d[t.nodes_off + 20 * k + 2 * j + 1]]))).collect();
                if got == q { ok += 1 } else { bad += 1; eprintln!("{e} @{}: {} of {} nodes differ", s.start, got.iter().zip(&q).filter(|(a, b)| a != b).count(), q.len()); }
                if (0..6).all(|j| co[j].to_bits() == f32_at(d, t.coeff_off + 4 * j).to_bits()) { coeff_ok += 1 }
            }
        }
        eprintln!("trees bit-exact {ok}, differ {bad}, coeffs exact {coeff_ok}, streams without a quantised tree {none}");
        assert_eq!(coeff_ok, ok + bad);
        assert!(bad <= 2, "only the two degenerate zero-extent streams may differ");
    }

    #[test]
    fn bump_patch_is_consistent_and_local() {
        let Some(g) = test_game() else { return };
        let e = g.find_one("/Levelmesh_Landscape_S122_COL._xcom").unwrap();
        let src = g.read_archive(&e).unwrap();
        // a 20 m / 5 m bump centred on this sector's first stream's vertex 0 (local metres)
        let s0 = &parse_streams(&src).unwrap()[0];
        let (cx, cz) = (f32_at(&src, s0.voff), f32_at(&src, s0.voff + 8));
        let b = crate::terrain::Bump { x: cx * 100.0, z: cz * 100.0, radius: 2000.0, height: 500.0, level: None };
        let (out, rep) = apply(&src, &|x, y, z| b.offset(x * 100.0, y * 100.0, z * 100.0) / 100.0).unwrap();
        assert_eq!(out.len(), src.len());
        assert!(rep.moved_vertices > 0 && rep.nodes_rewritten > 0, "{rep:?}");
        let (a, b2) = (parse_streams(&src).unwrap(), parse_streams(&out).unwrap());
        for (s, t) in a.iter().zip(&b2) {
            assert_eq!(s.tris, t.tris);
            let (vo, vn) = (verts(&src, s), verts(&out, t));
            for (p, q) in vo.iter().zip(&vn) {
                assert_eq!(p[0].to_bits(), q[0].to_bits()); assert_eq!(p[2].to_bits(), q[2].to_bits());
                assert_eq!(q[1], p[1] + b.offset(p[0] * 100.0, p[1] * 100.0, p[2] * 100.0) / 100.0);
            }
            // every dequantised node box contains its float box, and re-deriving is a fixed point
            if let Some(tt) = &t.tree {
                let fb = node_boxes(&vn, t, tt).unwrap();
                for (k, (mn, mx)) in fb.iter().enumerate() {
                    for a3 in 0..3 {
                        let at = tt.nodes_off + 20 * k;
                        let qc = i16::from_le_bytes([out[at + 2 * a3], out[at + 2 * a3 + 1]]) as f32 * f32_at(&out, tt.coeff_off + 4 * a3);
                        let qe = u16::from_le_bytes([out[at + 6 + 2 * a3], out[at + 7 + 2 * a3]]) as f32 * f32_at(&out, tt.coeff_off + 12 + 4 * a3);
                        assert!(qc - qe <= mn[a3] + 1e-4 && qc + qe >= mx[a3] - 1e-4, "node {k} axis {a3}");
                    }
                }
                let (q, _) = recompute_tree(&out, t).unwrap().unwrap();
                let got: Vec<[u16; 6]> = (0..tt.nodes.len()).map(|k| std::array::from_fn(|j| u16::from_le_bytes([out[tt.nodes_off + 20 * k + 2 * j], out[tt.nodes_off + 20 * k + 2 * j + 1]]))).collect();
                assert_eq!(got, q);
            }
        }
        // bytes outside touched streams' vertex/tree/tail/SubBoundaries are unchanged: count changed bytes by region
        let changed = src.iter().zip(&out).filter(|(x, y)| x != y).count();
        eprintln!("{rep:?}, changed bytes {changed}");
    }
}
