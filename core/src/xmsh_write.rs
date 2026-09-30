//! Writing a Risen 1 static mesh (`._xmsh`), in the shape the shipped item meshes have:
//!
//! * one vertex format, 52 bytes: position, normal, tangent (FLOAT3 each), two D3DCOLORs, uv0; plus
//!   the stream-3 instancing element every item mesh declares;
//! * colour 0 = ff ff <s> ff with s = 255 for a right-handed tangent frame, 0 otherwise; colour 1 =
//!   00 00 00 ff;
//! * INDEX32 triangle lists, clockwise front faces (Direct3D), indices absolute; each submesh owns a
//!   contiguous vertex range and a contiguous index range, in submesh order;
//! * each submesh's bounding-volume tree is a single node (its box, first 0, size 1), as in 523
//!   of the 1666 shipped meshes;
//! * one trailing byte: 1 = instancing allowed (0 when the name contains "noinstancing").
//!
//! Layout of the data part: `xmsh_geom.rs`.

use crate::gr01::{bbox_bytes, Body, Prop, Resource, Section, Value};

/// The shipped item declaration (It_Wpn_2H_Berserk and 750 other files), 128 bytes with two D3DDECL_END.
const DECLARATION: [u8; 128] = {
    let head: [u8; 64] = [
        0, 0, 0, 0, 2, 0, 0, 0, // stream 0 @0  FLOAT3 POSITION
        0, 0, 12, 0, 2, 0, 3, 0, // @12 FLOAT3 NORMAL
        0, 0, 24, 0, 2, 0, 6, 0, // @24 FLOAT3 TANGENT
        0, 0, 36, 0, 4, 0, 10, 0, // @36 D3DCOLOR COLOR0
        0, 0, 40, 0, 4, 0, 10, 1, // @40 D3DCOLOR COLOR1
        0, 0, 44, 0, 1, 0, 5, 0, // @44 FLOAT2 TEXCOORD0
        3, 0, 0, 0, 0, 0, 5, 4, // stream 3 @0 FLOAT1 TEXCOORD4 (instancing)
        0xff, 0, 0, 0, 17, 0, 0, 0, // D3DDECL_END
    ];
    let mut d = [0u8; 128];
    let mut i = 0;
    while i < 64 { d[i] = head[i]; i += 1; }
    let end: [u8; 8] = [0xff, 0, 0, 0, 17, 0, 0, 0];
    while i < 72 { d[i] = end[i - 64]; i += 1; }
    d
};
const STRIDE: u32 = 52;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex { pub pos: [f32; 3], pub normal: [f32; 3], pub tangent: [f32; 3], pub right_handed: bool, pub uv: [f32; 2] }

/// One submesh: its `._xmat` name (`Foo._xmat`), its vertices and triangles (indices into `vertices`).
pub struct Part { pub material: String, pub vertices: Vec<Vertex>, pub triangles: Vec<[u32; 3]> }

fn bounds<'a>(vs: impl Iterator<Item = &'a Vertex>) -> ([f32; 3], [f32; 3]) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in vs { for k in 0..3 { lo[k] = lo[k].min(v.pos[k]); hi[k] = hi[k].max(v.pos[k]); } }
    if lo[0] > hi[0] { ([0.0; 3], [0.0; 3]) } else { (lo, hi) }
}

fn long(name: &str, v: i32) -> Prop { Prop { name: name.into(), ty: "long".into(), tag: 30, value: Value::Raw(v.to_le_bytes().to_vec()) } }
fn bcbox(name: &str, lo: [f32; 3], hi: [f32; 3]) -> Prop { Prop { name: name.into(), ty: "bCBox".into(), tag: 30, value: Value::Raw(bbox_bytes(lo, hi)) } }
fn string(name: &str, v: &str) -> Prop { Prop { name: name.into(), ty: "bCString".into(), tag: 30, value: Value::Str(v.into()) } }

pub fn write(name: &str, parts: &[Part], filetime: u64) -> Vec<u8> {
    let (mut vbuf, mut ibuf, mut elems) = (vec![], vec![], vec![]);
    let (mut first_vertex, mut first_index) = (0u32, 0u32);
    for p in parts {
        for v in &p.vertices {
            for f in v.pos.iter().chain(&v.normal).chain(&v.tangent) { vbuf.extend(f.to_le_bytes()); }
            vbuf.extend([0xff, 0xff, if v.right_handed { 0xff } else { 0 }, 0xff, 0, 0, 0, 0xff]);
            for f in v.uv { vbuf.extend(f.to_le_bytes()); }
        }
        for t in &p.triangles { for i in t { ibuf.extend((i + first_vertex).to_le_bytes()); } }
        let (lo, hi) = bounds(p.vertices.iter());
        let index_count = p.triangles.len() as u32 * 3;
        let mut tree = 201u16.to_le_bytes().to_vec();
        tree.extend(1u32.to_le_bytes());
        tree.extend(bbox_bytes(lo, hi));
        tree.extend(0u32.to_le_bytes());
        tree.extend(1u32.to_le_bytes());
        elems.push(Body {
            props: vec![string("MaterialName", &p.material), bcbox("Extends", lo, hi), long("FirstIndex", first_index as i32), long("IndexCount", index_count as i32), long("FirstVertex", first_vertex as i32), long("VertexCount", p.vertices.len() as i32)],
            rest: tree,
        });
        first_vertex += p.vertices.len() as u32;
        first_index += index_count;
    }
    let (lo, hi) = bounds(parts.iter().flat_map(|p| p.vertices.iter()));
    let root = Body {
        props: vec![
            Prop { name: "SubMeshes".into(), ty: "bTObjArray<class eCSubMesh>".into(), tag: 30, value: Value::Array { head: 1, elems } },
            string("LightmapName", ""),
            bcbox("Boundary", lo, hi),
        ],
        rest: 201u16.to_le_bytes().to_vec(),
    };
    let section = Section { prefix: [1, 0, 1, 1, 0, 1], class: "eCMeshResource2".into(), mid: [1, 0, 0], root, tail: vec![] };

    let mut data = DECLARATION.to_vec();
    for v in [vbuf.len() as u32, 0, 0, 1, STRIDE] { data.extend(v.to_le_bytes()); }
    data.extend([0u8; 40]);
    for v in [ibuf.len() as u32, 0, 102, 1] { data.extend(v.to_le_bytes()); }
    data.extend(vbuf);
    data.extend(ibuf);
    data.push(!name.to_lowercase().contains("noinstancing") as u8);
    Resource { magic: *b"GR01MS02", filetime: filetime.to_le_bytes(), reserved: [0; 8], section, data }.write()
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]] }
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn normalize(a: [f32; 3]) -> Option<[f32; 3]> { let l = dot(a, a).sqrt(); (l > 1e-12).then(|| [a[0] / l, a[1] / l, a[2] / l]) }

