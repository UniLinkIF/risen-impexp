//! Trees and bushes. A `._xspt` is a SpeedTree recipe the game grows at run time — there is no mesh
//! in it. What it does declare is its bounding box (cm) and texture names, so each species comes into
//! Blender as a stand-in of the right size and look: a tapered bark trunk plus crossed leaf cards
//! (bushes and marine plants: cards only). It is for placing and building around, not for editing —
//! the game would not read it back as a tree.
//!
//! Leaves: the per-species leaf names in the recipe (`RedOakLeaves_1.tga`, …) are not files — they
//! are baked into the shared atlas `ST_Composite_01_Diffuse_01`, and which cell is which species is
//! not stored anywhere. One hand-picked atlas cell per leaf family is cropped, tightened to its alpha
//! and binarised (the atlas alpha is noise between 80 and 255 for SpeedTree's fade).

use crate::game::GameCtx;
use crate::mesh::TextureIndex;
use anyhow::{anyhow, Result};
use std::fmt::Write as _;
use std::path::Path;

pub const LEAF_ATLAS: &str = "ST_Composite_01_Diffuse_01";
const ALPHA_ON: u8 = 64;

/// Leaf family: first key (lowercase substring of a texture name in the recipe) that matches picks the
/// atlas cell `[x, y, w, h]` (px in the 1024×2048 atlas).
const LEAF_FAMILIES: &[(&str, &str, [u32; 4])] = &[
    ("douglasfirneedles", "fir", [0, 1536, 512, 512]),
    ("easternredcedarneedles", "cedar", [0, 1024, 512, 512]),
    ("pinoak", "pinoak", [512, 1152, 256, 256]),
    ("curlypalm", "palm", [512, 128, 256, 512]),
    ("clippedfrond", "palm", [512, 128, 256, 512]),
    ("azaleaflowers", "azaleaflower", [256, 768, 256, 256]),
    ("azalealeaves", "azalea", [0, 768, 256, 256]),
    ("banyan", "banyan", [768, 768, 256, 256]),
    ("bamboo", "bamboo", [768, 1280, 256, 256]),
    ("boxwood", "boxwood", [512, 1408, 256, 256]),
    ("mimosa", "mimosa", [512, 1792, 192, 256]),
    ("japanesemaple", "maple", [512, 640, 256, 256]),
    ("redoak", "redoak", [256, 512, 256, 256]),
    ("lilypad", "lilypad", [128, 384, 256, 128]),
];
const GENERIC_LEAF: (&str, [u32; 4]) = ("generic", [512, 896, 256, 256]);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Form { Tree, Palm, Bush }

pub fn form_of(stem: &str) -> Form {
    let s = stem.to_lowercase();
    if s.starts_with("st_bush") || s.starts_with("st_marine") { Form::Bush } else if s.contains("palm") { Form::Palm } else { Form::Tree }
}

pub fn leaf_family(textures: &[String]) -> (&'static str, [u32; 4]) {
    let names: Vec<String> = textures.iter().map(|t| t.to_lowercase()).filter(|t| !t.contains("normal") && !t.starts_with("st_")).collect();
    LEAF_FAMILIES.iter().find(|(k, _, _)| names.iter().any(|n| n.contains(k))).map(|(_, f, r)| (*f, *r)).unwrap_or(GENERIC_LEAF)
}

fn bark(textures: &[String], what: &str) -> Option<String> {
    textures.iter().find(|t| { let l = t.to_lowercase(); l.starts_with("st_bark_") && l.contains(what) }).map(|t| crate::mesh::key(t))
}

type V3 = [f32; 3];
fn sub(a: V3, b: V3) -> V3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn dot(a: V3, b: V3) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn cross(a: V3, b: V3) -> V3 { [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]] }

/// Stand-in geometry in game space (cm, Y up, pivot at the foot), wound like the game's own static
/// meshes (`cross(b - a, c - a)` against the face normal), Direct3D V.
#[derive(Default)]
pub struct Geometry { pub pos: Vec<V3>, pub nrm: Vec<V3>, pub uv: Vec<[f32; 2]>, pub trunk: Vec<[u32; 3]>, pub crown: Vec<[u32; 3]> }

