//! Risen 1 actors (`._xmac`): the EMotionFX "XAC " stream inside a `GR01MA02` resource, read and
//! written section by section so a new skinned mesh can replace an actor's mesh on its skeleton
//! (armour, clothes, a reshaped creature).
//!
//! ```text
//! resource  eCMotionActorResource2 { Boundary: bCBox } · data: u32 XAC bytes after offset 140
//!           · "XAC " u8 hi lo endian pad · sections to that end · trailing bytes
//! section   u32 id · u32 size · u32 version · body (size bytes)
//!   7  scene info, 11 nodes (skeleton), 12 morph targets              ← kept as they are
//!   13 materials: u32 n, u32 n, u32 0 — then n material sections (id 3) follow it
//!   3  material: f32 ambient[4] diffuse[4] specular[4] emissive[4] · f32 shine, shine strength,
//!      opacity, ior · u16 · u8 transparency type · u8 maps · u32 name length · name
//!      — then its maps, NOT counted in the section size: f32 amount, u offset, v offset,
//!        u tiling, v tiling, rotation · u16 material · u8 map type (2 diffuse, 3 specular,
//!        5 normal) · u8 · u32 length · texture name (no extension)
//!   1  mesh: u32 node · u32 final vertices · u32 raw vertices · u32 indices · u32 submeshes
//!      · u32 layers · u32 flags · layers: (u32 type, u32 bytes per vertex, u16 keep, u16 tag,
//!      data[raw]) with 5 base-vertex (u32), 0 positions, 1 normals (f32×3), 3 uv (f32×2),
//!      2 tangents (f32×4) · submeshes: u32 indices, u32 vertices, u32 material, u32 bones,
//!      u32 index[indices] (from the submesh's first vertex), u32 node[bones]
//!   2  skin: u32 node · u32 local bones · u32 influences · u32 skin index ·
//!      (f32 weight, u16 node, u16 tag)[influences] · (u32 first, u32 count)[final vertices]
//! ```
//! Every little-endian actor in the game re-writes byte for byte (see the tests).

