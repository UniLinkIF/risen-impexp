//! Dense landscape collision where an edit bends the ground.
//!
//! The shipped collision sectors (`Levelmesh_Landscape_Snnn_COL._xcom`) are coarser than the render
//! mesh: one collision triangle often spans several render triangles. Moving only the collision
//! vertices (`terrain_col::apply`) keeps each collision triangle flat between its corners, so where
//! an edit changes the slope inside it (a plateau edge, a ramp, a cliff) the collision surface cuts
//! a straight line through the air above the lowered ground, or through the raised ground: players
//! walk in the air or sink in. Here such collision triangles are replaced by the edited render
//! triangles that cover them, re-cooked with the cooker (`cook.rs`); everything else in the sector
//! stays (moved by the offset as before), surfaces (shape materials) are kept.
//!
//! ```text
//! candidate  collision triangle whose XZ box meets a render triangle with a moved corner (or a
//!            removed triangle of a topology edit)
//! replaced   candidate where, at ~50 cm samples, the patched coarse surface (corner offsets
//!            interpolated linearly) lies more than THRESHOLD_CM off the edited render surface, or
//!            that overlaps a removed triangle; samples over no render triangle at all (under the
//!            town's shell) keep the coarse triangle, so no hole opens where the render mesh has one
//! added      every render triangle of the edited mesh that overlaps a replaced triangle in XZ with
//!            positive area (separating axis test) — together they cover it completely
//! ```

use crate::nxs::{ShapeMaterial, Xcom, SHAPE_MATERIALS};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap};

type V3 = [f32; 3];
type P2 = [f32; 2];

/// Largest disagreement (cm) between the moved coarse collision and the edited ground before a
/// collision triangle is rebuilt from the render triangles.
pub const THRESHOLD_CM: f32 = 5.0;
/// Sample spacing inside a collision triangle (cm).
const SAMPLE_CM: f32 = 50.0;
const CELL: f32 = 1000.0;

/// XZ triangles hashed by their boxes.
pub struct TriGrid { map: HashMap<(i32, i32), Vec<u32>> }

impl TriGrid {
    pub fn new(tris: &[[P2; 3]], keep: &dyn Fn(usize) -> bool) -> TriGrid {
        let mut map: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (t, tri) in tris.iter().enumerate() {
            if !keep(t) { continue; }
            let (lo, hi) = bbox(tri);
            for cx in cell(lo[0])..=cell(hi[0]) { for cz in cell(lo[1])..=cell(hi[1]) { map.entry((cx, cz)).or_default().push(t as u32); } }
        }
        TriGrid { map }
    }
    /// Triangles whose cells meet the box (each once, ascending).
    pub fn query(&self, lo: P2, hi: P2) -> Vec<u32> {
        let mut out = vec![];
        for cx in cell(lo[0])..=cell(hi[0]) { for cz in cell(lo[1])..=cell(hi[1]) { if let Some(l) = self.map.get(&(cx, cz)) { out.extend_from_slice(l); } } }
        out.sort_unstable();
        out.dedup();
        out
    }
    pub fn is_empty(&self) -> bool { self.map.is_empty() }
}

fn cell(x: f32) -> i32 { (x / CELL).floor() as i32 }
pub fn bbox(t: &[P2; 3]) -> (P2, P2) {
    let lo = [t[0][0].min(t[1][0]).min(t[2][0]), t[0][1].min(t[1][1]).min(t[2][1])];
    let hi = [t[0][0].max(t[1][0]).max(t[2][0]), t[0][1].max(t[1][1]).max(t[2][1])];
    (lo, hi)
}
fn boxes_meet(a: (P2, P2), b: (P2, P2)) -> bool { a.0[0] <= b.1[0] && b.0[0] <= a.1[0] && a.0[1] <= b.1[1] && b.0[1] <= a.1[1] }

/// Two XZ triangles overlap with positive area (touching along an edge or at a corner does not count).
pub fn overlap(a: &[P2; 3], b: &[P2; 3]) -> bool { !separated(a, b, 0.01) }

/// Two XZ triangles meet, touching included (a wall seen from above is a sliver: it still meets
/// the ground it stands on).
pub fn meet(a: &[P2; 3], b: &[P2; 3]) -> bool { !separated(a, b, -0.5) }

