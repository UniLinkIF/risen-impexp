//! Risen 1 collision meshes: the `._xcom` container and the PhysX 2.8 cooked triangle-mesh stream
//! inside it — read and written by us, so collision for new models needs no PhysX SDK.
//!
//! ```text
//! ._xcom   GR01CM00 · eCCollisionMeshResource2 {IsConvex, Boundary, SubBoundaries: one box per mesh}
//!          data: u32 n · n triangle-mesh streams · u32 m · m convex streams (none seen)
//!                · u32 k · k × (u8 shape material, u8 ignored-by-trace-ray, u8 no collision, u8 no response)
//! stream   "NXS\x01" "MESH" u32 1 · u32 flags (1 materials, 2 face remap, 8 / 16 = 8- / 16-bit indices)
//!          · f32 convex edge threshold (0.001) · u32 0xFF (no height field) · f32 0
//!          · u32 nv · u32 nt · f32 vertex[3·nv] (metres, game axes) · indices[3·nt]
//!          · [u16 material[nt]] · [u32 max · remap[nt] (u8/u16/u32 by max): new → original triangle]
//!          · u32 convex parts · u32 flat parts · [u16 convex part[nt]] · [flat part[nt]: u8 if < 256 parts, else u16]
//!          · u32 model bytes · collision tree (below)
//!          · f32 geometric epsilon · f32 sphere[4] (centre, radius) · f32 AABB[6] (min, max)
//!          · f32 mass (volume, density 1) · f32 inertia[9] · f32 centre of mass[3]
//!          · u32 nt · u8 edge flags[nt]
//! model    "OPC\x01" u32 1 · u32 code (4 = single leaf, 3 = quantized no-leaf tree)
//!          · [u32 nodes · nodes × 20 bytes · f32 centre coeff[3] · f32 extents coeff[3]]
//!          · "HBM\x01" u32 0 · u32 leaves · [u32 max · leaf[leaves] (u8/u16/u32 by max)] · u32 0
//! node     i16 centre[3] · u16 extents[3] (× the coefficients) · u32 a · u32 b
//!          a = 0xDEAD: the first child is the next node; else bit 31: the first child is leaf
//!          (a & 0x3FFFFFFF), bit 30: the second child is the leaf after it.
//!          b = number of nodes below this one (so the second child node sits at i + 1 + size of the first).
//! leaf     triangle index << 4 | (count − 1); leaves cover the (remapped) triangles in order.
//! ```
//! Worked out on the shipped files; every stream in the game re-writes byte for byte (see tests).

