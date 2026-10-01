//! Morph targets of an actor (XAC section 12): the faces' blend shapes (blink, smile, the mouth
//! shapes of lip-sync), as position deltas per vertex.
//!
//! ```text
//! section 12 v1   u32 targets · u32 lod · then per target:
//!   f32 range min · f32 range max · u32 lod · u32 meshes · u32 transforms · u32 phoneme sets
//!   · u32 length · name
//!   · meshes: u32 node · f32 min · f32 max · u32 n · u16[3][n] position (min..max)
//!             · u8[3][n] normal · u8[3][n] tangent (not in every file) · u32[n] vertex (the
//!             mesh's own vertex, after the uv-seam split)
//!   · transforms: u32 node · f32 rotation[4] · f32 scale rotation[4] · f32 position[3] · f32 scale[3]
//! ```
//! Both halves of a vertex split at a uv seam are listed, with the same delta.

use crate::xmac_write::{Actor, Section};
use anyhow::{ensure, Context, Result};

/// Section 12 as read: `tangents` = each delta carries a second u8[3] block (most heads do).
#[derive(Debug, Clone, PartialEq)]
pub struct Morphs { pub lod: u32, pub tangents: bool, pub targets: Vec<Target> }

#[derive(Debug, Clone, PartialEq)]
pub struct Target { pub name: String, pub range: (f32, f32), pub lod: u32, pub phonemes: u32, pub meshes: Vec<MeshDeltas>, pub transforms: Vec<u8> }

#[derive(Debug, Clone, PartialEq)]
pub struct MeshDeltas { pub node: u32, pub deltas: Vec<(u32, [f32; 3])> }

struct R<'a> { d: &'a [u8], at: usize }
impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> { let b = self.d.get(self.at..self.at + n).with_context(|| format!("morph targets run short at 0x{:x}", self.at))?; self.at += n; Ok(b) }
    fn u32(&mut self) -> Result<u32> { Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
    fn f32(&mut self) -> Result<f32> { Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
}

pub fn read(body: &[u8]) -> Result<Morphs> {
    read_with(body, false).or_else(|_| read_with(body, true))
}

fn read_with(body: &[u8], tangents: bool) -> Result<Morphs> {
    let mut r = R { d: body, at: 0 };
    let n = r.u32()?;
    let lod = r.u32()?;
    let mut targets = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let range = (r.f32()?, r.f32()?);
        let lod = r.u32()?;
        let (nm, nt, phonemes) = (r.u32()?, r.u32()?, r.u32()?);
        let len = r.u32()? as usize;
        let name = crate::gr01::latin1(r.take(len)?);
        let mut meshes = vec![];
        for _ in 0..nm {
            let node = r.u32()?;
            let (lo, hi) = (r.f32()?, r.f32()?);
            let nv = r.u32()? as usize;
            let packed = r.take(nv * 6)?;
            r.take(nv * if tangents { 6 } else { 3 })?;
            let verts = r.take(nv * 4)?;
            let deltas = (0..nv).map(|i| {
                let c = |k: usize| lo + (hi - lo) * u16::from_le_bytes([packed[i * 6 + k * 2], packed[i * 6 + k * 2 + 1]]) as f32 / 65535.0;
                (u32::from_le_bytes(verts[i * 4..i * 4 + 4].try_into().unwrap()), [c(0), c(1), c(2)])
            }).collect();
            meshes.push(MeshDeltas { node, deltas });
        }
        let transforms = r.take(nt as usize * 60)?.to_vec();
        targets.push(Target { name, range, lod, phonemes, meshes, transforms });
    }
    ensure!(r.at == body.len(), "morph targets end at 0x{:x} of 0x{:x}", r.at, body.len());
    Ok(Morphs { lod, tangents, targets })
}