/// Separating-axis test in XZ: an edge normal of either triangle along which they lie more than
/// `eps` cm (eps > 0: touching counts as apart; eps < 0: a gap that small still counts as touching).
fn separated(a: &[P2; 3], b: &[P2; 3], eps: f32) -> bool {
    for tri in [a, b] {
        for k in 0..3 {
            let (p, q) = (tri[k], tri[(k + 1) % 3]);
            let n = [q[1] - p[1], p[0] - q[0]];
            if n[0] == 0.0 && n[1] == 0.0 { continue; }
            let proj = |t: &[P2; 3]| { let v = t.map(|s| s[0] * n[0] + s[1] * n[1]); (v[0].min(v[1]).min(v[2]), v[0].max(v[1]).max(v[2])) };
            let (a0, a1) = proj(a);
            let (b0, b1) = proj(b);
            let scale = (n[0] * n[0] + n[1] * n[1]).sqrt();
            if a1 <= b0 + eps * scale || b1 <= a0 + eps * scale { return true; }
        }
    }
    false
}

/// (u, v) barycentric weights of (x, z) in an XZ triangle, None when degenerate.
fn bary(t: &[P2; 3], p: P2) -> Option<[f32; 3]> {
    let d = (t[1][1] - t[2][1]) * (t[0][0] - t[2][0]) + (t[2][0] - t[1][0]) * (t[0][1] - t[2][1]);
    if d.abs() < 1e-6 { return None; }
    let wa = ((t[1][1] - t[2][1]) * (p[0] - t[2][0]) + (t[2][0] - t[1][0]) * (p[1] - t[2][1])) / d;
    let wb = ((t[2][1] - t[0][1]) * (p[0] - t[2][0]) + (t[0][0] - t[2][0]) * (p[1] - t[2][1])) / d;
    Some([wa, wb, 1.0 - wa - wb])
}

/// The edited render mesh, hashed for the collision plans of every sector.
pub struct Dense {
    /// Render triangles (file indices into the positions) of the edited mesh.
    pub tris: Vec<[u32; 3]>,

    pub new: Vec<V3>,
    xz: Vec<[P2; 3]>,
    all: TriGrid,
    changed: TriGrid,
    holes: Vec<[P2; 3]>,
    hole_grid: TriGrid,
}

/// What to do with one sector: which collision triangles go (per stream), which render triangles
/// come in, and for each added one the stream/triangle it replaces (for its surface).
pub struct Plan { pub remove: Vec<Vec<bool>>, pub add: Vec<u32>, pub add_from: Vec<(usize, usize)>, pub removed: usize, pub worst_cm: f32 }

impl Dense {
    /// `holes`: XZ triangles (cm) of ground removed by a topology edit; their collision always goes.
    pub fn new(old: &[V3], new: &[V3], tris: &[[u32; 3]], holes: Vec<[P2; 3]>) -> Dense {
        let xz: Vec<[P2; 3]> = tris.iter().map(|t| t.map(|i| [new[i as usize][0], new[i as usize][2]])).collect();
        let all = TriGrid::new(&xz, &|_| true);
        let changed = TriGrid::new(&xz, &|t| tris[t].iter().any(|&i| old[i as usize][1] != new[i as usize][1]));
        let hole_grid = TriGrid::new(&holes, &|_| true);
        Dense { tris: tris.to_vec(), new: new.to_vec(), xz, all, changed, holes, hole_grid }
    }

    pub fn nothing_changed(&self) -> bool { self.changed.is_empty() && self.hole_grid.is_empty() }

    /// Height (cm) of the edited render surface under (x, z) nearest to `near` (overhangs have several), or None.
    pub fn render_height(&self, p: P2, near: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        for t in self.all.query(p, p) {
            let tri = &self.xz[t as usize];
            let Some(w) = bary(tri, p) else { continue };
            if w.iter().any(|&x| x < -1e-4) { continue; }
            let y: f32 = (0..3).map(|k| w[k] * self.new[self.tris[t as usize][k] as usize][1]).sum();
            if best.map_or(true, |b| (y - near).abs() < (b - near).abs()) { best = Some(y); }
        }
        best
    }

    /// Distance (cm) from a point to the nearest edited render triangle within `reach` cm in XZ, or None.
    pub fn distance(&self, q: V3, reach: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        for t in self.all.query([q[0] - reach, q[2] - reach], [q[0] + reach, q[2] + reach]) {
            let tri = self.tris[t as usize].map(|i| self.new[i as usize]);
            let d = point_triangle(q, tri);
            if best.map_or(true, |b| d < b) { best = Some(d); }
        }
        best
    }

