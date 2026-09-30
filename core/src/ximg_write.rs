//! Writing a Risen 1 texture (`._ximg`): an `eCImageResource2` section followed by a plain DDS.
//!
//! ```text
//! section  Width · Height · SkipMips · PixelFormat ("DXT1" / "DXT5")
//!          SkipMips = hi << 8 | lo (levels dropped at the two lower texture qualities)
//!          rest: u16 201 · u32 chain(lo) · u32 chain(hi) or 0 · 2 × u32 0 · 16-byte preview block
//!          chain(k) = bytes of the mip levels from k on, stopping before the first level
//!                  smaller than 4 px on either side
//!          preview = the whole picture as one 4×4 block, DXT5-shaped (a DXT1 texture puts the
//!                  opaque alpha block ffff000000000000 in front of its colour block)
//! data     "DDS " header (124 bytes, flags 0x21007, caps 0x401008) + every mip level down to 1×1
//! ```
//! Layout checked against every shipped texture (see the tests).

use crate::gr01::{Body, Prop, Resource, Section, Value};
use anyhow::{bail, Result};
use texpresso::{Algorithm, Format, Params};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { Dxt1, Dxt5 }

impl Kind {
    fn fourcc(self) -> &'static [u8; 4] { match self { Kind::Dxt1 => b"DXT1", Kind::Dxt5 => b"DXT5" } }
    fn format(self) -> Format { match self { Kind::Dxt1 => Format::Bc1, Kind::Dxt5 => Format::Bc3 } }
    fn block(self) -> usize { match self { Kind::Dxt1 => 8, Kind::Dxt5 => 16 } }
}

fn levels(w: u32, h: u32) -> Vec<(u32, u32)> {
    let mut v = vec![(w, h)];
    while v.last().map_or(false, |&(a, b)| a > 1 || b > 1) { let (a, b) = *v.last().unwrap(); v.push(((a / 2).max(1), (b / 2).max(1))); }
    v
}

fn level_bytes(kind: Kind, w: u32, h: u32) -> usize { (w as usize).div_ceil(4) * (h as usize).div_ceil(4) * kind.block() }

pub fn chain(kind: Kind, w: u32, h: u32, skip: u32) -> u32 {
    levels(w, h).into_iter().enumerate().take_while(|(_, (a, b))| (*a).min(*b) >= 4).filter(|(i, _)| *i as u32 >= skip).map(|(_, (a, b))| level_bytes(kind, a, b) as u32).sum()
}

fn params() -> Params { Params { algorithm: Algorithm::ClusterFit, ..Params::default() } }

fn compress(kind: Kind, rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let f = kind.format();
    let mut out = vec![0u8; f.compressed_size(w as usize, h as usize)];
    f.compress(rgba, w as usize, h as usize, params(), &mut out);
    out
}

fn resize(rgba: &[u8], w: u32, h: u32, nw: u32, nh: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_raw(w, h, rgba.to_vec()).expect("rgba size");
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle).into_raw()
}

/// A texture from RGBA pixels (sides must be powers of two; the caller resizes).
pub fn write(rgba: &[u8], w: u32, h: u32, kind: Kind, filetime: u64) -> Result<Vec<u8>> {
    if !w.is_power_of_two() || !h.is_power_of_two() { bail!("texture is {w}×{h}; sides must be powers of two"); }
    if rgba.len() != (w * h * 4) as usize { bail!("pixel buffer is {} bytes, expected {}", rgba.len(), w * h * 4); }
    let lv = levels(w, h);
    let mut pixels = vec![];
    let mut cur = rgba.to_vec();
    let (mut cw, mut ch) = (w, h);
    for (i, &(lw, lh)) in lv.iter().enumerate() {
        if i > 0 { cur = resize(&cur, cw, ch, lw, lh); (cw, ch) = (lw, lh); }
        pixels.extend(compress(kind, &cur, lw, lh));
    }
    // SkipMips = hi << 8 | lo: the levels dropped at the two lower texture qualities. The shipped habit
    // (1100 of 1263 files): 0x103 from 256 px, 1 at 128, 0 below.
    let skip: u32 = match w.max(h) { s if s >= 256 => 0x103, 128 => 1, _ => 0 };
    let (lo, hi) = (skip & 0xff, skip >> 8);

    let tiny = resize(rgba, w, h, 4, 4);
    let mut preview = vec![];
    if kind == Kind::Dxt1 { preview.extend([0xff, 0xff, 0, 0, 0, 0, 0, 0]); }
    preview.extend(compress(kind, &tiny, 4, 4));

    let mut rest = 201u16.to_le_bytes().to_vec();
    for v in [chain(kind, w, h, lo), if hi > 0 { chain(kind, w, h, hi) } else { 0 }, 0, 0] { rest.extend(v.to_le_bytes()); }
    rest.extend(preview);
    let raw = |name: &str, ty: &str, v: Vec<u8>| Prop { name: name.into(), ty: ty.into(), tag: 30, value: Value::Raw(v) };
    let mut pf = 201u16.to_le_bytes().to_vec();
    pf.extend(kind.fourcc());
    let root = Body {
        props: vec![
            raw("Width", "int", (w as i32).to_le_bytes().to_vec()),
            raw("Height", "int", (h as i32).to_le_bytes().to_vec()),
            raw("SkipMips", "long", (skip as i32).to_le_bytes().to_vec()),
            raw("PixelFormat", "bTPropertyContainer<enum eCGfxShared::eEColorFormat>", pf),
        ],
        rest,
    };
    let section = Section { prefix: [1, 0, 1, 1, 0, 1], class: "eCImageResource2".into(), mid: [1, 0, 0], root, tail: vec![] };

    let mut dds = b"DDS ".to_vec();
    let mut hdr = [0u32; 31];
    hdr[0] = 124;
    hdr[1] = 0x0002_1007;
    hdr[2] = h;
    hdr[3] = w;
    hdr[6] = lv.len() as u32;
    hdr[18] = 32; // pixel format size
    hdr[19] = 4; // DDPF_FOURCC
    hdr[20] = u32::from_le_bytes(*kind.fourcc());
    hdr[26] = 0x0040_1008;
    for v in hdr { dds.extend(v.to_le_bytes()); }
    dds.extend(pixels);
    Ok(Resource { magic: *b"GR01IM04", filetime: filetime.to_le_bytes(), reserved: [0; 8], section, data: dds }.write())
}

