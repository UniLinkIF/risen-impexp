//! `._xmsh` → OBJ + MTL + PNG textures for Blender's own OBJ importer.
//!
//! Axes: Risen is Direct3D (left-handed, Y up, centimetres, clockwise front faces, V down). The OBJ we
//! write is the game geometry mirrored in Z — `(x, y, -z)`, right-handed and Y up, the same rule
//! world layers need to line up — with V flipped to OBJ's V up. One mirror turns
//! D3D's clockwise winding into OBJ's counter-clockwise, so triangles keep their file order.
//! Blender's importer (forward -Z, up Y) then turns Y up into Z up. Units stay centimetres; the
//! add-on applies its scale on import.

use crate::game::GameCtx;
use crate::xmsh_geom;
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

/// Texture file stem (lowercase, cut at the first '.') → its `._ximg` entry.
pub struct TextureIndex { by_key: HashMap<String, String> }

pub fn key(name: &str) -> String {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    file.split('.').next().unwrap_or(file).to_lowercase()
}

impl TextureIndex {
    pub fn build(g: &GameCtx) -> TextureIndex {
        TextureIndex { by_key: g.entries_with_suffix("._ximg").into_iter().map(|e| (key(&e), e)).collect() }
    }

    /// A diffuse or (converted) normal texture as PNG under `out`; the path relative to `out`.
    pub fn texture(&self, g: &GameCtx, name: &str, out: &Path, normal: bool) -> Result<Option<String>> {
        if normal { self.png(g, name, out, "_n", Some(convert_normal)) } else { self.png(g, name, out, "", None) }
    }

    /// `textures/<stem><suffix>.png` under `out`, decoded once; `convert` rewrites the pixels (normal maps).
    fn png(&self, g: &GameCtx, name: &str, out: &Path, suffix: &str, convert: Option<fn(&[u8]) -> Vec<u8>>) -> Result<Option<String>> {
        let k = key(name);
        let Some(entry) = self.by_key.get(&k) else { return Ok(None) };
        let rel = format!("textures/{k}{suffix}.png");
        let path = out.join(&rel);
        if !path.is_file() {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let (bytes, _) = g.read(entry)?;
            let img = risen_formats::dds::decode(risen_formats::ximg::extract_dds(&bytes)?)?;
            let rgba = match convert { Some(f) => f(&img.rgba), None => img.rgba };
            let tmp = path.with_extension("png.rc-tmp");
            image::save_buffer_with_format(&tmp, &rgba, img.width, img.height, image::ExtendedColorType::Rgba8, image::ImageFormat::Png)?;
            std::fs::rename(&tmp, &path)?;
        }
        Ok(Some(rel))
    }
}

/// DXT5nm keeps X in alpha with R = B = 0; a plain RGB map has B ≈ 255 (Z), so it never matches.
fn is_dxt5nm(rgba: &[u8]) -> bool {
    let (mut r, mut b, mut a) = (0u8, 0u8, 0u8);
    for p in rgba.chunks_exact(4) { r = r.max(p[0]); b = b.max(p[2]); a = a.max(p[3]); }
    r <= 16 && b <= 16 && a > 16
}