impl Geometry {
    fn vert(&mut self, p: V3, n: V3, uv: [f32; 2]) -> u32 { self.pos.push(p); self.nrm.push(n); self.uv.push(uv); self.pos.len() as u32 - 1 }
    fn tri(&self, [a, b, c]: [u32; 3], n: V3) -> [u32; 3] {
        let p = |i: u32| self.pos[i as usize];
        if dot(cross(sub(p(b), p(a)), sub(p(c), p(a))), n) <= 0.0 { [a, b, c] } else { [a, c, b] }
    }
    /// A two-sided card: corners top-left, top-right, bottom-right, bottom-left.
    fn card(&mut self, c: [V3; 4], n: V3) {
        let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for side in [n, [-n[0], -n[1], -n[2]]] {
            let v: Vec<u32> = (0..4).map(|i| self.vert(c[i], side, uv[i])).collect();
            let (t0, t1) = (self.tri([v[0], v[1], v[2]], side), self.tri([v[0], v[2], v[3]], side));
            self.crown.extend([t0, t1]);
        }
    }
}

pub const TRUNK_SIDES: usize = 10;

pub fn geometry((min, max): (V3, V3), form: Form) -> Geometry {
    let mut g = Geometry::default();
    let h = (max[1] - min[1]).max(1.0);
    let (cx, cz) = ((min[0] + max[0]) * 0.5, (min[2] + max[2]) * 0.5);
    let (hx, hz) = (((max[0] - min[0]) * 0.5).max(10.0), ((max[2] - min[2]) * 0.5).max(10.0));
    let crown_base = match form { Form::Bush => min[1], Form::Palm => min[1] + 0.62 * h, Form::Tree => min[1] + 0.3 * h };
    if form != Form::Bush {
        // No trunk radius in the recipe: ~2.5 % of the height, 6..90 cm, tapering to 40 %.
        let r0 = (0.025 * h).clamp(6.0, 90.0);
        let r1 = 0.4 * r0;
        let (y0, y1) = (min[1], if form == Form::Palm { max[1] - 0.12 * h } else { crown_base + 0.5 * (max[1] - crown_base) });
        let circ = std::f32::consts::TAU * r0;
        let slope = (r0 - r1) / (y1 - y0).max(1.0);
        let mut ring = vec![];
        for i in 0..=TRUNK_SIDES {
            let a = i as f32 / TRUNK_SIDES as f32 * std::f32::consts::TAU;
            let (s, c) = a.sin_cos();
            let n = { let l = (1.0 + slope * slope).sqrt(); [c / l, slope / l, s / l] };
            let u = i as f32 / TRUNK_SIDES as f32;
            // Bark textures are 1:4: one wrap round the base, V repeating every 4 circumferences.
            let lo = g.vert([c * r0, y0, s * r0], n, [u, 0.0]);
            let hi = g.vert([c * r1, y1, s * r1], n, [u, -(y1 - y0) / (4.0 * circ)]);
            ring.push((lo, hi, n));
        }
        for i in 0..TRUNK_SIDES {
            let ((l0, h0, n0), (l1, h1, n1)) = (ring[i], ring[i + 1]);
            let n = [(n0[0] + n1[0]) * 0.5, n0[1], (n0[2] + n1[2]) * 0.5];
            let (t0, t1) = (g.tri([l0, h0, h1], n), g.tri([l0, h1, l1], n));
            g.trunk.extend([t0, t1]);
        }
    }
    let top = max[1];
    for k in 0..3 {
        let (s, c) = (k as f32 * std::f32::consts::PI / 3.0).sin_cos();
        let (dx, dz) = (c * hx, s * hz);
        let n = { let l = (dx * dx + dz * dz).sqrt().max(1e-3); [-dz / l, 0.0, dx / l] };
        g.card([[cx - dx, top, cz - dz], [cx + dx, top, cz + dz], [cx + dx, crown_base, cz + dz], [cx - dx, crown_base, cz - dz]], n);
    }
    if form != Form::Bush {
        let y = crown_base + 0.55 * (top - crown_base);
        g.card([[cx - hx, y, cz + hz], [cx + hx, y, cz + hz], [cx + hx, y, cz - hz], [cx - hx, y, cz - hz]], [0.0, 1.0, 0.0]);
    }
    g
}

