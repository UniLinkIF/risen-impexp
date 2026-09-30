//! The property section shared by Genome resource files (`GR01MS02` meshes, `GR01MA02`-style
//! materials, `GR01IM04` images, …) and by the engine's resource directories (`compiled_*.bin`).
//!
//! ```text
//! file  0x00 "GR01" + 4-char kind     0x08 u32 40 (section offset)   0x0C u32 section size
//!       0x10 u32 data offset          0x14 u32 data size             0x18 FILETIME, 8 bytes (zero in most files)
//!       0x28 section, then the kind's own data (mesh buffers, DDS, …) from the data offset
//! section  6 bytes · str class · 3 bytes · object · tail to section end
//! object   u16 201 · u16 201 · u32 size · body          (the root)
//! element  u16 201 · u32 size · body                    (one entry of a bTObjArray)
//! body     u16 201 · u32 property count · properties · opaque rest of `size`
//! property str name · str type · u16 tag · u32 size · value
//!          bCString       → str
//!          bTObjArray<…>  → u8 · u32 count · elements
//!          anything else  → raw bytes
//! str      u16 length · Latin-1 bytes
//! ```
//!
//! In a `compiled_*.bin` record every `str` is a u16 index into the file's string pool instead; the
//! layout is otherwise the same, so one parser and one writer take the string codec as a parameter.

use anyhow::{bail, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Array { head: u8, elems: Vec<Body> },
    /// `bTObjArray<struct eCMotionResource2::SEffect>`: (key frame, effect name) pairs.
    Effects { head: u8, items: Vec<(u16, String)> },
    Raw(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prop { pub name: String, pub ty: String, pub tag: u16, pub value: Value }

#[derive(Debug, Clone, PartialEq)]
pub struct Body { pub props: Vec<Prop>, pub rest: Vec<u8> }

#[derive(Debug, Clone, PartialEq)]
pub struct Section { pub prefix: [u8; 6], pub class: String, pub mid: [u8; 3], pub root: Body, pub tail: Vec<u8> }

/// How a string is stored: inline (resource files) or as a pool index (resource directories).
pub trait Strings {
    fn read(&mut self, r: &mut Reader) -> Result<String>;
    fn write(&mut self, s: &str, out: &mut Vec<u8>);
}

pub struct Inline;
impl Strings for Inline {
    fn read(&mut self, r: &mut Reader) -> Result<String> { let n = r.u16()? as usize; Ok(latin1(r.bytes(n)?)) }
    fn write(&mut self, s: &str, out: &mut Vec<u8>) { let b = to_latin1(s); out.extend((b.len() as u16).to_le_bytes()); out.extend(b); }
}

pub fn latin1(b: &[u8]) -> String { b.iter().map(|&c| c as char).collect() }
pub fn to_latin1(s: &str) -> Vec<u8> { s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect() }

pub struct Reader<'a> { pub d: &'a [u8], pub at: usize }
impl<'a> Reader<'a> {
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let Some(b) = self.d.get(self.at..self.at + n) else { bail!("runs past end: offset 0x{:x} + {n}", self.at) };
        self.at += n;
        Ok(b)
    }
    pub fn u8(&mut self) -> Result<u8> { Ok(self.bytes(1)?[0]) }
    pub fn u16(&mut self) -> Result<u16> { let b = self.bytes(2)?; Ok(u16::from_le_bytes([b[0], b[1]])) }
    pub fn u32(&mut self) -> Result<u32> { let b = self.bytes(4)?; Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]])) }
    fn expect201(&mut self) -> Result<()> { let at = self.at; let v = self.u16()?; if v != 201 { bail!("expected version 201, got {v}: offset 0x{at:x}") } Ok(()) }
}

fn body(r: &mut Reader, s: &mut dyn Strings, size: usize) -> Result<Body> {
    let end = r.at + size;
    r.expect201()?;
    let n = r.u32()?;
    let mut props = Vec::with_capacity(n as usize);
    for _ in 0..n { props.push(prop(r, s)?); }
    if r.at > end { bail!("object body overruns its size: offset 0x{:x} > 0x{end:x}", r.at); }
    let rest = r.bytes(end - r.at)?.to_vec();
    Ok(Body { props, rest })
}