    /// The highest render triangle (edited mesh) under (x, z): its index, or None.
    #[allow(dead_code)]
    fn render_at(&self, p: P2) -> Option<u32> {
        let mut best: Option<(f32, u32)> = None;
        for t in self.all.query(p, p) {
            let tri = &self.xz[t as usize];
            let Some(w) = bary(tri, p) else { continue };
            if w.iter().any(|&x| x < -1e-4) { continue; }
            let y: f32 = (0..3).map(|k| w[k] * self.new[self.tris[t as usize][k] as usize][1]).sum();
            if best.map_or(true, |(by, _)| y > by) { best = Some((y, t)); }
        }
        best.map(|b| b.1)
    }

    /// The plan for a sector at `p` (cm) holding `x`, with `dy(x, y, z)` (world cm → cm) the offset
    /// field the coarse patch uses. None: the coarse patch is good enough everywhere here.
    pub fn plan(&self, x: &Xcom, p: V3, dy: &dyn Fn(f32, f32, f32) -> f32) -> Option<Plan> {
        if self.nothing_changed() { return None; }
        let mut remove: Vec<Vec<bool>> = x.meshes.iter().map(|m| vec![false; m.tris.len()]).collect();
        let mut removed = 0;
        let mut worst = 0f32;
        let mut gone: Vec<(usize, usize, [P2; 3])> = vec![];
        for (k, m) in x.meshes.iter().enumerate() {
            for (i, t) in m.tris.iter().enumerate() {
                let w3: [V3; 3] = t.map(|v| { let q = m.verts[v as usize]; [p[0] + q[0] * 100.0, p[1] + q[1] * 100.0, p[2] + q[2] * 100.0] });
                let xz: [P2; 3] = w3.map(|q| [q[0], q[2]]);
                let bb = bbox(&xz);
                let hole_hit = self.hole_grid.query(bb.0, bb.1).into_iter().any(|h| overlap(&xz, &self.holes[h as usize]));
                let mut bad = hole_hit;
                if !bad {
                    let near = self.changed.query(bb.0, bb.1);
                    if !near.iter().any(|&r| boxes_meet(bb, bbox(&self.xz[r as usize]))) { continue; }
                    let d3 = w3.map(|q| dy(q[0], q[1], q[2]));
                    let edge = (0..3).map(|e| { let (a, b) = (xz[e], xz[(e + 1) % 3]); ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt() }).fold(0f32, f32::max);
                    let n = ((edge / SAMPLE_CM).ceil() as usize).clamp(1, 96);
                    let mut over_nothing = false;
                    let mut miss = 0f32;
                    for a in 0..=n {
                        for b in 0..=n - a {
                            let w = [a as f32 / n as f32, b as f32 / n as f32, (n - a - b) as f32 / n as f32];
                            let q: V3 = [0, 1, 2].map(|c| w[0] * w3[0][c] + w[1] * w3[1][c] + w[2] * w3[2][c]);
                            // The patched coarse surface here against the edited ground itself.
                            let coarse = q[1] + w[0] * d3[0] + w[1] * d3[1] + w[2] * d3[2];
                            let Some(ground) = self.render_height([q[0], q[2]], coarse) else { over_nothing = true; break };
                            miss = miss.max((coarse - ground).abs());
                        }
                        if over_nothing { break; }
                    }
                    if over_nothing { continue; }
                    worst = worst.max(miss);
                    bad = miss > THRESHOLD_CM;
                }
                if bad { remove[k][i] = true; removed += 1; gone.push((k, i, xz)); }
            }
        }
        if removed == 0 { return None; }
        let mut add_from: BTreeMap<u32, (usize, usize)> = BTreeMap::new();
        for (k, i, xz) in &gone {
            let bb = bbox(xz);
            for r in self.all.query(bb.0, bb.1) {
                if add_from.contains_key(&r) { continue; }
                if meet(xz, &self.xz[r as usize]) { add_from.insert(r, (*k, *i)); }
            }
        }
        let (add, from): (Vec<u32>, Vec<(usize, usize)>) = add_from.into_iter().unzip();
        Some(Plan { remove, add, add_from: from, removed, worst_cm: worst })
    }
}