/// The leaf atlas cell of one family, alpha binarised: `(png relative to out, mean colour, has alpha)`.
fn leaf_png(g: &GameCtx, tex: &TextureIndex, out: &Path, family: &str, rect: [u32; 4]) -> Result<(Option<String>, V3)> {
    let rel = format!("textures/st_leaf_{family}.png");
    let atlas_rel = tex.texture(g, LEAF_ATLAS, out, false)?.ok_or_else(|| anyhow!("{LEAF_ATLAS} is not in the game archives"))?;
    let atlas = image::open(out.join(&atlas_rel))?.to_rgba8();
    let (aw, ah) = atlas.dimensions();
    let [x, y, w, h] = rect;
    let (x1, y1) = ((x + w).min(aw), (y + h).min(ah));
    let has_alpha = atlas.pixels().filter(|p| p.0[3] < 16).count() * 20 > (aw * ah) as usize;
    let (mut bx0, mut by0, mut bx1, mut by1, mut sum, mut n) = (x1, y1, x, y, [0f64; 3], 0f64);
    for yy in y..y1 {
        for xx in x..x1 {
            let p = atlas.get_pixel(xx, yy).0;
            if !has_alpha || p[3] >= ALPHA_ON {
                bx0 = bx0.min(xx); by0 = by0.min(yy); bx1 = bx1.max(xx + 1); by1 = by1.max(yy + 1);
                for i in 0..3 { sum[i] += p[i] as f64; }
                n += 1.0;
            }
        }
    }
    if n == 0.0 { (bx0, by0, bx1, by1) = (x, y, x1, y1); n = 1.0; }
    let colour = sum.map(|s| (s / n / 255.0) as f32);
    if !has_alpha { return Ok((None, colour)); }
    let (px0, py0, px1, py1) = (bx0.saturating_sub(2).max(x), by0.saturating_sub(2).max(y), (bx1 + 2).min(x1), (by1 + 2).min(y1));
    let mut crop = image::imageops::crop_imm(&atlas, px0, py0, px1 - px0, py1 - py0).to_image();
    for p in crop.pixels_mut() { p.0[3] = if p.0[3] >= ALPHA_ON { 255 } else { 0 }; }
    std::fs::create_dir_all(out.join("textures"))?;
    crop.save_with_format(out.join(&rel), image::ImageFormat::Png)?;
    Ok((Some(rel), colour))
}

#[derive(serde::Serialize)]
pub struct TreeOut { pub entry: String, pub obj: String, pub form: String, pub height_cm: f32, pub warnings: Vec<String> }