/// Section 12's body. Normal (and tangent) deltas are written as the game's own heads have them:
/// all 126, no change.
pub fn write(m: &Morphs) -> Vec<u8> {
    let mut o = vec![];
    let u = |o: &mut Vec<u8>, x: u32| o.extend_from_slice(&x.to_le_bytes());
    let f = |o: &mut Vec<u8>, x: f32| o.extend_from_slice(&x.to_le_bytes());
    u(&mut o, m.targets.len() as u32);
    u(&mut o, m.lod);
    for t in &m.targets {
        f(&mut o, t.range.0); f(&mut o, t.range.1);
        u(&mut o, t.lod);
        u(&mut o, t.meshes.len() as u32);
        u(&mut o, (t.transforms.len() / 60) as u32);
        u(&mut o, t.phonemes);
        let name = crate::gr01::to_latin1(&t.name);
        u(&mut o, name.len() as u32);
        o.extend_from_slice(&name);
        for md in &t.meshes {
            let (lo, hi) = md.deltas.iter().flat_map(|d| d.1).fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
            let (lo, hi) = if md.deltas.is_empty() { (0.0, 0.0) } else { (lo, hi) };
            u(&mut o, md.node); f(&mut o, lo); f(&mut o, hi);
            u(&mut o, md.deltas.len() as u32);
            for (_, d) in &md.deltas {
                for x in d {
                    let q = if hi > lo { ((x - lo) / (hi - lo) * 65535.0).round().clamp(0.0, 65535.0) as u16 } else { 0 };
                    o.extend_from_slice(&q.to_le_bytes());
                }
            }
            o.resize(o.len() + md.deltas.len() * if m.tangents { 6 } else { 3 }, 126);
            for (v, _) in &md.deltas { u(&mut o, *v); }
        }
        o.extend_from_slice(&t.transforms);
    }
    o
}

/// The actor's morph targets as full delta arrays over its skinned-mesh vertices (in the order
/// `xmesh_skin` reads them: every mesh with uvs, its vertices in file order).
pub fn per_vertex(actor: &Actor) -> Result<Vec<(String, Vec<[f32; 3]>)>> {
    let Some(body) = actor.sections.iter().find_map(|s| match s { Section::Raw { id: 12, body, .. } => Some(body), _ => None }) else { return Ok(vec![]) };
    let targets = read(body)?.targets;
    // Vertex range of each mesh, by node.
    let mut meshes = vec![];
    let mut base = 0usize;
    for s in &actor.sections {
        let Section::Mesh(m) = s else { continue };
        if !m.layers.iter().any(|l| l.kind == 3) { continue; }
        let n = m.raw_vertices as usize;
        meshes.push((m.node, base, n));
        base += n;
    }
    let mut out = vec![];
    for t in targets {
        let mut d = vec![[0f32; 3]; base];
        for md in &t.meshes {
            let Some((_, start, n)) = meshes.iter().find(|(node, _, _)| *node == md.node) else { continue };
            for (v, x) in &md.deltas {
                ensure!((*v as usize) < *n, "morph {} moves vertex {v} of {n}", t.name);
                d[start + *v as usize] = *x;
            }
        }
        if d.iter().any(|v| v.iter().any(|x| x.abs() > 1e-6)) { out.push((t.name, d)); }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn every_actor_reads() {
        let g = crate::game::test_game().unwrap();
        let (mut actors, mut with, mut targets) = (0, 0, 0);
        let mut names = std::collections::BTreeMap::<String, usize>::new();
        for e in g.entries_with_suffix("._xmac") {
            let Ok(a) = crate::xmac_write::read(&g.read(&e).unwrap().0) else { continue };
            actors += 1;
            let m = super::per_vertex(&a).unwrap_or_else(|err| panic!("{e}: {err:#}"));
            if !m.is_empty() { with += 1; targets += m.len(); }
            for (n, _) in &m { *names.entry(n.clone()).or_default() += 1; }
        }
        eprintln!("{actors} actors, {with} with morphs, {targets} targets; {names:?}");
    }
}

#[cfg(test)]
mod write_tests {
    /// Every head's face shapes, written again, read back to the same shapes (within the 16-bit
    /// steps), the same size, and the normal blocks as the game has them.
    #[test]
    #[ignore]
    fn heads_write_back() {
        let g = crate::game::test_game().unwrap();
        let mut n = 0;
        for e in g.entries_with_suffix("._xmac") {
            let Ok(a) = crate::xmac_write::read(&g.read(&e).unwrap().0) else { continue };
            let Some(body) = a.sections.iter().find_map(|s| match s { crate::xmac_write::Section::Raw { id: 12, body, .. } => Some(body.clone()), _ => None }) else { continue };
            let m = super::read(&body).unwrap();
            let w = super::write(&m);
            assert_eq!(w.len(), body.len(), "{e}");
            let back = super::read(&w).unwrap();
            for (t0, t1) in m.targets.iter().zip(&back.targets) {
                assert_eq!((&t0.name, t0.phonemes, t0.range), (&t1.name, t1.phonemes, t1.range));
                for (a, b) in t0.meshes.iter().zip(&t1.meshes) {
                    for (x, y) in a.deltas.iter().zip(&b.deltas) {
                        assert_eq!(x.0, y.0);
                        assert!((0..3).all(|k| (x.1[k] - y.1[k]).abs() < 1e-4), "{e} {}: {:?} vs {:?}", t0.name, x.1, y.1);
                    }
                }
            }
            n += 1;
        }
        assert!(n >= 12, "{n} heads");
    }
}