fn prop(r: &mut Reader, s: &mut dyn Strings) -> Result<Prop> {
    let name = s.read(r)?;
    let ty = s.read(r)?;
    let tag = r.u16()?;
    let size = r.u32()? as usize;
    let end = r.at + size;
    let value = if ty == "bCString" {
        Value::Str(s.read(r)?)
    } else if ty == "bTObjArray<struct eCMotionResource2::SEffect>" {
        let head = r.u8()?;
        let n = r.u32()?;
        let mut items = Vec::with_capacity(n as usize);
        for _ in 0..n { let k = r.u16()?; items.push((k, s.read(r)?)); }
        Value::Effects { head, items }
    } else if ty.starts_with("bTObjArray<") && r.d.get(r.at + 5..r.at + 7).map_or(true, |b| b == [201, 0]) {
        // An array of objects (each element a versioned body). Arrays of plain values
        // (`bTObjArray<class bCBox>` in `._xcom`) have no version tag and stay raw.
        let head = r.u8()?;
        let n = r.u32()?;
        let mut elems = Vec::with_capacity(n as usize);
        for _ in 0..n { r.expect201()?; let sz = r.u32()? as usize; elems.push(body(r, s, sz)?); }
        Value::Array { head, elems }
    } else {
        Value::Raw(r.bytes(size)?.to_vec())
    };
    if r.at != end { bail!("property {name} ({ty}) size {size} does not match its contents: offset 0x{:x} vs 0x{end:x}", r.at); }
    Ok(Prop { name, ty, tag, value })
}

pub fn read_section(r: &mut Reader, s: &mut dyn Strings, end: usize) -> Result<Section> {
    let prefix: [u8; 6] = r.bytes(6)?.try_into().unwrap();
    let class = s.read(r)?;
    let mid: [u8; 3] = r.bytes(3)?.try_into().unwrap();
    r.expect201()?;
    r.expect201()?;
    let size = r.u32()? as usize;
    let root = body(r, s, size)?;
    if r.at > end { bail!("section overruns its size: offset 0x{:x} > 0x{end:x}", r.at); }
    let tail = r.bytes(end - r.at)?.to_vec();
    Ok(Section { prefix, class, mid, root, tail })
}

fn write_body(b: &Body, s: &mut dyn Strings) -> Vec<u8> {
    let mut o = 201u16.to_le_bytes().to_vec();
    o.extend((b.props.len() as u32).to_le_bytes());
    for p in &b.props {
        s.write(&p.name, &mut o);
        s.write(&p.ty, &mut o);
        o.extend(p.tag.to_le_bytes());
        let mut v = vec![];
        match &p.value {
            Value::Str(x) => s.write(x, &mut v),
            Value::Raw(x) => v.extend(x),
            Value::Effects { head, items } => {
                v.push(*head);
                v.extend((items.len() as u32).to_le_bytes());
                for (k, name) in items { v.extend(k.to_le_bytes()); s.write(name, &mut v); }
            }
            Value::Array { head, elems } => {
                v.push(*head);
                v.extend((elems.len() as u32).to_le_bytes());
                for e in elems { let eb = write_body(e, s); v.extend(201u16.to_le_bytes()); v.extend((eb.len() as u32).to_le_bytes()); v.extend(eb); }
            }
        }
        o.extend((v.len() as u32).to_le_bytes());
        o.extend(v);
    }
    o.extend(&b.rest);
    o
}

pub fn write_section(sec: &Section, s: &mut dyn Strings) -> Vec<u8> {
    let mut o = sec.prefix.to_vec();
    s.write(&sec.class, &mut o);
    o.extend(sec.mid);
    let root = write_body(&sec.root, s);
    o.extend(201u16.to_le_bytes());
    o.extend(201u16.to_le_bytes());
    o.extend((root.len() as u32).to_le_bytes());
    o.extend(root);
    o.extend(&sec.tail);
    o
}

/// A whole resource file: header fields, section, and the bytes from the data offset on.
#[derive(Debug, Clone, PartialEq)]
pub struct Resource { pub magic: [u8; 8], pub filetime: [u8; 8], pub reserved: [u8; 8], pub section: Section, pub data: Vec<u8> }

