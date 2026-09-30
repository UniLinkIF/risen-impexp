//! Blender mesh → the files of a Risen mod: `._xmsh`, new `._xmat`/`._ximg` for new materials, and
//! the three resource directories with our records in them.
//!
//! Input (written by the add-on): a JSON spec plus a raw geometry file, already in game space
//! (centimetres, Y up, left-handed, clockwise front faces, Direct3D V):
//! ```text
//! f32 position[3·N] · f32 normal[3·N] · f32 uv[2·N] · u32 material[N/3]      N = triangle corners
//! ```

use crate::cache::{self, Cache};
use crate::game::GameCtx;
use crate::xmsh_write::{self, Part, Vertex};
use crate::ximg_write::{self, Kind};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(serde::Deserialize)]
pub struct MaterialSpec {
    /// Blender material name. When it names a material the game already has (as imported meshes'
    /// materials do), that material is reused as is.
    pub name: String,
    pub diffuse: Option<String>,
    pub normal: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct Spec {
    /// The mesh to write: an existing game mesh name replaces it, a new name adds a mesh.
    pub name: String,
    pub geometry: String,
    pub corners: usize,
    pub materials: Vec<MaterialSpec>,
    #[serde(default)]
    pub collision: Option<CollisionSpec>,
}

/// Collision for the mesh (`<name>_COL._xcom`): `geometry` = separate collision objects in the same
/// raw layout (only positions are used), or none = the render mesh itself. `material` = a Risen shape
/// material name (stone, wood, …); none = keep the replaced collision's, else stone.
#[derive(serde::Deserialize)]
pub struct CollisionSpec { pub geometry: Option<String>, #[serde(default)] pub corners: usize, pub material: Option<String> }

#[derive(serde::Serialize, Default)]
pub struct Report { pub files: Vec<String>, pub replaced: bool, pub collision: Option<String>, pub vertices: usize, pub triangles: usize, pub materials: Vec<String>, pub warnings: Vec<String> }

/// The template material: diffuse + normal sampler, default shader. New materials copy it with
/// their texture names swapped in; names keep the template's lengths because the shader data
/// after the section carries its own offsets (not yet decoded), so nothing may shift.
const TEMPLATE_MAT: &str = "ItWpn_SwordMisc_01_Diffuse_01";
const TEMPLATE_STEM: &str = "ItWpn_SwordMisc_01";

fn hash(s: &str) -> u64 { s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3)) }

/// An 18-character stem (the template's length): `BM_` + up to 10 letters of the name + `_` + hash.
pub fn stem_for(name: &str) -> String {
    let part: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).take(10).collect();
    let n = TEMPLATE_STEM.len() - 4 - part.len();
    format!("BM_{part}_{}", &format!("{:016x}", hash(name))[..n])
}

/// Blender adds `.001`-style suffixes to duplicated names; the game name is what came before.
fn game_name(blender: &str) -> &str {
    match blender.rsplit_once('.') { Some((a, b)) if b.len() == 3 && b.bytes().all(|c| c.is_ascii_digit()) => a, _ => blender }
}

fn read_geometry(path: &Path, corners: usize) -> Result<(Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>)> {
    let d = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if corners % 3 != 0 { bail!("{corners} corners is not whole triangles"); }
    let want = corners * (3 + 3 + 2) * 4 + corners / 3 * 4;
    if d.len() != want { bail!("geometry is {} bytes, expected {want} for {corners} corners", d.len()); }
    let f = |i: usize| f32::from_le_bytes(d[i * 4..i * 4 + 4].try_into().unwrap());
    let pos = (0..corners).map(|i| [f(i * 3), f(i * 3 + 1), f(i * 3 + 2)]).collect();
    let o = corners * 3;
    let nrm = (0..corners).map(|i| [f(o + i * 3), f(o + i * 3 + 1), f(o + i * 3 + 2)]).collect();
    let o = corners * 6;
    let uv = (0..corners).map(|i| [f(o + i * 2), f(o + i * 2 + 1)]).collect();
    let o = corners * 8;
    let mat = (0..corners / 3).map(|i| u32::from_le_bytes(d[(o + i) * 4..(o + i) * 4 + 4].try_into().unwrap())).collect();
    Ok((pos, nrm, uv, mat))
}

/// Where a file lives relative to the game folder.
fn loose_mesh_path(g: &GameCtx, name: &str) -> Result<(String, bool)> {
    match g.find_one(&format!("/{name}._xmsh")) {
        Ok(entry) => {
            let p = g.physical_path(&entry)?;
            Ok((p.strip_prefix(&g.root)?.to_string_lossy().replace('\\', "/"), true))
        }
        Err(_) => Ok((format!("data/common/meshes/BlenderMod/{name}._xmsh"), false)),
    }
}