use crate::gr01::Resource;
use anyhow::{ensure, Context, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct Map { pub params: [f32; 6], pub material: u16, pub kind: u8, pub flag: u8, pub texture: String }

#[derive(Debug, Clone, PartialEq)]
pub struct Material { pub version: u32, pub colors: [f32; 20], pub u16a: u16, pub transparency: u8, pub name: String, pub maps: Vec<Map> }

#[derive(Debug, Clone, PartialEq)]
pub struct Layer { pub kind: u32, pub size: u32, pub keep: u16, pub tag: u16, pub data: Vec<u8> }

#[derive(Debug, Clone, PartialEq)]
pub struct SubMesh { pub vertices: u32, pub material: u32, pub indices: Vec<u32>, pub bones: Vec<u32> }

#[derive(Debug, Clone, PartialEq)]
pub struct Mesh { pub version: u32, pub node: u32, pub final_vertices: u32, pub raw_vertices: u32, pub flags: u32, pub layers: Vec<Layer>, pub subs: Vec<SubMesh> }

#[derive(Debug, Clone, PartialEq)]
pub struct Skin { pub version: u32, pub node: u32, pub local_bones: u32, pub skin_index: u32, pub influences: Vec<(f32, u16, u16)>, pub ranges: Vec<(u32, u32)> }

#[derive(Debug, Clone, PartialEq)]
pub enum Section { Raw { id: u32, version: u32, body: Vec<u8> }, Materials { version: u32, counts: [u32; 3], list: Vec<Material> }, Mesh(Mesh), Skin(Skin) }

#[derive(Debug, Clone, PartialEq)]
pub struct Actor { pub resource: Resource, pub head: [u8; 8], pub sections: Vec<Section>, pub trailing: Vec<u8> }

struct R<'a> { d: &'a [u8], at: usize }
impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> { let b = self.d.get(self.at..self.at + n).with_context(|| format!("actor runs short at 0x{:x}", self.at))?; self.at += n; Ok(b) }
    fn u8(&mut self) -> Result<u8> { Ok(self.take(1)?[0]) }
    fn u16(&mut self) -> Result<u16> { Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap())) }
    fn u32(&mut self) -> Result<u32> { Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
    fn f32(&mut self) -> Result<f32> { Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
    fn str(&mut self) -> Result<String> { let n = self.u32()? as usize; Ok(crate::gr01::latin1(self.take(n)?)) }
    fn u32s(&mut self, n: usize) -> Result<Vec<u32>> { (0..n).map(|_| self.u32()).collect() }
}

fn put_str(o: &mut Vec<u8>, s: &str) { let b = crate::gr01::to_latin1(s); o.extend((b.len() as u32).to_le_bytes()); o.extend(b); }

pub fn read(d: &[u8]) -> Result<Actor> {
    let resource = Resource::parse(d)?;
    ensure!(&resource.magic == b"GR01MA02", "not an actor (GR01MA02)");
    let data = &resource.data;
    ensure!(data.len() >= 12 && &data[4..8] == b"XAC ", "no XAC stream");
    ensure!(data[10] == 0, "big-endian actor (console build) — not supported");
    let end = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize + 4;
    ensure!(end <= data.len(), "XAC size past the end");
    let head: [u8; 8] = data[4..12].try_into().unwrap();
    let mut r = R { d: data, at: 12 };
    let mut sections = vec![];
    while r.at < end {
        let (id, size, version) = (r.u32()?, r.u32()? as usize, r.u32()?);
        let start = r.at;
        match id {
            13 => {
                let counts = [r.u32()?, r.u32()?, r.u32()?];
                ensure!(r.at - start == size, "materials header is {} bytes, section says {size}", r.at - start);
                let mut list = vec![];
                for _ in 0..counts[0] {
                    ensure!(r.u32()? == 3, "material section expected after materials");
                    let msize = r.u32()? as usize;
                    let mversion = r.u32()?;
                    let mstart = r.at;
                    let mut colors = [0f32; 20];
                    for c in &mut colors { *c = r.f32()?; }
                    let u16a = r.u16()?;
                    let transparency = r.u8()?;
                    let nmaps = r.u8()?;
                    let name = r.str()?;
                    ensure!(r.at - mstart == msize, "material {name} is {} bytes, section says {msize}", r.at - mstart);
                    let mut maps = vec![];
                    for _ in 0..nmaps {
                        let mut params = [0f32; 6];
                        for p in &mut params { *p = r.f32()?; }
                        let (material, kind, flag) = (r.u16()?, r.u8()?, r.u8()?);
                        maps.push(Map { params, material, kind, flag, texture: r.str()? });
                    }
                    list.push(Material { version: mversion, colors, u16a, transparency, name, maps });
                }
                sections.push(Section::Materials { version, counts, list });
            }
            1 => {
                let node = r.u32()?;
                let (final_vertices, raw_vertices, nindices, nsubs, nlayers, flags) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?);
                let mut layers = vec![];
                for _ in 0..nlayers {
                    let (kind, lsize, keep, tag) = (r.u32()?, r.u32()?, r.u16()?, r.u16()?);
                    layers.push(Layer { kind, size: lsize, keep, tag, data: r.take(lsize as usize * raw_vertices as usize)?.to_vec() });
                }
                let mut subs = vec![];
                let mut total = 0;
                for _ in 0..nsubs {
                    let (ni, nv, material, nb) = (r.u32()? as usize, r.u32()?, r.u32()?, r.u32()? as usize);
                    let indices = r.u32s(ni)?;
                    let bones = r.u32s(nb)?;
                    total += ni;
                    subs.push(SubMesh { vertices: nv, material, indices, bones });
                }
                ensure!(total == nindices as usize, "mesh says {nindices} indices, submeshes hold {total}");
                ensure!(r.at - start == size, "mesh section is {} bytes, header says {size}", r.at - start);
                sections.push(Section::Mesh(Mesh { version, node, final_vertices, raw_vertices, flags, layers, subs }));
            }
            2 => {
                let (node, local_bones, n, skin_index) = (r.u32()?, r.u32()?, r.u32()? as usize, r.u32()?);
                let mut influences = Vec::with_capacity(n);
                for _ in 0..n { influences.push((r.f32()?, r.u16()?, r.u16()?)); }
                let rest = size - (r.at - start);
                ensure!(rest % 8 == 0, "skin table is not whole (first, count) pairs");
                let ranges = (0..rest / 8).map(|_| Ok((r.u32()?, r.u32()?))).collect::<Result<Vec<_>>>()?;
                sections.push(Section::Skin(Skin { version, node, local_bones, skin_index, influences, ranges }));
            }
            _ => sections.push(Section::Raw { id, version, body: r.take(size)?.to_vec() }),
        }
    }
    ensure!(r.at == end, "sections end at 0x{:x}, XAC at 0x{end:x}", r.at);
    Ok(Actor { resource: resource.clone(), head, sections, trailing: data[end..].to_vec() })
}