impl Resource {
    pub fn parse(d: &[u8]) -> Result<Resource> {
        if d.len() < 0x28 || &d[..4] != b"GR01" { bail!("not a GR01 resource"); }
        let mut r = Reader { d, at: 0x08 };
        let (off, size, data_off) = (r.u32()? as usize, r.u32()? as usize, r.u32()? as usize);
        if off != 0x28 { bail!("section offset {off}, expected 40"); }
        if data_off > d.len() { bail!("data offset 0x{data_off:x} past end"); }
        let mut r = Reader { d, at: 0x28 };
        let section = read_section(&mut r, &mut Inline, 0x28 + size)?;
        Ok(Resource { magic: d[..8].try_into().unwrap(), filetime: d[0x18..0x20].try_into().unwrap(), reserved: d[0x20..0x28].try_into().unwrap(), section, data: d[data_off..].to_vec() })
    }

    /// Header sizes are recomputed: section size = data offset - 40 (the section's tail carries the
    /// bytes between the object and the data), data size = bytes from the data offset on.
    pub fn write(&self) -> Vec<u8> {
        let sec = write_section(&self.section, &mut Inline);
        let mut o = self.magic.to_vec();
        o.extend(0x28u32.to_le_bytes());
        o.extend((sec.len() as u32).to_le_bytes());
        o.extend((0x28 + sec.len() as u32).to_le_bytes());
        o.extend((self.data.len() as u32).to_le_bytes());
        o.extend(self.filetime);
        o.extend(self.reserved);
        o.extend(sec);
        o.extend(&self.data);
        o
    }
}

#[cfg(test)]
impl Body {
    pub fn get(&self, name: &str) -> Option<&Prop> { self.props.iter().find(|p| p.name == name) }
}

/// A `bCBox`: min xyz, max xyz as f32.
pub fn bbox_bytes(lo: [f32; 3], hi: [f32; 3]) -> Vec<u8> { lo.iter().chain(&hi).flat_map(|f| f.to_le_bytes()).collect() }

#[cfg(test)]
mod tests {
    use super::*;

    /// Every mesh, material and image in the game survives parse → write byte for byte.
    #[test]
    fn round_trips_every_resource_in_the_game() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        for suffix in ["._xmsh", "._xmat", "._ximg"] {
            let (mut ok, mut bad) = (0, vec![]);
            for e in g.entries_with_suffix(suffix) {
                let (d, _) = g.read(&e).unwrap();
                match Resource::parse(&d) {
                    Ok(r) if r.write() == d => ok += 1,
                    Ok(r) => { let w = r.write(); let at = w.iter().zip(&d).position(|(a, b)| a != b).unwrap_or(w.len().min(d.len())); bad.push(format!("{e}: differs at 0x{at:x} (len {} vs {}) hdr {:?}", w.len(), d.len(), &d[8..0x18])) }
                    Err(err) => bad.push(format!("{e}: {err}")),
                }
            }
            eprintln!("{suffix}: {ok} identical, {} not", bad.len());
            for b in bad.iter().take(5) { eprintln!("  {b}"); }
            assert!(bad.is_empty(), "{suffix}: {} files do not round-trip", bad.len());
        }
    }
}

/// Debug view of a body: properties with strings, arrays and raw hex.
pub fn to_json(b: &Body) -> serde_json::Value {
    let hex = |v: &[u8]| v.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let props: Vec<serde_json::Value> = b.props.iter().map(|p| {
        let v = match &p.value {
            Value::Str(s) => serde_json::json!(s),
            Value::Raw(r) => serde_json::json!(hex(r)),
            Value::Array { head, elems } => serde_json::json!({ "head": head, "elems": elems.iter().map(to_json).collect::<Vec<_>>() }),
            Value::Effects { items, .. } => serde_json::json!(items),
        };
        serde_json::json!({ "name": p.name, "type": p.ty, "tag": p.tag, "value": v })
    }).collect();
    serde_json::json!({ "props": props, "rest": hex(&b.rest) })
}
