//! Native decoder for the geometry of a Risen 1 `._xmsh` static mesh: vertex and index buffers.
//!
//! `risen_formats::xmsh` already reads the property section (the submesh table). What follows it is a
//! Direct3D 9 style vertex buffer and index buffer, laid out as below. Offsets are relative to the
//! "data offset" stored in the resource header at 0x10 (it points just past the property section's
//! trailing u16 version).
//!
//! ```text
//! +0x000  16 × D3DVERTEXELEMENT9 (8 bytes each: u16 stream, u16 offset, u8 type, u8 method,
//!         u8 usage, u8 usage_index), terminated by type 17 (D3DDECL_END); always 128 bytes
//! +0x080  vertex stream header: u32 byte size, u32, u32, u32, u32 vertex stride
//! +0x094  40 bytes (5 × u64) not needed for geometry
//! +0x0BC  index stream header: u32 byte size, u32, u32 D3DFORMAT (101 = INDEX16, 102 = INDEX32), u32
//! +0x0CC  vertex bytes, then index bytes (triangle lists)
//! ```
//!
//! Positions come out exactly as stored (centimetres, Y up). Checked on every shipped mesh: 1664 of
//! 1666 decode (a SpeedTree billboard has FLOAT4 positions; the UI crosshair has two POSITIONs).

use anyhow::{bail, Result};

/// A decoded static mesh. `normals`/`uvs` are either empty or parallel to `positions`.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshGeometry {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Triangle list, absolute indices into `positions`.
    pub indices: Vec<u32>,
    pub submeshes: Vec<SubRange>,
}

/// One submesh: its material and the slice of `indices` it draws.
#[derive(Debug, Clone, PartialEq)]
pub struct SubRange { pub material: String, pub first_index: u32, pub index_count: u32 }

const DECL_BYTES: usize = 16 * 8;
const VERTEX_HEADER: usize = 20;
const GAP: usize = 40;
const INDEX_HEADER: usize = 16;
// D3DDECLTYPE / D3DDECLUSAGE / D3DFORMAT values used here.
const T_FLOAT2: u8 = 1;
const T_FLOAT3: u8 = 2;
const T_END: u8 = 17;
const U_POSITION: u8 = 0;
const U_NORMAL: u8 = 3;
const U_TEXCOORD: u8 = 5;
const FMT_INDEX16: u32 = 101;
const FMT_INDEX32: u32 = 102;

fn u16_at(d: &[u8], at: usize, what: &str) -> Result<u16> {
    match d.get(at..at + 2) { Some(b) => Ok(u16::from_le_bytes([b[0], b[1]])), None => bail!("{what} runs past end: offset 0x{at:x}") }
}
fn u32_at(d: &[u8], at: usize, what: &str) -> Result<u32> {
    match d.get(at..at + 4) { Some(b) => Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]])), None => bail!("{what} runs past end: offset 0x{at:x}") }
}
fn f32_at(d: &[u8], at: usize) -> f32 { f32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]]) }

/// Where each attribute sits inside one vertex (stream 0 only).
#[derive(Default)]
struct Layout { position: Option<usize>, normal: Option<usize>, uv: Option<usize> }

fn read_declaration(d: &[u8], at: usize) -> Result<Layout> {
    if d.len() < at + DECL_BYTES { bail!("vertex declaration runs past end: offset 0x{at:x}"); }
    let mut l = Layout::default();
    for k in 0..16 {
        let e = at + k * 8;
        let (stream, offset) = (u16_at(d, e, "vertex element")?, u16_at(d, e + 2, "vertex element")? as usize);
        let (ty, method, usage, usage_index) = (d[e + 4], d[e + 5], d[e + 6], d[e + 7]);
        if ty == T_END { return Ok(l); }
        if stream != 0 || method != 0 { continue; }
        match (usage, ty, usage_index) {
            (U_POSITION, T_FLOAT3, 0) => l.position = Some(offset),
            (U_NORMAL, T_FLOAT3, 0) => l.normal = Some(offset),
            (U_TEXCOORD, T_FLOAT2, 0) => l.uv = Some(offset),
            _ => {}
        }
    }
    bail!("vertex declaration has no D3DDECL_END within 16 elements: offset 0x{at:x}")
}