/// A sector re-cooked: its triangles regrouped by shape material (one stream each, ordered by the
/// material's name as the shipped sectors are), heights moved by `off` (metres, local), the
/// triangles `remove[stream][i]` left out and `extra` triangles (local metres, shape) added with
/// the per-triangle material most common in the sector.
pub fn rebuild(x: &Xcom, shapes: &[Vec<u8>], off: &dyn Fn(f32, f32, f32) -> f32, remove: Option<&[Vec<bool>]>, extra: &[([V3; 3], u8)]) -> Result<Vec<u8>> {
    let mut groups: BTreeMap<&str, (u8, Vec<V3>, Vec<[u32; 3]>, Vec<u16>)> = Default::default();
    let mut mat_count: HashMap<u16, usize> = HashMap::new();
    for (si, m) in x.meshes.iter().enumerate() {
        let verts: Vec<V3> = m.verts.iter().map(|v| [v[0], v[1] + off(v[0], v[1], v[2]), v[2]]).collect();
        for (ti, t) in m.tris.iter().enumerate() {
            let mat = m.materials.as_ref().and_then(|ms| ms.get(ti).copied()).unwrap_or(0);
            *mat_count.entry(mat).or_default() += 1;
            if remove.map_or(false, |r| r[si][ti]) { continue; }
            let s = shapes[si][ti];
            let e = groups.entry(SHAPE_MATERIALS.get(s as usize).copied().unwrap_or("none")).or_insert((s, vec![], vec![], vec![]));
            let base = e.1.len() as u32;
            for &i in t { e.1.push(verts[i as usize]); }
            e.2.push([base, base + 1, base + 2]);
            e.3.push(mat);
        }
    }
    let common = mat_count.into_iter().max_by_key(|(m, n)| (*n, std::cmp::Reverse(*m))).map(|(m, _)| m).unwrap_or(0);
    for (tri, s) in extra {
        let e = groups.entry(SHAPE_MATERIALS.get(*s as usize).copied().unwrap_or("none")).or_insert((*s, vec![], vec![], vec![]));
        let base = e.1.len() as u32;
        e.1.extend_from_slice(tri);
        e.2.push([base, base + 1, base + 2]);
        e.3.push(common);
    }
    let mut meshes = vec![];
    let mut shape_materials = vec![];
    let mut boxes = vec![];
    for (s, verts, tris, mats) in groups.values() {
        let mesh = crate::cook::cook(verts, tris, mats)?;
        boxes.extend(crate::gr01::bbox_bytes([0, 1, 2].map(|k| mesh.aabb[k] * 100.0 - 0.01), [0, 1, 2].map(|k| mesh.aabb[3 + k] * 100.0 + 0.01)));
        meshes.push(mesh);
        shape_materials.push(x.shape_materials.iter().find(|o| o.material == *s).copied().unwrap_or(ShapeMaterial { material: *s, ignored_by_trace_ray: 0, no_collision: 0, no_response: 0 }));
    }
    let mut resource = x.resource.clone();
    let sb = resource.section.root.props.iter_mut().find(|p| p.name == "SubBoundaries").context("collision sector without SubBoundaries")?;
    let crate::gr01::Value::Raw(raw) = &mut sb.value else { anyhow::bail!("SubBoundaries is not raw") };
    let head = raw.first().copied().unwrap_or(1);
    let mut v = vec![head];
    v.extend((meshes.len() as u32).to_le_bytes());
    v.extend(boxes);
    *raw = v;
    Ok(crate::nxs::write_xcom(&Xcom { resource, meshes, shape_materials }))
}

/// Distance from p to triangle abc (closest point by Voronoi regions, Ericson 5.1.5).
pub fn point_triangle(p: V3, [a, b, c]: [V3; 3]) -> f32 {
    let sub = |x: V3, y: V3| [x[0] - y[0], x[1] - y[1], x[2] - y[2]];
    let dot = |x: V3, y: V3| x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
    let len = |x: V3| dot(x, x).sqrt();
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 { return len(ap); }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 { return len(bp); }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 { let v = d1 / (d1 - d3); return len(sub(p, [a[0] + ab[0] * v, a[1] + ab[1] * v, a[2] + ab[2] * v])); }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 { return len(cp); }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 { let w = d2 / (d2 - d6); return len(sub(p, [a[0] + ac[0] * w, a[1] + ac[1] * w, a[2] + ac[2] * w])); }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        let bc = sub(c, b);
        return len(sub(p, [b[0] + bc[0] * w, b[1] + bc[1] * w, b[2] + bc[2] * w]));
    }
    let den = 1.0 / (va + vb + vc);
    let (v, w) = (vb * den, vc * den);
    len(sub(p, [a[0] + ab[0] * v + ac[0] * w, a[1] + ab[1] * v + ac[1] * w, a[2] + ab[2] * v + ac[2] * w]))
}