/// Per-vertex tangents from the UVs (tangent along +u), Gram-Schmidt against the normal; the frame
/// is right-handed when cross(normal, tangent) points along +v.
pub fn compute_tangents(vs: &mut [Vertex], tris: &[[u32; 3]]) {
    let mut t = vec![[0f32; 3]; vs.len()];
    let mut b = vec![[0f32; 3]; vs.len()];
    for tri in tris {
        let [a, c, d] = tri.map(|i| vs[i as usize]);
        let (e1, e2) = (sub(c.pos, a.pos), sub(d.pos, a.pos));
        let (du1, dv1, du2, dv2) = (c.uv[0] - a.uv[0], c.uv[1] - a.uv[1], d.uv[0] - a.uv[0], d.uv[1] - a.uv[1]);
        let r = du1 * dv2 - du2 * dv1;
        if r.abs() < 1e-12 { continue; }
        let tu = [(e1[0] * dv2 - e2[0] * dv1) / r, (e1[1] * dv2 - e2[1] * dv1) / r, (e1[2] * dv2 - e2[2] * dv1) / r];
        let bv = [(e2[0] * du1 - e1[0] * du2) / r, (e2[1] * du1 - e1[1] * du2) / r, (e2[2] * du1 - e1[2] * du2) / r];
        for &i in tri { for k in 0..3 { t[i as usize][k] += tu[k]; b[i as usize][k] += bv[k]; } }
    }
    for (i, v) in vs.iter_mut().enumerate() {
        let n = v.normal;
        let ortho = sub(t[i], { let d = dot(n, t[i]); [n[0] * d, n[1] * d, n[2] * d] });
        let tan = normalize(ortho).unwrap_or_else(|| {
            // No usable UV gradient: any unit vector perpendicular to the normal.
            let axis = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
            normalize(cross(axis, n)).unwrap_or([1.0, 0.0, 0.0])
        });
        v.tangent = tan;
        v.right_handed = dot(cross(n, tan), b[i]) >= 0.0;
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Every vertex of a 52-byte-stride mesh, with the fields this writer emits.
    pub fn read_parts(d: &[u8]) -> Vec<Part> {
        let res = Resource::parse(d).unwrap();
        let data = &res.data;
        let f = |a: usize| f32::from_le_bytes(data[a..a + 4].try_into().unwrap());
        let u = |a: usize| u32::from_le_bytes(data[a..a + 4].try_into().unwrap());
        assert_eq!(u(0x90), STRIDE);
        let vb = 0xcc;
        let ib = vb + u(0x80) as usize;
        let mut parts = vec![];
        for s in risen_formats::xmsh::parse_submeshes(d) {
            let (fv, vc, fi, ic) = (s.first_vertex.unwrap() as usize, s.vertex_count.unwrap() as usize, s.first_index.unwrap() as usize, s.index_count.unwrap() as usize);
            let vertices = (fv..fv + vc).map(|i| {
                let a = vb + i * 52;
                Vertex { pos: [f(a), f(a + 4), f(a + 8)], normal: [f(a + 12), f(a + 16), f(a + 20)], tangent: [f(a + 24), f(a + 28), f(a + 32)], right_handed: data[a + 38] == 0xff, uv: [f(a + 44), f(a + 48)] }
            }).collect();
            let triangles = (0..ic / 3).map(|t| [0, 1, 2].map(|k| u(ib + (fi + t * 3 + k) * 4) - fv as u32)).collect();
            parts.push(Part { material: s.material, vertices, triangles });
        }
        parts
    }

    /// Every item-style mesh in the game decodes the same after going through this writer.
    #[test]
    fn rewrites_game_meshes_without_changing_geometry() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        let mut checked = 0;
        for e in g.entries_with_suffix("._xmsh").into_iter().step_by(7) {
            let d = g.read(&e).unwrap().0;
            let Ok(res) = Resource::parse(&d) else { continue };
            if res.data.get(..72) != Some(&DECLARATION[..72]) { continue; }
            let name = e.rsplit('/').next().unwrap().split('.').next().unwrap();
            let out = write(name, &read_parts(&d), 0);
            let (a, b) = (crate::xmsh_geom::decode(&d).unwrap(), crate::xmsh_geom::decode(&out).unwrap());
            assert_eq!((a.positions, a.normals, a.uvs, a.indices), (b.positions, b.normals, b.uvs, b.indices), "{e}");
            assert_eq!(a.submeshes, b.submeshes, "{e}");
            checked += 1;
        }
        eprintln!("{checked} meshes rewritten");
        assert!(checked > 50);
    }
}
