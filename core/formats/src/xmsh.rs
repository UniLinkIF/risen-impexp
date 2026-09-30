//! Static meshes (._xmsh): the submesh table (materials and index/vertex ranges).

use crate::tple;

const MAGIC: &[u8; 8] = b"GR01MS02";

#[derive(Debug, Clone, PartialEq)]
pub struct SubMesh {
    pub material: String,
    pub first_index: Option<i32>,
    pub index_count: Option<i32>,
    pub first_vertex: Option<i32>,
    pub vertex_count: Option<i32>,
    pub index_count_at: Option<usize>,
}

pub fn is_xmsh(data: &[u8]) -> bool {
    data.len() >= MAGIC.len() && &data[..MAGIC.len()] == MAGIC
}

pub fn parse_submeshes(data: &[u8]) -> Vec<SubMesh> {
    if !is_xmsh(data) {
        return Vec::new();
    }
    let tokens = tple::read_tokens(data);
    let read_i32 = |at: usize| -> Option<i32> {
        Some(i32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
    };
    let long_after = |from: usize, property: &str| -> Option<(usize, i32)> {
        for (i, (_, name)) in tokens.iter().enumerate().skip(from + 1) {
            if name == "MaterialName" {
                return None;
            }
            if name != property {
                continue;
            }
            let (type_at, type_name) = tokens.get(i + 1)?;
            if type_name != "long" {
                return None;
            }
            let after_type = type_at + 2 + type_name.len();
            let at = after_type + 2 + 4;
            return Some((at, read_i32(at)?));
        }
        None
    };

    let mut out = Vec::new();
    for (i, (_, name)) in tokens.iter().enumerate() {
        if name != "MaterialName" {
            continue;
        }
        let Some((_, type_name)) = tokens.get(i + 1) else { continue };
        if type_name != "bCString" {
            continue;
        }
        let Some((_, material)) = tokens.get(i + 2) else { continue };
        let index_count = long_after(i, "IndexCount");
        out.push(SubMesh {
            material: material.clone(),
            first_index: long_after(i, "FirstIndex").map(|(_, v)| v),
            index_count: index_count.map(|(_, v)| v),
            first_vertex: long_after(i, "FirstVertex").map(|(_, v)| v),
            vertex_count: long_after(i, "VertexCount").map(|(_, v)| v),
            index_count_at: index_count.map(|(at, _)| at),
        });
    }
    out
}

pub fn parse_material_names(data: &[u8]) -> Vec<String> {
    parse_submeshes(data).into_iter().map(|s| s.material).collect()
}

pub fn hide_submeshes(data: &[u8], hidden_materials: &[String]) -> Result<Vec<u8>, String> {
    let submeshes = parse_submeshes(data);
    if submeshes.is_empty() {
        return Err("this file declares no submeshes — it is not a readable ._xmsh mesh".into());
    }
    let wanted: Vec<String> = hidden_materials.iter().map(|m| m.trim().to_ascii_lowercase()).collect();
    let mut out = data.to_vec();
    let mut hidden = 0usize;
    for sub in &submeshes {
        if !wanted.iter().any(|w| w == &sub.material.trim().to_ascii_lowercase()) {
            continue;
        }
        let Some(at) = sub.index_count_at else {
            return Err(format!("submesh '{}' has no readable IndexCount to clear", sub.material));
        };
        if out.len() < at + 4 {
            return Err(format!("submesh '{}' points past the end of the file", sub.material));
        }
        out[at..at + 4].copy_from_slice(&0i32.to_le_bytes());
        hidden += 1;
    }
    if hidden == 0 {
        return Err("none of the named materials belong to this mesh — nothing was changed".into());
    }
    if hidden == submeshes.len() {
        return Err("that would hide every layer of the model, leaving it invisible in the game".into());
    }
    Ok(out)
}

pub fn parse_lightmap_name(data: &[u8]) -> Option<String> {
    if !is_xmsh(data) {
        return None;
    }
    let tokens = tple::read_tokens(data);
    for (i, (_, name)) in tokens.iter().enumerate() {
        if name != "LightmapName" {
            continue;
        }
        let (_, type_name) = tokens.get(i + 1)?;
        if type_name != "bCString" {
            continue;
        }
        let value = tokens.get(i + 2)?.1.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