/// +1 when a sector's collision triangles face up as (b−a)×(c−a), −1 when as (c−a)×(b−a).
pub fn up_winding(x: &Xcom) -> f32 {
    let mut s = 0f64;
    for m in &x.meshes {
        for t in &m.tris {
            let [a, b, c] = t.map(|i| m.verts[i as usize]);
            let (u, v) = ([b[0] - a[0], b[2] - a[2]], [c[0] - a[0], c[2] - a[2]]);
            // y of (b−a)×(c−a) = uz·vx − ux·vz
            s += (u[1] * v[0] - u[0] * v[1]) as f64;
        }
    }
    if s >= 0.0 { 1.0 } else { -1.0 }
}

/// Collision surface heights for checks: every collision triangle (world cm) of a set of sectors.
pub struct Surface { pub tris: Vec<[V3; 3]>, grid: TriGrid }

impl Surface {
    pub fn new(tris: Vec<[V3; 3]>) -> Surface {
        let xz: Vec<[P2; 3]> = tris.iter().map(|t| t.map(|q| [q[0], q[2]])).collect();
        let grid = TriGrid::new(&xz, &|_| true);
        Surface { tris, grid }
    }
    pub fn from_xcom(x: &Xcom, p: V3, out: &mut Vec<[V3; 3]>) {
        for m in &x.meshes { for t in &m.tris { out.push(t.map(|i| { let q = m.verts[i as usize]; [p[0] + q[0] * 100.0, p[1] + q[1] * 100.0, p[2] + q[2] * 100.0] })); } }
    }
    /// Points on the triangles whose XZ centre passes `keep` (only those not steeper than 60° when
    /// `walkable_only`): about one per `step` cm along the longest edge, at most 20 along it.
    pub fn samples(&self, step: f32, keep: &dyn Fn(f32, f32) -> bool, walkable_only: bool) -> Vec<V3> {
        let mut out = vec![];
        for t in &self.tris {
            let c = [0, 2].map(|k| (t[0][k] + t[1][k] + t[2][k]) / 3.0);
            if !keep(c[0], c[1]) { continue; }
            let (u, v) = ([t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]], [t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]]);
            let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len == 0.0 || (walkable_only && n[1].abs() < 0.5 * len) { continue; }
            let edge = (0..3).map(|e| { let (a, b) = (t[e], t[(e + 1) % 3]); ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt() }).fold(0f32, f32::max);
            let n = ((edge / step).ceil() as usize).clamp(1, 19);
            for a in 0..=n { for b in 0..=n - a {
                let w = [a as f32 / n as f32, b as f32 / n as f32, (n - a - b) as f32 / n as f32];
                // Stay a hair inside, so a point on a shared edge is not tested against the wrong side.
                let w = w.map(|x| x * 0.998 + 0.000_666_7);
                out.push([0, 1, 2].map(|k| w[0] * t[0][k] + w[1] * t[1][k] + w[2] * t[2][k]));
            } }
        }
        out
    }

    /// The collision height at (x, z) nearest to `y` (cm), if any triangle lies under the point.
    pub fn height_near(&self, x: f32, z: f32, y: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        for t in self.grid.query([x, z], [x, z]) {
            let tri = &self.tris[t as usize];
            let Some(w) = bary(&tri.map(|q| [q[0], q[2]]), [x, z]) else { continue };
            if w.iter().any(|&a| a < -1e-4) { continue; }
            let h = w[0] * tri[0][1] + w[1] * tri[1][1] + w[2] * tri[2][1];
            if best.map_or(true, |b| (h - y).abs() < (b - y).abs()) { best = Some(h); }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_needs_area() {
        let a = [[0.0, 0.0], [100.0, 0.0], [0.0, 100.0]];
        assert!(overlap(&a, &[[10.0, 10.0], [20.0, 10.0], [10.0, 20.0]]));
        // Shares the hypotenuse only.
        assert!(!overlap(&a, &[[100.0, 0.0], [100.0, 100.0], [0.0, 100.0]]));
        // Shares a corner only.
        assert!(!overlap(&a, &[[100.0, 0.0], [200.0, 0.0], [100.0, 50.0]]));
        // Crossing edges, no corner inside the other.
        assert!(overlap(&[[0.0, 40.0], [100.0, 40.0], [50.0, 200.0]], &[[0.0, 60.0], [100.0, 60.0], [50.0, -100.0]]));
    }
}
