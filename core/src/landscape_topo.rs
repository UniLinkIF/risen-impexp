//! Topology edits of the island's ground: subdividing triangles (finer ground where an editor
//! needs detail) and removing them (a hole for a cave or cellar entrance). The landscape file then
//! gets new vertex and index counts; the edits travel as a list of operations replayed on the
//! archive's mesh, so every tool numbers vertices and triangles the same way.
//!
//! ```text
//! topology.bin  "LTOP" · u32 version 1 · u32 archive vertices · u32 archive triangles (welded, as
//!               landscape-get numbers them) · u32 ops · ops × (u32 kind: 1 subdivide, 2 remove ·
//!               u32 n · u32 triangle[n]) · u32 result vertices · u32 result triangles
//!               · u64 FNV-1a of the result's welded triangles (u32 LE)
//! ```
//!
//! An operation names triangles of the mesh as the operations before it left it. Numbering is
//! stable: vertices are only appended (a removed triangle leaves its corners); a subdivided
//! triangle keeps its slot for its first child and appends the others; removal keeps the order of
//! the rest. Subdividing splits every edge of the chosen triangles at its midpoint; a neighbour
//! sharing a split edge splits into 2 or 3, so the surface never cracks.
//!
//! In the file each new midpoint is a vertex record interpolated from the two records of its edge
//! (colours, uvs and lightmap uvs averaged, normal and tangent renormalised): one record per pair
//! of file vertices, so a uv seam stays a seam. The rewritten file keeps the shipped structure
//! (one vertex range per submesh, culling trees, exact boxes), and every ground material keeps its
//! submesh: an edit that would remove the last triangle of one is refused.
//!
//! The installed edit is kept next to the landscape as `Levelmesh_Landscape_01.topology`, so the
//! current ground can be read back (`landscape-get current`) and set again by any tool.

use crate::landscape_paint::{build_tree, long_prop, sub_of, u32_at, Node, I_FORMAT, I_HEADER, V_HEADER, V_START, V_STRIDE};
use crate::gr01::{bbox_bytes, Resource, Value};
use anyhow::{bail, ensure, Context, Result};
use std::collections::{HashMap, HashSet};

type V3 = [f32; 3];
type P2 = [f32; 2];

pub const KIND_SUBDIVIDE: u32 = 1;
pub const KIND_REMOVE: u32 = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct Op { pub kind: u32, pub tris: Vec<u32> }

#[derive(Debug, Clone, PartialEq)]
pub struct Spec { pub archive_vertices: u32, pub archive_triangles: u32, pub ops: Vec<Op>, pub result_vertices: u32, pub result_triangles: u32, pub check: u64 }

pub fn read(d: &[u8]) -> Result<Spec> {
    let mut at = 0usize;
    let mut u32n = || -> Result<u32> { let v = d.get(at..at + 4).context("topology file is cut short")?; at += 4; Ok(u32::from_le_bytes(v.try_into().unwrap())) };
    ensure!(u32n()? == u32::from_le_bytes(*b"LTOP"), "not a landscape topology file (LTOP)");
    let ver = u32n()?;
    ensure!(ver == 1, "landscape topology version {ver} is not supported");
    let (archive_vertices, archive_triangles) = (u32n()?, u32n()?);
    let n = u32n()?;
    let mut ops = vec![];
    for _ in 0..n {
        let kind = u32n()?;
        ensure!(kind == KIND_SUBDIVIDE || kind == KIND_REMOVE, "unknown topology operation {kind}");
        let c = u32n()? as usize;
        let mut tris = Vec::with_capacity(c);
        for _ in 0..c { tris.push(u32n()?); }
        ops.push(Op { kind, tris });
    }
    let (result_vertices, result_triangles) = (u32n()?, u32n()?);
    let check = d.get(at..at + 8).context("topology file is cut short")?;
    let check = u64::from_le_bytes(check.try_into().unwrap());
    ensure!(at + 8 == d.len(), "topology file has {} bytes past its end", d.len() - at - 8);
    Ok(Spec { archive_vertices, archive_triangles, ops, result_vertices, result_triangles, check })
}

pub fn write(s: &Spec) -> Vec<u8> {
    let mut o = b"LTOP".to_vec();
    for v in [1, s.archive_vertices, s.archive_triangles, s.ops.len() as u32] { o.extend(v.to_le_bytes()); }
    for op in &s.ops { o.extend(op.kind.to_le_bytes()); o.extend((op.tris.len() as u32).to_le_bytes()); for t in &op.tris { o.extend(t.to_le_bytes()); } }
    o.extend(s.result_vertices.to_le_bytes());
    o.extend(s.result_triangles.to_le_bytes());
    o.extend(s.check.to_le_bytes());
    o
}