/// Risen (D3D, Y down) normal texture → Blender tangent-space normal map (OpenGL, Y up), Z rebuilt.
fn convert_normal(rgba: &[u8]) -> Vec<u8> {
    let nm = is_dxt5nm(rgba);
    let mut out = Vec::with_capacity(rgba.len());
    for p in rgba.chunks_exact(4) {
        let x = (if nm { p[3] } else { p[0] }) as f32 / 255.0 * 2.0 - 1.0;
        let y = p[1] as f32 / 255.0 * 2.0 - 1.0;
        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
        let b = |v: f32| ((v * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
        out.extend_from_slice(&[b(x), b(-y), b(z), 255]);
    }
    out
}

#[derive(serde::Serialize)]
pub struct Material { pub name: String, pub diffuse: Option<String>, pub normal: Option<String> }

#[derive(serde::Serialize)]
pub struct MeshOut { pub entry: String, pub obj: String, pub vertices: usize, pub triangles: usize, pub materials: Vec<Material>, pub warnings: Vec<String> }

/// A submesh material `Foo.xmat` → its textures, through `Foo._xmat`; a material without one is
/// tried as a texture name (a few meshes name the texture directly).
pub fn material(g: &GameCtx, tex: &TextureIndex, name: &str, out: &Path, warnings: &mut Vec<String>) -> Result<Material> {
    let base = name.split('.').next().unwrap_or(name);
    let mut m = Material { name: name.to_string(), diffuse: None, normal: None };
    match g.find_one(&format!("/{base}._xmat")) {
        Ok(entry) => {
            let (bytes, _) = g.read(&entry)?;
            let Some(t) = risen_formats::xmat::parse(&bytes) else { warnings.push(format!("material {name}: unreadable ._xmat")); return Ok(m) };
            if let Some(d) = &t.diffuse { m.diffuse = tex.png(g, d, out, "", None)?; }
            if let Some(n) = &t.normal {
                match tex.png(g, n, out, "_n", Some(convert_normal)) { Ok(p) => m.normal = p, Err(e) => warnings.push(format!("material {name}: normal map {n} skipped ({e})")) }
            }
        }
        Err(_) => m.diffuse = tex.png(g, base, out, "", None)?,
    }
    if m.diffuse.is_none() { warnings.push(format!("material {name}: no diffuse texture found")); }
    Ok(m)
}

/// `name` is an entry path (`/…/Foo._xmsh`) or a bare mesh name (`Foo`, `Foo._xmsh`).
pub fn resolve_entry(g: &GameCtx, name: &str) -> Result<String> {
    if name.starts_with('/') { return Ok(name.to_string()); }
    let stem = name.strip_suffix("._xmsh").unwrap_or(name);
    g.find_one(&format!("/{stem}._xmsh"))
}

pub fn export_obj(g: &GameCtx, tex: &TextureIndex, name: &str, out: &Path) -> Result<MeshOut> {
    let entry = resolve_entry(g, name)?;
    let (bytes, _) = g.read(&entry)?;
    let m = xmsh_geom::decode(&bytes)?;
    if m.submeshes.is_empty() { bail!("{entry}: no submeshes"); }
    let stem = entry.rsplit('/').next().unwrap().split('.').next().unwrap().to_string();
    let mut warnings = vec![];

    // One OBJ group per material; submeshes sharing a material merge.
    let mut order: Vec<String> = vec![];
    let mut tris: HashMap<String, Vec<&[u32]>> = HashMap::new();
    for s in &m.submeshes {
        let range = &m.indices[s.first_index as usize..(s.first_index + s.index_count) as usize];
        if !tris.contains_key(&s.material) { order.push(s.material.clone()); }
        tris.entry(s.material.clone()).or_default().extend(range.chunks_exact(3));
    }
    let mats: Vec<Material> = order.iter().map(|n| material(g, tex, n, out, &mut warnings)).collect::<Result<_>>()?;

    let mut obj = format!("# {entry}\nmtllib {stem}.mtl\n");
    for p in &m.positions { writeln!(obj, "v {} {} {}", p[0], p[1], -p[2])?; }
    for t in &m.uvs { writeln!(obj, "vt {} {}", t[0], 1.0 - t[1])?; }
    for n in &m.normals { writeln!(obj, "vn {} {} {}", n[0], n[1], -n[2])?; }
    let (has_uv, has_n) = (!m.uvs.is_empty(), !m.normals.is_empty());
    let corner = |i: u32| { let k = i + 1; match (has_uv, has_n) { (true, true) => format!("{k}/{k}/{k}"), (true, false) => format!("{k}/{k}"), (false, true) => format!("{k}//{k}"), _ => k.to_string() } };
    let mut mtl = String::new();
    let mut triangles = 0;
    for (name, mat) in order.iter().zip(&mats) {
        let ident = name.split('.').next().unwrap_or(name).replace([' ', '\t'], "_");
        writeln!(obj, "g {ident}\nusemtl {ident}")?;
        for t in &tris[name] { writeln!(obj, "f {} {} {}", corner(t[0]), corner(t[1]), corner(t[2]))?; triangles += 1; }
        writeln!(mtl, "newmtl {ident}\nKd 1 1 1")?;
        if let Some(d) = &mat.diffuse { writeln!(mtl, "map_Kd {d}")?; }
        if let Some(n) = &mat.normal { writeln!(mtl, "map_Bump {n}")?; }
    }
    // .mtl first, .obj last: the .obj appearing is the commit point.
    let obj_path = out.join(format!("{stem}.obj"));
    std::fs::create_dir_all(out)?;
    std::fs::write(obj_path.with_extension("mtl"), mtl)?;
    let tmp = out.join(format!("{stem}.obj.rc-tmp"));
    std::fs::write(&tmp, obj)?;
    std::fs::rename(&tmp, &obj_path)?;
    Ok(MeshOut { entry, obj: obj_path.to_string_lossy().into_owned(), vertices: m.positions.len(), triangles, materials: mats, warnings })
}