pub struct Built { pub report: Report, /// relative game path → bytes
    pub files: Vec<(String, Vec<u8>)> }

pub fn build(g: &GameCtx, spec: &Spec) -> Result<Built> {
    let name = spec.name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') { bail!("mesh name {name:?}: use letters, digits and _ only"); }
    let (pos, nrm, uv, mat) = read_geometry(Path::new(&spec.geometry), spec.corners)?;
    if pos.is_empty() { bail!("the mesh has no triangles"); }
    let mut report = Report::default();
    let mut files: Vec<(String, Vec<u8>)> = vec![];
    let now = cache::filetime(std::time::SystemTime::now());

    // Materials: reuse a game material by name, otherwise make one from the template.
    let game_mats: HashMap<String, String> = g.entries_with_suffix("._xmat").into_iter()
        .map(|e| (e.rsplit('/').next().unwrap().split('.').next().unwrap().to_lowercase(), e)).collect();
    let mut mat_names = vec![];
    for (i, m) in spec.materials.iter().enumerate() {
        let base = game_name(&m.name);
        if let Some(e) = game_mats.get(&base.to_lowercase()) {
            mat_names.push(format!("{}._xmat", e.rsplit('/').next().unwrap().split('.').next().unwrap()));
            report.materials.push(format!("{}: game material reused", m.name));
            continue;
        }
        let stem = stem_for(&m.name);
        let (tex_d, tex_n) = (format!("{stem}_Diffuse_01"), format!("{stem}_Normal_01"));
        let diffuse = match &m.diffuse {
            Some(p) => ximg_write::load_png(Path::new(p)).with_context(|| format!("material {}: diffuse {p}", m.name))?,
            None => { report.warnings.push(format!("material {}: no Base Color image, grey used", m.name)); (vec![128; 4 * 4 * 4], 4, 4) }
        };
        let normal = match &m.normal {
            Some(p) => { let (px, w, h) = ximg_write::load_png(Path::new(p)).with_context(|| format!("material {}: normal {p}", m.name))?; (ximg_write::to_dxt5nm(&px), w, h) }
            None => (ximg_write::to_dxt5nm(&[128, 128, 255, 255].repeat(16)), 4, 4),
        };
        files.push((format!("data/compiled/images/BlenderMod/{tex_d}._ximg"), ximg_write::write(&diffuse.0, diffuse.1, diffuse.2, Kind::Dxt1, now)?));
        files.push((format!("data/compiled/images/BlenderMod/{tex_n}._ximg"), ximg_write::write(&normal.0, normal.1, normal.2, Kind::Dxt5, now)?));
        files.push((format!("data/common/materials/{tex_d}._xmat"), material_from_template(g, &stem)?));
        mat_names.push(format!("{tex_d}._xmat"));
        report.materials.push(format!("{}: new material {tex_d} ({}×{})", m.name, diffuse.1, diffuse.2));
        let _ = i;
    }

    // Geometry: weld identical corners per material, one submesh per material in slot order.
    let mut parts = vec![];
    for (mi, material) in mat_names.iter().enumerate() {
        let mut vertices: Vec<Vertex> = vec![];
        let mut seen: HashMap<[u32; 8], u32> = HashMap::new();
        let mut triangles = vec![];
        for (t, &m) in mat.iter().enumerate() {
            if m as usize != mi { continue; }
            let tri = [0, 1, 2].map(|k| {
                let c = t * 3 + k;
                let key = [pos[c][0], pos[c][1], pos[c][2], nrm[c][0], nrm[c][1], nrm[c][2], uv[c][0], uv[c][1]].map(f32::to_bits);
                *seen.entry(key).or_insert_with(|| { vertices.push(Vertex { pos: pos[c], normal: nrm[c], tangent: [0.0; 3], right_handed: true, uv: uv[c] }); vertices.len() as u32 - 1 })
            });
            triangles.push(tri);
        }
        if triangles.is_empty() { continue; }
        xmsh_write::compute_tangents(&mut vertices, &triangles);
        report.vertices += vertices.len();
        report.triangles += triangles.len();
        parts.push(Part { material: material.clone(), vertices, triangles });
    }
    if let Some(&bad) = mat.iter().find(|&&m| m as usize >= mat_names.len()) { bail!("triangle uses material slot {bad}, only {} given", mat_names.len()); }
    let (mesh_path, replaced) = loose_mesh_path(g, name)?;
    report.replaced = replaced;
    files.push((mesh_path, xmsh_write::write(name, &parts, now)));
    if let Some(c) = &spec.collision {
        let (cpos, ctris) = match &c.geometry {
            Some(p) => { let (cp, _, _, _) = read_geometry(Path::new(p), c.corners)?; weld_positions(&cp) }
            None => weld_positions(&pos),
        };
        let existing = g.find_one(&format!("/{name}_COL._xcom")).ok();
        let kept = existing.as_ref().and_then(|e| g.read(e).ok()).and_then(|(d, _)| crate::nxs::read_xcom(&d).ok()).and_then(|x| x.shape_materials.first().map(|s| s.material));
        let material = match c.material.as_deref() {
            Some(m) => crate::nxs::SHAPE_MATERIALS.iter().position(|s| s.eq_ignore_ascii_case(m)).with_context(|| format!("shape material {m}: use one of {}", crate::nxs::SHAPE_MATERIALS.join(", ")))? as u8,
            None => kept.unwrap_or(4),
        };
        let path = match &existing { Some(e) => g.physical_path(e)?.strip_prefix(&g.root)?.to_string_lossy().replace('\\', "/"), None => format!("data/common/physics/BlenderMod/{name}_COL._xcom") };
        files.push((path.clone(), crate::cook::xcom(&cpos, &ctris, material)?));
        report.collision = Some(format!("{path} ({} triangles, {})", ctris.len(), crate::nxs::SHAPE_MATERIALS[material as usize]));
    }
    report.files = files.iter().map(|(p, _)| p.clone()).collect();
    Ok(Built { report, files })
}