/// Nearest power of two not above 2048 (Risen's largest), per side.
pub fn pot(n: u32) -> u32 { let p = n.max(4).next_power_of_two(); if p / 2 >= n.max(4) * 3 / 4 && p > 4 { p / 2 } else { p }.min(2048) }

/// PNG (any size) → RGBA at power-of-two size.
pub fn load_png(path: &std::path::Path) -> Result<(Vec<u8>, u32, u32)> {
    let img = image::open(path)?.to_rgba8();
    let (w, h) = img.dimensions();
    let (nw, nh) = (pot(w), pot(h));
    if (nw, nh) == (w, h) { return Ok((img.into_raw(), w, h)); }
    Ok((image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Lanczos3).into_raw(), nw, nh))
}

/// Blender normal map (OpenGL, Y up, X in red) → Risen DXT5nm pixels (X in alpha, Y down in green).
pub fn to_dxt5nm(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).flat_map(|p| [0, 255 - p[1], 0, p[0]]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chain value and the zeros around it follow the rule above in every shipped texture.
    #[test]
    fn chain_rule_holds_for_every_shipped_texture() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        let mut skips = std::collections::BTreeMap::new();
        let (mut ok, mut bad) = (0, vec![]);
        for e in g.entries_with_suffix("._ximg") {
            let r = Resource::parse(&g.read(&e).unwrap().0).unwrap();
            let b = &r.section.root;
            let i = |n: &str| match b.get(n).map(|p| &p.value) { Some(Value::Raw(v)) => u32::from_le_bytes(v[..4].try_into().unwrap()), _ => panic!("{e}: {n}") };
            let kind = match b.get("PixelFormat").map(|p| &p.value) { Some(Value::Raw(v)) if &v[2..] == b"DXT1" => Kind::Dxt1, Some(Value::Raw(v)) if &v[2..] == b"DXT5" => Kind::Dxt5, _ => continue };
            let rest = &b.rest;
            let u = |a: usize| u32::from_le_bytes(rest[a..a + 4].try_into().unwrap());
            *skips.entry((i("Width").max(i("Height")), i("SkipMips"))).or_insert(0) += 1;
            let want = chain(kind, i("Width"), i("Height"), i("SkipMips") & 0xff);
            let hi = i("SkipMips") >> 8;
            let want2 = if hi > 0 { chain(kind, i("Width"), i("Height"), hi) } else { 0 };
            if rest.len() == 34 && ((u(2) == want && u(6) == want2) || (u(2) == 0 && u(6) == 0)) && u(10) == 0 && u(14) == 0 && &r.data[..4] == b"DDS " { ok += 1 } else { bad.push(format!("{e}: rest {} chain {} want {want} z {} {} {} skip {:x} {}x{}", rest.len(), u(2), u(6), u(10), u(14), i("SkipMips"), i("Width"), i("Height"))) }
        }
        eprintln!("(max side, SkipMips) -> count: {skips:?}");
        eprintln!("{ok} textures follow the rule; {} do not: {:?}", bad.len(), &bad[..bad.len().min(5)]);
        // A handful of editor and special textures (MipMapRange_*, posters) carry other numbers.
        assert!(bad.len() * 100 < ok, "{} of {} break the rule", bad.len(), ok + bad.len());
    }

    #[test]
    fn written_texture_decodes_back() {
        let (w, h) = (64u32, 32u32);
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 4) as u8, (i / w * 8) as u8, 128, 255]).collect();
        let f = write(&rgba, w, h, Kind::Dxt1, 0).unwrap();
        let r = Resource::parse(&f).unwrap();
        assert_eq!(r.write(), f);
        let img = risen_formats::dds::decode(risen_formats::ximg::extract_dds(&f).unwrap()).unwrap();
        assert_eq!((img.width, img.height), (w, h));
        let err = img.rgba.iter().zip(&rgba).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(err <= 24, "max channel error {err}");
    }
}