use crate::gr01::{Reader, Resource};
use anyhow::{bail, ensure, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct Node { pub center: [i16; 3], pub extents: [u16; 3], pub a: u32, pub b: u32 }

#[derive(Debug, Clone, PartialEq)]
pub struct Model { pub code: u32, pub nodes: Vec<Node>, pub coeffs: [f32; 6], pub leaves: Vec<u32> }

#[derive(Debug, Clone, PartialEq)]
pub struct TriMesh {
    pub flags: u32,
    pub edge_threshold: f32,
    pub verts: Vec<[f32; 3]>,
    pub tris: Vec<[u32; 3]>,
    pub materials: Option<Vec<u16>>,
    pub remap: Option<Vec<u32>>,
    pub convex_parts: u32,
    pub flat_parts: u32,
    pub convex_part: Vec<u16>,
    pub flat_part: Vec<u16>,
    pub model: Model,
    pub geom_epsilon: f32,
    pub sphere: [f32; 4],
    pub aabb: [f32; 6],
    pub mass: f32,
    pub inertia: [f32; 9],
    pub com: [f32; 3],
    pub edge_flags: Vec<u8>,
}

pub const F_MATERIALS: u32 = 1;
pub const F_REMAP: u32 = 2;
pub const F_8BIT: u32 = 8;
pub const F_16BIT: u32 = 16;

fn f32r(r: &mut Reader) -> Result<f32> { Ok(f32::from_bits(r.u32()?)) }
fn arr<const N: usize>(r: &mut Reader) -> Result<[f32; N]> { let mut a = [0f32; N]; for x in &mut a { *x = f32r(r)?; } Ok(a) }

fn width_for(max: u32) -> usize { if max < 256 { 1 } else if max < 65536 { 2 } else { 4 } }
fn read_uint(r: &mut Reader, w: usize) -> Result<u32> { Ok(match w { 1 => r.u8()? as u32, 2 => r.u16()? as u32, _ => r.u32()? }) }
fn put_uint(o: &mut Vec<u8>, w: usize, v: u32) { match w { 1 => o.push(v as u8), 2 => o.extend((v as u16).to_le_bytes()), _ => o.extend(v.to_le_bytes()) } }

/// `u32 max` + values sized by max (remap tables, leaf lists).
fn read_packed(r: &mut Reader, n: usize) -> Result<Vec<u32>> {
    let max = r.u32()?;
    let w = width_for(max);
    (0..n).map(|_| read_uint(r, w)).collect()
}
fn put_packed(o: &mut Vec<u8>, v: &[u32]) {
    let max = v.iter().copied().max().unwrap_or(0);
    o.extend(max.to_le_bytes());
    let w = width_for(max);
    for &x in v { put_uint(o, w, x); }
}

fn read_model(r: &mut Reader) -> Result<Model> {
    ensure!(r.bytes(4)? == b"OPC\x01", "no OPC header: offset 0x{:x}", r.at - 4);
    ensure!(r.u32()? == 1, "OPC version");
    let code = r.u32()?;
    let (mut nodes, mut coeffs) = (vec![], [0f32; 6]);
    if code != 4 {
        let n = r.u32()? as usize;
        for _ in 0..n {
            let c = [r.u16()? as i16, r.u16()? as i16, r.u16()? as i16];
            let e = [r.u16()?, r.u16()?, r.u16()?];
            nodes.push(Node { center: c, extents: e, a: r.u32()?, b: r.u32()? });
        }
        coeffs = arr(r)?;
    }
    ensure!(r.bytes(4)? == b"HBM\x01", "no HBM header: offset 0x{:x}", r.at - 4);
    ensure!(r.u32()? == 0, "HBM version");
    let nl = r.u32()? as usize;
    let leaves = if nl > 1 { read_packed(r, nl)? } else { vec![0; nl] };
    ensure!(r.u32()? == 0, "HBM primitive table is not empty: offset 0x{:x}", r.at - 4);
    Ok(Model { code, nodes, coeffs, leaves })
}

fn write_model(m: &Model) -> Vec<u8> {
    let mut o = b"OPC\x01".to_vec();
    o.extend(1u32.to_le_bytes());
    o.extend(m.code.to_le_bytes());
    if m.code != 4 {
        o.extend((m.nodes.len() as u32).to_le_bytes());
        for n in &m.nodes {
            for c in n.center { o.extend(c.to_le_bytes()); }
            for e in n.extents { o.extend(e.to_le_bytes()); }
            o.extend(n.a.to_le_bytes());
            o.extend(n.b.to_le_bytes());
        }
        for c in m.coeffs { o.extend(c.to_le_bytes()); }
    }
    o.extend(b"HBM\x01");
    o.extend(0u32.to_le_bytes());
    o.extend((m.leaves.len() as u32).to_le_bytes());
    if m.leaves.len() > 1 { put_packed(&mut o, &m.leaves); }
    o.extend(0u32.to_le_bytes());
    o
}

pub fn read_trimesh(r: &mut Reader) -> Result<TriMesh> {
    let at = r.at;
    ensure!(r.bytes(8)? == b"NXS\x01MESH", "no NXS MESH header: offset 0x{at:x}");
    ensure!(r.u32()? == 1, "NXS version");
    let flags = r.u32()?;
    let edge_threshold = f32r(r)?;
    ensure!(r.u32()? == 0xff && f32r(r)? == 0.0, "height-field meshes are not supported");
    let (nv, nt) = (r.u32()? as usize, r.u32()? as usize);
    let verts = (0..nv).map(|_| arr::<3>(r)).collect::<Result<Vec<_>>>()?;
    let w = if flags & F_8BIT != 0 { 1 } else if flags & F_16BIT != 0 { 2 } else { 4 };
    let tris = (0..nt).map(|_| Ok([read_uint(r, w)?, read_uint(r, w)?, read_uint(r, w)?])).collect::<Result<Vec<_>>>()?;
    let materials = if flags & F_MATERIALS != 0 { Some((0..nt).map(|_| r.u16()).collect::<Result<Vec<_>>>()?) } else { None };
    let remap = if flags & F_REMAP != 0 { Some(read_packed(r, nt)?) } else { None };
    let (convex_parts, flat_parts) = (r.u32()?, r.u32()?);
    let convex_part = if convex_parts > 0 { (0..nt).map(|_| r.u16()).collect::<Result<Vec<_>>>()? } else { vec![] };
    let flat_part = if flat_parts > 0 { let fw = if flat_parts < 256 { 1 } else { 2 }; (0..nt).map(|_| Ok(read_uint(r, fw)? as u16)).collect::<Result<Vec<_>>>()? } else { vec![] };
    let model_bytes = r.u32()? as usize;
    let m_at = r.at;
    let model = read_model(r)?;
    ensure!(r.at - m_at == model_bytes, "model is {} bytes, header says {model_bytes}", r.at - m_at);
    let geom_epsilon = f32r(r)?;
    let sphere = arr(r)?;
    let aabb = arr(r)?;
    let mass = f32r(r)?;
    let inertia = arr(r)?;
    let com = arr(r)?;
    let ne = r.u32()? as usize;
    let edge_flags = r.bytes(ne)?.to_vec();
    Ok(TriMesh { flags, edge_threshold, verts, tris, materials, remap, convex_parts, flat_parts, convex_part, flat_part, model, geom_epsilon, sphere, aabb, mass, inertia, com, edge_flags })
}

pub fn write_trimesh(m: &TriMesh) -> Vec<u8> {
    let mut o = b"NXS\x01MESH".to_vec();
    let u = |o: &mut Vec<u8>, v: u32| o.extend(v.to_le_bytes());
    let f = |o: &mut Vec<u8>, v: f32| o.extend(v.to_le_bytes());
    u(&mut o, 1);
    u(&mut o, m.flags);
    f(&mut o, m.edge_threshold);
    u(&mut o, 0xff);
    f(&mut o, 0.0);
    u(&mut o, m.verts.len() as u32);
    u(&mut o, m.tris.len() as u32);
    for v in &m.verts { for &x in v { f(&mut o, x); } }
    let w = if m.flags & F_8BIT != 0 { 1 } else if m.flags & F_16BIT != 0 { 2 } else { 4 };
    for t in &m.tris { for &i in t { put_uint(&mut o, w, i); } }
    if let Some(ms) = &m.materials { for &x in ms { o.extend(x.to_le_bytes()); } }
    if let Some(r) = &m.remap { put_packed(&mut o, r); }
    u(&mut o, m.convex_parts);
    u(&mut o, m.flat_parts);
    if m.convex_parts > 0 { for &x in &m.convex_part { o.extend(x.to_le_bytes()); } }
    if m.flat_parts > 0 { let fw = if m.flat_parts < 256 { 1 } else { 2 }; for &x in &m.flat_part { put_uint(&mut o, fw, x as u32); } }
    let model = write_model(&m.model);
    u(&mut o, model.len() as u32);
    o.extend(model);
    f(&mut o, m.geom_epsilon);
    for x in m.sphere.iter().chain(&m.aabb) { f(&mut o, *x); }
    f(&mut o, m.mass);
    for x in m.inertia.iter().chain(&m.com) { f(&mut o, *x); }
    u(&mut o, m.edge_flags.len() as u32);
    o.extend(&m.edge_flags);
    o
}

/// Shape material and flags of one collision mesh (Risen's `eEShapeMaterial` order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapeMaterial { pub material: u8, pub ignored_by_trace_ray: u8, pub no_collision: u8, pub no_response: u8 }

pub const SHAPE_MATERIALS: [&str; 23] = ["none", "wood", "metal", "water", "stone", "earth", "ice", "leather", "clay", "glass", "flesh", "snow", "debris", "foliage", "magic", "grass", "springanddamper1", "springanddamper2", "springanddamper3", "damage", "sand", "movement", "axe"];

#[derive(Debug, Clone, PartialEq)]
pub struct Xcom { pub resource: Resource, pub meshes: Vec<TriMesh>, pub shape_materials: Vec<ShapeMaterial> }

pub fn read_xcom(d: &[u8]) -> Result<Xcom> {
    let resource = Resource::parse(d)?;
    ensure!(&resource.magic == b"GR01CM00", "not a GR01CM00 collision mesh");
    let data = &resource.data;
    let mut r = Reader { d: data, at: 0 };
    let n = r.u32()?;
    let mut meshes = vec![];
    for _ in 0..n { meshes.push(read_trimesh(&mut r)?); }
    ensure!(r.u32()? == 0, "convex collision meshes are not supported");
    let k = r.u32()?;
    let mut shape_materials = vec![];
    for _ in 0..k { let b = r.bytes(4)?; shape_materials.push(ShapeMaterial { material: b[0], ignored_by_trace_ray: b[1], no_collision: b[2], no_response: b[3] }); }
    if r.at != data.len() { bail!("{} bytes after the shape materials", data.len() - r.at); }
    Ok(Xcom { resource, meshes, shape_materials })
}

pub fn write_xcom(x: &Xcom) -> Vec<u8> {
    let mut data = (x.meshes.len() as u32).to_le_bytes().to_vec();
    for m in &x.meshes { data.extend(write_trimesh(m)); }
    data.extend(0u32.to_le_bytes());
    data.extend((x.shape_materials.len() as u32).to_le_bytes());
    for s in &x.shape_materials { data.extend([s.material, s.ignored_by_trace_ray, s.no_collision, s.no_response]); }
    let mut res = x.resource.clone();
    res.data = data;
    res.write()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every collision file in the game: parse → write gives back the same bytes.
    #[test]
    fn every_shipped_collision_mesh_round_trips() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        let (mut files, mut streams, mut bad) = (0, 0, vec![]);
        for e in g.entries_with_suffix("._xcom") {
            let d = g.read(&e).unwrap().0;
            if e.to_lowercase().ends_with("_cv._xcom") { continue; } // convex (2 files): not written by us
            match read_xcom(&d) {
                Ok(x) => {
                    streams += x.meshes.len();
                    let w = write_xcom(&x);
                    if w == d { files += 1 } else { let at = w.iter().zip(&d).position(|(a, b)| a != b).unwrap_or(w.len().min(d.len())); bad.push(format!("{e}: differs at 0x{at:x}")) }
                }
                Err(err) => bad.push(format!("{e}: {err:#}")),
            }
        }
        eprintln!("{files} files ({streams} streams) identical; {} not: {:?}", bad.len(), &bad[..bad.len().min(8)]);
        assert!(bad.is_empty());
    }
}