fn section(o: &mut Vec<u8>, id: u32, version: u32, body: &[u8]) { o.extend(id.to_le_bytes()); o.extend((body.len() as u32).to_le_bytes()); o.extend(version.to_le_bytes()); o.extend(body); }

pub fn write(a: &Actor) -> Vec<u8> {
    let mut x = a.head.to_vec();
    for s in &a.sections {
        match s {
            Section::Raw { id, version, body } => section(&mut x, *id, *version, body),
            Section::Materials { version, counts, list } => {
                let mut b = vec![];
                for c in counts { b.extend(c.to_le_bytes()); }
                section(&mut x, 13, *version, &b);
                for m in list {
                    let mut b = vec![];
                    for c in m.colors { b.extend(c.to_le_bytes()); }
                    b.extend(m.u16a.to_le_bytes());
                    b.push(m.transparency);
                    b.push(m.maps.len() as u8);
                    put_str(&mut b, &m.name);
                    section(&mut x, 3, m.version, &b);
                    for mp in &m.maps {
                        for p in mp.params { x.extend(p.to_le_bytes()); }
                        x.extend(mp.material.to_le_bytes());
                        x.push(mp.kind);
                        x.push(mp.flag);
                        put_str(&mut x, &mp.texture);
                    }
                }
            }
            Section::Mesh(m) => {
                let mut b = vec![];
                let nindices: usize = m.subs.iter().map(|s| s.indices.len()).sum();
                for v in [m.node, m.final_vertices, m.raw_vertices, nindices as u32, m.subs.len() as u32, m.layers.len() as u32, m.flags] { b.extend(v.to_le_bytes()); }
                for l in &m.layers { b.extend(l.kind.to_le_bytes()); b.extend(l.size.to_le_bytes()); b.extend(l.keep.to_le_bytes()); b.extend(l.tag.to_le_bytes()); b.extend(&l.data); }
                for s in &m.subs {
                    for v in [s.indices.len() as u32, s.vertices, s.material, s.bones.len() as u32] { b.extend(v.to_le_bytes()); }
                    for i in &s.indices { b.extend(i.to_le_bytes()); }
                    for i in &s.bones { b.extend(i.to_le_bytes()); }
                }
                section(&mut x, 1, m.version, &b);
            }
            Section::Skin(s) => {
                let mut b = vec![];
                for v in [s.node, s.local_bones, s.influences.len() as u32, s.skin_index] { b.extend(v.to_le_bytes()); }
                for (w, n, t) in &s.influences { b.extend(w.to_le_bytes()); b.extend(n.to_le_bytes()); b.extend(t.to_le_bytes()); }
                for (f, c) in &s.ranges { b.extend(f.to_le_bytes()); b.extend(c.to_le_bytes()); }
                section(&mut x, 2, s.version, &b);
            }
        }
    }
    let mut data = ((x.len()) as u32).to_le_bytes().to_vec();
    data.extend(x);
    data.extend(&a.trailing);
    let mut res = a.resource.clone();
    res.data = data;
    res.write()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every little-endian actor in the game: read → write gives back the same bytes.
    #[test]
    fn every_actor_round_trips() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        let (mut ok, mut be, mut bad) = (0, 0, vec![]);
        let mut kinds = std::collections::BTreeMap::new();
        for e in g.entries_with_suffix("._xmac") {
            let d = g.read(&e).unwrap().0;
            match read(&d) {
                Ok(a) => {
                    for s in &a.sections { let k = match s { Section::Raw { id, .. } => format!("raw {id}"), Section::Materials { .. } => "materials".into(), Section::Mesh(m) => format!("mesh layers {:?}", m.layers.iter().map(|l| l.kind).collect::<Vec<_>>()), Section::Skin(_) => "skin".into() }; *kinds.entry(k).or_insert(0) += 1; }
                    let w = write(&a);
                    if w == d { ok += 1 } else { let at = w.iter().zip(&d).position(|(a, b)| a != b).unwrap_or(w.len().min(d.len())); bad.push(format!("{e}: differs at 0x{at:x} ({} vs {})", w.len(), d.len())) }
                }
                Err(err) if err.to_string().contains("big-endian") => be += 1,
                Err(err) => bad.push(format!("{e}: {err:#}")),
            }
        }
        eprintln!("{ok} identical, {be} big-endian skipped, {} bad {:?}\nsections {kinds:?}", bad.len(), &bad[..bad.len().min(6)]);
        assert!(bad.is_empty());
    }
}