/// FNV-1a 64 of the triangles as u32 little-endian.
pub fn checksum(tris: &[[u32; 3]]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for t in tris { for v in t { for b in v.to_le_bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); } } }
    h
}

/// The mesh an edit applies to: for the plain archive, the archive itself; with a topology, the
/// archive with the operations replayed and written out as a landscape file (archive heights,
/// midpoints between them). Every tool's welded numbering maps onto its file vertices and triangles.
pub struct Base {
    pub bytes: Vec<u8>,
    /// File vertex → welded vertex.
    pub map: Vec<u32>,
    /// Welded positions (archive heights).
    pub welded: Vec<V3>,
    /// File triangle → welded triangle.
    pub tri_welded: Vec<u32>,
    /// Welded triangles, their corner uvs as landscape-get writes them (u, 1 − v), their material.
    pub welded_tris: Vec<[u32; 3]>,
    pub welded_uv: Vec<[P2; 3]>,
    pub welded_sub: Vec<u32>,
    /// XZ of the removed triangles (cm): their collision goes too.
    pub holes: Vec<[P2; 3]>,
    pub spec: Option<Spec>,
}

/// The archive as a base (no topology edit): the file as shipped, welded by exact position.
pub fn archive_base(src: &[u8]) -> Result<Base> {
    let m = crate::xmsh_geom::decode(src)?;
    let (welded, map) = crate::landscape::weld(&m.positions);
    let welded_tris: Vec<[u32; 3]> = m.indices.chunks_exact(3).map(|t| [map[t[0] as usize], map[t[1] as usize], map[t[2] as usize]]).collect();
    let uv = |i: u32| { let u = m.uvs.get(i as usize).copied().unwrap_or([0.0; 2]); [u[0], 1.0 - u[1]] };
    let welded_uv = m.indices.chunks_exact(3).map(|t| [uv(t[0]), uv(t[1]), uv(t[2])]).collect();
    let welded_sub = sub_of(&m);
    Ok(Base { bytes: src.to_vec(), map, welded, tri_welded: (0..welded_tris.len() as u32).collect(), welded_tris, welded_uv, welded_sub, holes: vec![], spec: None })
}

/// A vertex declaration element of stream 0: offset, D3DDECLTYPE, usage.
fn declaration(d: &[u8]) -> Result<Vec<(usize, u8, u8)>> {
    let base = u32::from_le_bytes(d.get(0x10..0x14).context("short file")?.try_into()?) as usize;
    let mut out = vec![];
    for k in 0..16 {
        let e = base + k * 8;
        let ty = *d.get(e + 4).context("short declaration")?;
        if ty == 17 { return Ok(out); }
        if u16::from_le_bytes([d[e], d[e + 1]]) != 0 { continue; }
        out.push((u16::from_le_bytes([d[e + 2], d[e + 3]]) as usize, ty, d[e + 6]));
    }
    bail!("vertex declaration without an end")
}

/// The record halfway between two vertex records: floats averaged (normal and tangent
/// renormalised), colours averaged per byte, anything else taken from the first.
fn lerp_record(decl: &[(usize, u8, u8)], a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut o = a.to_vec();
    let f = |r: &[u8], at: usize| f32::from_le_bytes(r[at..at + 4].try_into().unwrap());
    for &(off, ty, usage) in decl {
        let n = match ty { 0 => 1, 1 => 2, 2 => 3, 3 => 4, 4 => { for k in 0..4 { o[off + k] = ((a[off + k] as u16 + b[off + k] as u16) / 2) as u8; } continue } _ => continue };
        let mut v: Vec<f32> = (0..n).map(|k| (f(a, off + 4 * k) + f(b, off + 4 * k)) * 0.5).collect();
        if ty == 2 && (usage == 3 || usage == 6) {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            if l > 0.0 { for x in &mut v { *x /= l; } }
        }
        for (k, x) in v.iter().enumerate() { o[off + 4 * k..off + 4 * k + 4].copy_from_slice(&x.to_le_bytes()); }
    }
    o
}

/// Local ids 0 a, 1 b, 2 c, 3 mid ab, 4 mid bc, 5 mid ca → the children, first one keeps the slot.
fn pattern(ab: bool, bc: bool, ca: bool) -> &'static [[usize; 3]] {
    match (ab, bc, ca) {
        (true, true, true) => &[[0, 3, 5], [3, 1, 4], [5, 4, 2], [3, 4, 5]],
        (true, true, false) => &[[3, 1, 4], [0, 3, 4], [0, 4, 2]],
        (false, true, true) => &[[4, 2, 5], [1, 4, 5], [1, 5, 0]],
        (true, false, true) => &[[5, 0, 3], [2, 5, 3], [2, 3, 1]],
        (true, false, false) => &[[0, 3, 2], [3, 1, 2]],
        (false, true, false) => &[[0, 1, 4], [0, 4, 2]],
        _ => &[[0, 1, 5], [5, 1, 2]],
    }
}