/// Mass properties of a closed (or not) triangle mesh at density 1 — Eberly's "Polyhedral Mass
/// Properties" (Mirtich's volume integrals): (volume, centre of mass, inertia about the centre).
pub fn mass_properties(verts: &[[f32; 3]], tris: &[[u32; 3]]) -> (f64, [f64; 3], [f64; 9]) {
    let mult = [1.0 / 6.0, 1.0 / 24.0, 1.0 / 24.0, 1.0 / 24.0, 1.0 / 60.0, 1.0 / 60.0, 1.0 / 60.0, 1.0 / 120.0, 1.0 / 120.0, 1.0 / 120.0];
    let mut intg = [0f64; 10];
    let sub = |w0: f64, w1: f64, w2: f64| {
        let (t0, t1) = (w0 + w1, w0 * w0);
        let t2 = t1 + w1 * t0;
        let f1 = t0 + w2;
        let f2 = t2 + w2 * f1;
        let f3 = w0 * t1 + w1 * t2 + w2 * f2;
        let g0 = f2 + w0 * (f1 + w0);
        let g1 = f2 + w1 * (f1 + w1);
        let g2 = f2 + w2 * (f1 + w2);
        (f1, f2, f3, g0, g1, g2)
    };
    for t in tris {
        let [p0, p1, p2] = t.map(|i| verts[i as usize].map(|x| x as f64));
        let (a1, b1, c1) = (p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]);
        let (a2, b2, c2) = (p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]);
        let (d0, d1, d2) = (b1 * c2 - b2 * c1, a2 * c1 - a1 * c2, a1 * b2 - a2 * b1);
        let (f1x, f2x, f3x, g0x, g1x, g2x) = sub(p0[0], p1[0], p2[0]);
        let (_f1y, f2y, f3y, g0y, g1y, g2y) = sub(p0[1], p1[1], p2[1]);
        let (_f1z, f2z, f3z, g0z, g1z, g2z) = sub(p0[2], p1[2], p2[2]);
        intg[0] += d0 * f1x;
        intg[1] += d0 * f2x; intg[2] += d1 * f2y; intg[3] += d2 * f2z;
        intg[4] += d0 * f3x; intg[5] += d1 * f3y; intg[6] += d2 * f3z;
        intg[7] += d0 * (p0[1] * g0x + p1[1] * g1x + p2[1] * g2x);
        intg[8] += d1 * (p0[2] * g0y + p1[2] * g1y + p2[2] * g2y);
        intg[9] += d2 * (p0[0] * g0z + p1[0] * g1z + p2[0] * g2z);
    }
    for i in 0..10 { intg[i] *= mult[i]; }
    let mass = intg[0];
    let cm = if mass != 0.0 { [intg[1] / mass, intg[2] / mass, intg[3] / mass] } else { [0.0; 3] };
    let ixx = intg[5] + intg[6] - mass * (cm[1] * cm[1] + cm[2] * cm[2]);
    let iyy = intg[4] + intg[6] - mass * (cm[2] * cm[2] + cm[0] * cm[0]);
    let izz = intg[4] + intg[5] - mass * (cm[0] * cm[0] + cm[1] * cm[1]);
    let ixy = -(intg[7] - mass * cm[0] * cm[1]);
    let iyz = -(intg[8] - mass * cm[1] * cm[2]);
    let ixz = -(intg[9] - mass * cm[2] * cm[0]);
    (mass, cm, [ixx, ixy, ixz, ixy, iyy, iyz, ixz, iyz, izz])
}