fn material_from_template(g: &GameCtx, stem: &str) -> Result<Vec<u8>> {
    assert_eq!(stem.len(), TEMPLATE_STEM.len());
    let mut d = g.read_archive(&g.find_one(&format!("/{TEMPLATE_MAT}._xmat"))?)?;
    let mut n = 0;
    let (from, to) = (TEMPLATE_STEM.as_bytes(), stem.as_bytes());
    let mut i = 0;
    while i + from.len() <= d.len() {
        if &d[i..i + from.len()] == from { d[i..i + from.len()].copy_from_slice(to); n += 1; i += from.len() } else { i += 1 }
    }
    if n != 2 { bail!("template material {TEMPLATE_MAT}: expected its name twice (diffuse, normal), found {n}"); }
    Ok(d)
}

/// The three resource directories: `base` (the archive's, or the game's current loose copy) with a
/// record for every resource file in `files`. `mtime(path)` = the FILETIME the file will carry.
pub fn directories(g: &GameCtx, files: &[(String, Vec<u8>)], from_loose: bool, mtime: impl Fn(&str) -> u64) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = vec![];
    for (suffix, entry, loose) in cache::DIRECTORIES {
        let mine: Vec<&(String, Vec<u8>)> = files.iter().filter(|(p, _)| p.to_lowercase().ends_with(suffix)).collect();
        if mine.is_empty() { continue; }
        let e = g.find_one(entry)?;
        let base = if from_loose { g.read(&e)?.0 } else { g.read_archive(&e)? };
        let mut c = Cache::parse(&base).with_context(|| format!("parse {entry}"))?;
        for (p, bytes) in mine {
            let rname = p.rsplit('/').next().unwrap().split('.').next().unwrap();
            let rec = c.record_for(bytes, rname, mtime(p))?;
            c.put(rec);
        }
        let bytes = c.write();
        Cache::parse(&bytes).with_context(|| format!("re-parse our {entry}"))?;
        out.push((loose.to_string(), bytes));
    }
    Ok(out)
}

pub fn write_all(root: &Path, files: &[(String, Vec<u8>)]) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    for (rel, bytes) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, bytes).with_context(|| format!("write {}", p.display()))?;
        out.push(p);
    }
    Ok(out)
}

pub fn file_mtime(p: &Path) -> u64 { std::fs::metadata(p).and_then(|m| m.modified()).map(cache::filetime).unwrap_or(0) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stems_have_the_template_length_and_differ() {
        let names = ["My Crate Wood", "My Crate Wood 2", "x", "Long_Blade_Metal_Two_Handed", "ящик"];
        let stems: Vec<String> = names.iter().map(|n| stem_for(n)).collect();
        eprintln!("{stems:?}");
        for s in &stems { assert_eq!(s.len(), TEMPLATE_STEM.len(), "{s}"); }
        let mut u = stems.clone(); u.sort(); u.dedup();
        assert_eq!(u.len(), stems.len());
    }
}

/// Corner positions → welded vertices and triangles (collision needs positions only).
fn weld_positions(pos: &[[f32; 3]]) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut seen: HashMap<[u32; 3], u32> = HashMap::new();
    let mut verts = vec![];
    let idx: Vec<u32> = pos.iter().map(|p| *seen.entry(p.map(f32::to_bits)).or_insert_with(|| { verts.push(*p); verts.len() as u32 - 1 })).collect();
    (verts, idx.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect())
}