/// `<stem>.obj/.mtl` in `out` for a SpeedTree by name (`ST_Tree_XL_RedOak_01`) or entry, in the same
/// OBJ convention as the static models (game mirrored in Z, V up).
pub fn export_obj(g: &GameCtx, tex: &TextureIndex, name: &str, out: &Path) -> Result<TreeOut> {
    let entry = if name.starts_with('/') { name.to_string() } else { g.find_one(&format!("/{}._xspt", name.trim_end_matches("._xspt")))? };
    let stem = entry.rsplit('/').next().unwrap().split('.').next().unwrap().to_string();
    let info = risen_formats::xspt::parse(&g.read(&entry)?.0).ok_or_else(|| anyhow!("{entry}: not an ._xspt"))?;
    let bounds = info.bounds.ok_or_else(|| anyhow!("{entry}: declares no bounding box"))?;
    let form = form_of(&stem);
    let geo = geometry(bounds, form);
    let mut warnings = vec![];
    let (family, rect) = leaf_family(&info.textures);
    let (leaf, colour) = leaf_png(g, tex, out, family, rect)?;
    if leaf.is_none() { warnings.push(format!("{stem}: the leaf atlas has no alpha; the crown is a leaf-coloured card")); }
    let (bark_d, bark_n) = match bark(&info.textures, "diffuse") {
        Some(d) => (tex.texture(g, &d, out, false)?, bark(&info.textures, "normal").and_then(|n| tex.texture(g, &n, out, true).ok().flatten())),
        None => (None, None),
    };
    if form != Form::Bush && bark_d.is_none() { warnings.push(format!("{stem}: no bark texture; the trunk is plain brown")); }

    let mut obj = format!("# {entry} (SpeedTree stand-in)\nmtllib {stem}.mtl\n");
    for p in &geo.pos { writeln!(obj, "v {} {} {}", p[0], p[1], -p[2])?; }
    for t in &geo.uv { writeln!(obj, "vt {} {}", t[0], 1.0 - t[1])?; }
    for n in &geo.nrm { writeln!(obj, "vn {} {} {}", n[0], n[1], -n[2])?; }
    let mut mtl = String::new();
    for (group, tris) in [("trunk", &geo.trunk), ("crown", &geo.crown)] {
        if tris.is_empty() { continue; }
        let mat = format!("{stem}_{group}");
        writeln!(obj, "g {group}\nusemtl {mat}")?;
        for t in tris { let c = |i: u32| { let k = i + 1; format!("{k}/{k}/{k}") }; writeln!(obj, "f {} {} {}", c(t[0]), c(t[1]), c(t[2]))?; }
        writeln!(mtl, "newmtl {mat}")?;
        match group {
            "trunk" => {
                match &bark_d { Some(d) => writeln!(mtl, "Kd 1 1 1\nmap_Kd {d}")?, None => writeln!(mtl, "Kd 0.33 0.25 0.18")? }
                if let Some(n) = &bark_n { writeln!(mtl, "map_Bump {n}")?; }
            }
            _ => match &leaf { Some(l) => writeln!(mtl, "Kd 1 1 1\nmap_Kd {l}\nmap_d {l}")?, None => writeln!(mtl, "Kd {} {} {}", colour[0], colour[1], colour[2])? },
        }
    }
    std::fs::create_dir_all(out)?;
    let path = out.join(format!("{stem}.obj"));
    std::fs::write(path.with_extension("mtl"), mtl)?;
    std::fs::write(&path, obj)?;
    let form = match form { Form::Tree => "tree", Form::Palm => "palm", Form::Bush => "bush" }.to_string();
    Ok(TreeOut { entry, obj: path.to_string_lossy().into_owned(), form, height_cm: bounds.1[1] - bounds.0[1], warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stand_in_fills_its_bounds_and_winds_like_game_meshes() {
        let g = geometry(([-600.0, 0.0, -300.0], [400.0, 2000.0, 500.0]), Form::Tree);
        let ys = g.pos.iter().map(|p| p[1]);
        let (lo, hi) = (ys.clone().fold(f32::MAX, f32::min), ys.fold(f32::MIN, f32::max));
        assert!((lo - 0.0).abs() < 1e-3 && (hi - 2000.0).abs() < 1e-3);
        assert_eq!(g.trunk.len(), 2 * TRUNK_SIDES);
        assert_eq!(g.crown.len(), 16);
        for t in g.trunk.iter().chain(&g.crown) {
            let p = |i: u32| g.pos[i as usize];
            assert!(dot(cross(sub(p(t[1]), p(t[0])), sub(p(t[2]), p(t[0]))), g.nrm[t[0] as usize]) < 0.0);
        }
        let b = geometry(([-300.0, -120.0, -300.0], [300.0, 400.0, 280.0]), Form::Bush);
        assert!(b.trunk.is_empty() && !b.crown.is_empty());
    }

    #[test]
    fn real_tree_comes_with_bark_and_leaves() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        let out = std::env::temp_dir().join(format!("rc-tree-{}", std::process::id()));
        let r = export_obj(&g, &TextureIndex::build(&g), "ST_Tree_XL_RedOak_01", &out).unwrap();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let mtl = std::fs::read_to_string(std::path::Path::new(&r.obj).with_extension("mtl")).unwrap();
        assert!(mtl.contains("map_Kd textures/st_bark") && mtl.contains("map_d textures/st_leaf_redoak.png"), "{mtl}");
        let _ = std::fs::remove_dir_all(&out);
    }
}