pub fn decode(data: &[u8]) -> Result<MeshGeometry> {
    if !risen_formats::xmsh::is_xmsh(data) { bail!("not a GR01MS02 mesh"); }
    let base = u32_at(data, 0x10, "data offset")? as usize;
    let layout = read_declaration(data, base)?;
    let Some(pos_off) = layout.position else { bail!("vertex declaration has no float3 position: offset 0x{base:x}") };

    let vh = base + DECL_BYTES;
    let vbytes = u32_at(data, vh, "vertex stream size")? as usize;
    let stride = u32_at(data, vh + 16, "vertex stride")? as usize;
    let ih = vh + VERTEX_HEADER + GAP;
    let ibytes = u32_at(data, ih, "index stream size")? as usize;
    let format = u32_at(data, ih + 8, "index format")?;
    let isize = match format { FMT_INDEX16 => 2, FMT_INDEX32 => 4, f => bail!("index format {f} unknown: offset 0x{:x}", ih + 8) };
    if stride == 0 || vbytes % stride != 0 { bail!("vertex stream size {vbytes} is not a multiple of stride {stride}: offset 0x{vh:x}"); }
    for (name, off, w) in [("position", Some(pos_off), 12), ("normal", layout.normal, 12), ("uv", layout.uv, 8)] {
        if let Some(o) = off { if o + w > stride { bail!("{name} at +{o} does not fit a {stride}-byte vertex: offset 0x{base:x}"); } }
    }
    let vstart = ih + INDEX_HEADER;
    let istart = vstart + vbytes;
    if data.len() < istart { bail!("vertex stream runs past end: offset 0x{vstart:x}"); }
    if ibytes % (isize * 3) != 0 { bail!("index stream size {ibytes} is not whole {isize}-byte triangles: offset 0x{ih:x}"); }
    if data.len() < istart + ibytes { bail!("index stream runs past end: offset 0x{istart:x}"); }

    let count = vbytes / stride;
    let vec3 = |o: usize| -> Vec<[f32; 3]> { (0..count).map(|i| { let a = vstart + i * stride + o; [f32_at(data, a), f32_at(data, a + 4), f32_at(data, a + 8)] }).collect() };
    let positions = vec3(pos_off);
    let normals = layout.normal.map(vec3).unwrap_or_default();
    let uvs = layout.uv.map(|o| (0..count).map(|i| { let a = vstart + i * stride + o; [f32_at(data, a), f32_at(data, a + 4)] }).collect()).unwrap_or_default();
    let indices: Vec<u32> = (0..ibytes / isize).map(|k| {
        let a = istart + k * isize;
        if isize == 2 { u16::from_le_bytes([data[a], data[a + 1]]) as u32 } else { u32::from_le_bytes([data[a], data[a + 1], data[a + 2], data[a + 3]]) }
    }).collect();
    if let Some(bad) = indices.iter().find(|&&i| i as usize >= count) { bail!("index {bad} out of range of {count} vertices: offset 0x{istart:x}"); }

    let mut submeshes = vec![];
    for (k, s) in risen_formats::xmsh::parse_submeshes(data).into_iter().enumerate() {
        let (Some(first), Some(n)) = (s.first_index, s.index_count) else { bail!("submesh {k} ({}) has no FirstIndex/IndexCount", s.material) };
        if first < 0 || n < 0 || first as usize + n as usize > indices.len() { bail!("submesh {k} ({}) range {first}+{n} exceeds {} indices", s.material, indices.len()); }
        submeshes.push(SubRange { material: s.material, first_index: first as u32, index_count: n as u32 });
    }
    Ok(MeshGeometry { positions, normals, uvs, indices, submeshes })
}