fn key(a: u32, b: u32) -> (u32, u32) { if a < b { (a, b) } else { (b, a) } }

/// The welded mesh with the file record behind every triangle corner.
struct Work { pos: Vec<V3>, tris: Vec<[u32; 3]>, fc: Vec<[u32; 3]>, sub: Vec<u32>, records: Vec<u8>, rec_welded: Vec<u32>, stride: usize, decl: Vec<(usize, u8, u8)>, holes: Vec<[P2; 3]> }

impl Work {
    fn subdivide(&mut self, sel: &[u32]) {
        let nt0 = self.tris.len();
        let split: HashSet<(u32, u32)> = sel.iter().filter(|&&t| (t as usize) < nt0).flat_map(|&t| { let v = self.tris[t as usize]; [key(v[0], v[1]), key(v[1], v[2]), key(v[2], v[0])] }).collect();
        if split.is_empty() { return; }
        let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
        let mut fmid: HashMap<(u32, u32), u32> = HashMap::new();
        for t in 0..nt0 {
            let (v0, f0) = (self.tris[t], self.fc[t]);
            let e = [split.contains(&key(v0[0], v0[1])), split.contains(&key(v0[1], v0[2])), split.contains(&key(v0[2], v0[0]))];
            if !e.iter().any(|&x| x) { continue; }
            let mut v = [v0[0], v0[1], v0[2], u32::MAX, u32::MAX, u32::MAX];
            let mut f = [f0[0], f0[1], f0[2], u32::MAX, u32::MAX, u32::MAX];
            for k in 0..3 {
                if !e[k] { continue; }
                let (p, q) = (k, (k + 1) % 3);
                let w = *mid.entry(key(v[p], v[q])).or_insert_with(|| {
                    let (a, b) = (self.pos[v[p] as usize], self.pos[v[q] as usize]);
                    self.pos.push([(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5]);
                    self.pos.len() as u32 - 1
                });
                v[3 + k] = w;
                let s = self.stride;
                f[3 + k] = *fmid.entry(key(f[p], f[q])).or_insert_with(|| {
                    let (a, b) = (f[p] as usize, f[q] as usize);
                    let r = lerp_record(&self.decl, &self.records[a * s..(a + 1) * s], &self.records[b * s..(b + 1) * s]);
                    self.records.extend(r);
                    self.rec_welded.push(w);
                    self.rec_welded.len() as u32 - 1
                });
            }
            for (ci, ch) in pattern(e[0], e[1], e[2]).iter().enumerate() {
                let (tv, tf) = (ch.map(|c| v[c]), ch.map(|c| f[c]));
                if ci == 0 { self.tris[t] = tv; self.fc[t] = tf; } else { self.tris.push(tv); self.fc.push(tf); self.sub.push(self.sub[t]); }
            }
        }
    }

    fn remove(&mut self, sel: &[u32]) {
        let gone: HashSet<u32> = sel.iter().copied().collect();
        let mut keep = 0;
        for t in 0..self.tris.len() {
            if gone.contains(&(t as u32)) { let v = self.tris[t]; self.holes.push(v.map(|i| [self.pos[i as usize][0], self.pos[i as usize][2]])); continue; }
            self.tris[keep] = self.tris[t]; self.fc[keep] = self.fc[t]; self.sub[keep] = self.sub[t];
            keep += 1;
        }
        self.tris.truncate(keep); self.fc.truncate(keep); self.sub.truncate(keep);
    }
}

/// A spec for `ops` on the archive `src`, with the result counts and checksum filled in.
pub fn seal(src: &[u8], ops: Vec<Op>) -> Result<Spec> {
    let m = crate::xmsh_geom::decode(src)?;
    let (welded, map) = crate::landscape::weld(&m.positions);
    let mut s = Spec { archive_vertices: welded.len() as u32, archive_triangles: (m.indices.len() / 3) as u32, ops, result_vertices: 0, result_triangles: 0, check: 0 };
    let w = replay(src, &s)?;
    let _ = map;
    s.result_vertices = w.pos.len() as u32;
    s.result_triangles = w.tris.len() as u32;
    s.check = checksum(&w.tris);
    Ok(s)
}

fn replay(src: &[u8], spec: &Spec) -> Result<Work> {
    let m = crate::xmsh_geom::decode(src)?;
    let (welded, map) = crate::landscape::weld(&m.positions);
    let nt = m.indices.len() / 3;
    ensure!(spec.archive_vertices as usize == welded.len() && spec.archive_triangles as usize == nt,
        "the topology was made for a landscape of {} vertices and {} triangles, this game's has {} and {nt}", spec.archive_vertices, spec.archive_triangles, welded.len());
    let res = Resource::parse(src)?;
    let d = &res.data;
    let stride = u32_at(d, V_STRIDE)? as usize;
    let vbytes = u32_at(d, V_HEADER)? as usize;
    let mut w = Work {
        pos: welded,
        tris: m.indices.chunks_exact(3).map(|t| [map[t[0] as usize], map[t[1] as usize], map[t[2] as usize]]).collect(),
        fc: m.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect(),
        sub: sub_of(&m),
        records: d[V_START..V_START + vbytes].to_vec(),
        rec_welded: map.clone(),
        stride,
        decl: declaration(src)?,
        holes: vec![],
    };
    for (k, op) in spec.ops.iter().enumerate() {
        if let Some(bad) = op.tris.iter().find(|&&t| t as usize >= w.tris.len()) { bail!("topology operation {k} names triangle {bad} of {}", w.tris.len()); }
        if op.kind == KIND_SUBDIVIDE { w.subdivide(&op.tris) } else { w.remove(&op.tris) }
    }
    Ok(w)
}

/// The archive (`src`) with `spec` replayed: see `Base`. Refuses a spec made for another mesh, or
/// whose result differs from what it claims.
pub fn apply(src: &[u8], spec: &Spec) -> Result<Base> {
    let w = replay(src, spec)?;
    ensure!(w.pos.len() == spec.result_vertices as usize && w.tris.len() == spec.result_triangles as usize && checksum(&w.tris) == spec.check,
        "the topology replays to {} vertices and {} triangles (checksum {:x}), the file says {} and {} ({:x})", w.pos.len(), w.tris.len(), checksum(&w.tris), spec.result_vertices, spec.result_triangles, spec.check);
    let uv_at = decode_uv_offset(&w.decl);
    let rec_uv = |r: u32| -> P2 {
        let Some(o) = uv_at else { return [0.0; 2] };
        let at = r as usize * w.stride + o;
        let f = |k: usize| f32::from_le_bytes(w.records[at + 4 * k..at + 4 * k + 4].try_into().unwrap());
        [f(0), 1.0 - f(1)]
    };
    let welded_uv = w.fc.iter().map(|c| c.map(rec_uv)).collect();
    let (bytes, out_rec, out_tri) = write_mesh(src, &w.records, w.stride, &w.fc, &w.sub)?;
    Ok(Base {
        bytes,
        map: out_rec.iter().map(|&r| w.rec_welded[r as usize]).collect(),
        welded: w.pos,
        tri_welded: out_tri,
        welded_tris: w.tris,
        welded_uv,
        welded_sub: w.sub,
        holes: w.holes,
        spec: Some(spec.clone()),
    })
}

fn decode_uv_offset(decl: &[(usize, u8, u8)]) -> Option<usize> { decl.iter().find(|&&(_, ty, usage)| ty == 1 && usage == 5).map(|&(o, _, _)| o) }

/// `src`'s property section with new buffers: triangles `fc` (record indices into `records`) on
/// submesh `sub[t]` (indices into `src`'s submesh table), each submesh with its own copy of the
/// records it uses, a fresh culling tree and exact boxes (as `landscape_paint::repaint` writes).
/// Returns the file and, per written vertex, its record, and per written triangle, its index in `fc`.
pub fn write_mesh(src: &[u8], records: &[u8], stride: usize, fc: &[[u32; 3]], sub: &[u32]) -> Result<(Vec<u8>, Vec<u32>, Vec<u32>)> {
    let mut res = Resource::parse(src)?;
    let d = res.data.clone();
    ensure!(u32_at(&d, V_STRIDE)? as usize == stride, "stride changed");
    let vbytes = u32_at(&d, V_HEADER)? as usize;
    let ibytes = u32_at(&d, I_HEADER)? as usize;
    ensure!(u32_at(&d, I_FORMAT)? == 102, "the landscape's indices are not 32-bit");
    let trailer = d[V_START + vbytes + ibytes..].to_vec();
    let pos = |r: u32| -> V3 { let at = r as usize * stride; [0, 1, 2].map(|k| f32::from_le_bytes(records[at + 4 * k..at + 4 * k + 4].try_into().unwrap())) };
    let root = &mut res.section.root;
    let sm = root.props.iter_mut().find(|p| p.name == "SubMeshes").context("landscape without SubMeshes")?;
    let Value::Array { elems, .. } = &mut sm.value else { bail!("SubMeshes is not an object array") };
    let mut groups: Vec<Vec<u32>> = vec![vec![]; elems.len()];
    for (t, &s) in sub.iter().enumerate() { groups.get_mut(s as usize).context("submesh out of range")?.push(t as u32); }
    let names: Vec<String> = risen_formats::xmsh::parse_submeshes(src).into_iter().map(|s| s.material).collect();
    if let Some(k) = groups.iter().position(|g| g.is_empty()) { bail!("this edit removes every triangle of ground material {}; keep at least one", names.get(k).map(String::as_str).unwrap_or("?")); }
    let centre = |t: u32| -> [f32; 2] { let p = fc[t as usize].map(pos); [(p[0][0] + p[1][0] + p[2][0]) / 3.0, (p[0][2] + p[1][2] + p[2][2]) / 3.0] };
    let (mut vbuf, mut ibuf) = (Vec::with_capacity(records.len()), Vec::with_capacity(fc.len() * 12));
    let (mut out_rec, mut out_tri) = (vec![], vec![]);
    let (mut fv, mut fi) = (0u32, 0u32);
    let (mut blo, mut bhi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (k, group) in groups.iter_mut().enumerate() {
        let mut nodes: Vec<Node> = vec![];
        build_tree(group, &centre, 0, &mut nodes);
        let mut local: HashMap<u32, u32> = HashMap::new();
        let mut n_local = 0u32;
        for &t in group.iter() {
            out_tri.push(t);
            for c in 0..3 {
                let r = fc[t as usize][c];
                let ni = *local.entry(r).or_insert_with(|| {
                    vbuf.extend_from_slice(&records[r as usize * stride..(r as usize + 1) * stride]);
                    out_rec.push(r);
                    n_local += 1;
                    n_local - 1
                });
                ibuf.extend((fv + ni).to_le_bytes());
            }
        }
        let tri_box = |a: usize, n: usize| -> (V3, V3) {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for &t in &group[a..a + n] { for c in 0..3 { let p = pos(fc[t as usize][c]); for x in 0..3 { lo[x] = lo[x].min(p[x]); hi[x] = hi[x].max(p[x]); } } }
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
        let e = &mut elems[k];
        let ext = e.props.iter_mut().find(|p| p.name == "Extends").context("submesh without Extends")?;
        ext.value = Value::Raw(bbox_bytes(lo, hi));
        long_prop(e, "FirstIndex", fi)?;
        long_prop(e, "IndexCount", group.len() as u32 * 3)?;
        long_prop(e, "FirstVertex", fv)?;
        long_prop(e, "VertexCount", n_local)?;
        ensure!(e.rest.len() >= 6 && e.rest[..2] == [201, 0], "submesh culling tree does not start with 201");
        e.rest = tree;
        fv += n_local;
        fi += group.len() as u32 * 3;
    }
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
    data[I_HEADER..I_HEADER + 4].copy_from_slice(&(ibuf.len() as u32).to_le_bytes());
    data.extend(vbuf);
    data.extend(ibuf);
    data.extend(trailer);
    res.data = data;
    Ok((res.write(), out_rec, out_tri))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_round_trips() {
        let s = Spec { archive_vertices: 10, archive_triangles: 8, ops: vec![Op { kind: 1, tris: vec![1, 2] }, Op { kind: 2, tris: vec![7] }], result_vertices: 14, result_triangles: 13, check: 0xdead_beef_0123 };
        assert_eq!(read(&write(&s)).unwrap(), s);
        assert!(read(&write(&s)[..20]).is_err());
    }

    #[test]
    fn patterns_keep_the_winding() {
        // Corners and midpoints of a counter-clockwise triangle in XZ: every child has the same sign.
        let p: [P2; 6] = [[0.0, 0.0], [4.0, 0.0], [0.0, 4.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]];
        let area = |t: [usize; 3]| { let (a, b, c) = (p[t[0]], p[t[1]], p[t[2]]); (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) };
        for ab in [false, true] { for bc in [false, true] { for ca in [false, true] {
            if !(ab || bc || ca) { continue; }
            let kids = pattern(ab, bc, ca);
            let total: f32 = kids.iter().map(|&k| area(k)).sum();
            assert!(kids.iter().all(|&k| area(k) > 0.0), "{ab} {bc} {ca}");
            assert_eq!(total, area([0, 1, 2]));
        } } }
    }
}
