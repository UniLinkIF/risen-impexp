//! Actors (._xmac): the skinned meshes — raw vertices, uv seams, faces, materials, bone weights.

use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::HashMap;

const SECTION_MESH: u32 = 1;
const SECTION_SKIN: u32 = 2;
const SECTION_MATERIALS: u32 = 13;

const MESH_SECTION_VERTICES: u32 = 0;
const MESH_SECTION_NORMALS: u32 = 1;
const MESH_SECTION_TEXCOORDS: u32 = 3;
const MESH_SECTION_BASEVERTS: u32 = 5;

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkinnedMeshMaterial {
    pub name: String,
    pub diffuse: Option<String>,
    pub normal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkinnedMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub faces: Vec<[u32; 3]>,
    pub skin_weights: Vec<Vec<(u32, f32)>>,
    pub materials: Vec<SkinnedMeshMaterial>,
    pub face_material_ids: Vec<u32>,
}

fn read_u32(data: &[u8], off: usize) -> Result<u32> {
    let bytes: [u8; 4] = data.get(off..off + 4).ok_or_else(|| anyhow::anyhow!("xmesh: unexpected end of file at offset {off}"))?.try_into().unwrap();
    Ok(u32::from_le_bytes(bytes))
}
fn read_u16(data: &[u8], off: usize) -> Result<u16> {
    let bytes: [u8; 2] = data.get(off..off + 2).ok_or_else(|| anyhow::anyhow!("xmesh: unexpected end of file at offset {off}"))?.try_into().unwrap();
    Ok(u16::from_le_bytes(bytes))
}
fn read_f32(data: &[u8], off: usize) -> Result<f32> {
    Ok(f32::from_bits(read_u32(data, off)?))
}

struct RawMesh {
    node_index: usize,
    raw_positions: Vec<[f32; 3]>,
    raw_normals: Vec<[f32; 3]>,
    raw_uvs: Vec<[f32; 2]>,
    base_verts: Vec<u32>,
    final_vert_count: usize,
    faces_raw: Vec<[u32; 3]>,
    face_material_ids: Vec<u32>,
}

pub fn parse_skinned_mesh(data: &[u8]) -> Result<SkinnedMesh> {
    if data.len() < 150 {
        bail!("xmesh: file too short to be a valid ._xmac actor ({} bytes)", data.len());
    }
    let end_section_offset = read_u32(data, 136)? as usize + 140;
    if data.get(140..143) != Some(b"XAC") {
        bail!("xmesh: missing 'XAC' magic at offset 140");
    }
    if data[146] != 0 {
        bail!("xmesh: big-endian actor files aren't supported for mesh/skin parsing yet");
    }

    let mut raw_meshes: Vec<RawMesh> = Vec::new();
    let mut skin_entries: Vec<(usize, Vec<f32>, Vec<u16>, Vec<u32>)> = Vec::new();
    let mut materials: Vec<SkinnedMeshMaterial> = Vec::new();

    let mut next_section = 148usize;
    while next_section < end_section_offset {
        let section_id = read_u32(data, next_section)?;
        let declared_size = read_u32(data, next_section + 4)? as usize + 12;
        let section_start = next_section;

        if section_id == SECTION_MATERIALS {
            let (end, mats) = parse_materials_section(data, section_start)?;
            next_section = end;
            if materials.is_empty() {
                materials = mats;
            }
        } else {
            next_section += declared_size;
        }

        if section_id == SECTION_MESH {
            raw_meshes.push(parse_mesh_section(data, section_start)?);
        } else if section_id == SECTION_SKIN {
            skin_entries.push(parse_skin_section(data, section_start)?);
        }
    }

    if raw_meshes.iter().any(|m| !m.raw_uvs.is_empty()) {
        raw_meshes.retain(|m| !m.raw_uvs.is_empty());
    }

    let mut out = SkinnedMesh::default();
    out.materials = materials;
    let mut vert_offset = 0u32;
    let mut node_offsets: HashMap<usize, (u32, usize)> = HashMap::new();

    for (mesh_i, mesh) in raw_meshes.iter().enumerate() {
        let raw_count = mesh.base_verts.len();
        let pad3 = |src: &Vec<[f32; 3]>| -> Vec<[f32; 3]> {
            let mut v = src.clone();
            v.resize(raw_count, [0.0; 3]);
            v
        };
        out.positions.extend(pad3(&mesh.raw_positions));
        out.normals.extend(pad3(&mesh.raw_normals));
        let mut uvs = mesh.raw_uvs.clone();
        uvs.resize(raw_count, [0.0; 2]);
        out.uvs.extend(uvs);
        for face in &mesh.faces_raw {
            out.faces.push([face[0] + vert_offset, face[1] + vert_offset, face[2] + vert_offset]);
        }
        out.face_material_ids.extend_from_slice(&mesh.face_material_ids);
        node_offsets.insert(mesh.node_index, (vert_offset, mesh_i));
        vert_offset += raw_count as u32;
    }
    out.skin_weights = vec![Vec::new(); vert_offset as usize];

    for (node_index, weights, bone_indices, per_vert_count) in &skin_entries {
        let Some(&(offset, mesh_i)) = node_offsets.get(node_index) else { continue };
        let mesh = &raw_meshes[mesh_i];
        let mut final_weights: Vec<Vec<(u32, f32)>> = vec![Vec::new(); mesh.final_vert_count];
        let mut w_i = 0usize;
        for (final_vert, &count) in per_vert_count.iter().enumerate() {
            for _ in 0..count {
                if w_i < weights.len() {
                    if let Some(slot) = final_weights.get_mut(final_vert) {
                        slot.push((bone_indices[w_i] as u32, weights[w_i]));
                    }
                }
                w_i += 1;
            }
        }
        for (raw_i, &final_i) in mesh.base_verts.iter().enumerate() {
            if let Some(w) = final_weights.get(final_i as usize) {
                out.skin_weights[offset as usize + raw_i] = w.clone();
            }
        }
    }

    Ok(out)
}

pub fn parse_materials(data: &[u8]) -> Result<Vec<SkinnedMeshMaterial>> {
    if data.len() < 150 {
        bail!("xmesh: file too short to be a valid ._xmac actor ({} bytes)", data.len());
    }
    let end_section_offset = read_u32(data, 136)? as usize + 140;
    if data.get(140..143) != Some(b"XAC") {
        bail!("xmesh: missing 'XAC' magic at offset 140");
    }
    if data[146] != 0 {
        bail!("xmesh: big-endian actor files aren't supported for mesh/skin parsing yet");
    }

    let mut next_section = 148usize;
    while next_section < end_section_offset {
        let section_id = read_u32(data, next_section)?;
        let declared_size = read_u32(data, next_section + 4)? as usize + 12;
        if section_id == SECTION_MATERIALS {
            let (_, mats) = parse_materials_section(data, next_section)?;
            return Ok(mats);
        }
        if declared_size == 0 {
            break;
        }
        next_section += declared_size;
    }
    Ok(Vec::new())
}

pub fn is_degenerate(face: [u32; 3]) -> bool {
    face[0] == face[1] && face[1] == face[2]
}

pub fn hide_materials(data: &[u8], hidden_materials: &[String]) -> Result<Vec<u8>> {
    let materials = parse_materials(data)?;
    if materials.is_empty() {
        bail!("this actor declares no materials — nothing to hide");
    }
    let wanted: Vec<String> = hidden_materials.iter().map(|m| m.trim().to_ascii_lowercase()).collect();
    let hidden_ids: Vec<u32> = materials
        .iter()
        .enumerate()
        .filter(|(_, m)| wanted.iter().any(|w| *w == m.name.trim().to_ascii_lowercase()))
        .map(|(i, _)| i as u32)
        .collect();
    if hidden_ids.is_empty() {
        bail!("none of the named materials belong to this actor — nothing was changed");
    }
    if hidden_ids.len() >= materials.len() {
        bail!("that would hide every layer of the character, leaving it invisible in the game");
    }

    let end_section_offset = read_u32(data, 136)? as usize + 140;
    let mut out = data.to_vec();
    let mut collapsed = 0usize;
    let mut next_section = 148usize;
    while next_section < end_section_offset {
        let section_id = read_u32(data, next_section)?;
        let declared_size = read_u32(data, next_section + 4)? as usize + 12;
        if section_id == SECTION_MATERIALS {
            let (end, _) = parse_materials_section(data, next_section)?;
            next_section = end;
            continue;
        }
        if section_id == SECTION_MESH {
            collapsed += collapse_hidden_faces(data, &mut out, next_section, &hidden_ids)?;
        }
        if declared_size == 0 {
            break;
        }
        next_section += declared_size;
    }

    if collapsed == 0 {
        bail!("those materials have no faces in this actor — nothing was changed");
    }
    Ok(out)
}

fn collapse_hidden_faces(data: &[u8], out: &mut [u8], section_start: usize, hidden_ids: &[u32]) -> Result<usize> {
    let mut off = section_start + 12;
    off += 4;
    off += 4;
    let uvert_count = read_u32(data, off)? as usize;
    off += 4;
    let face_count = read_u32(data, off)? as usize / 3;
    off += 4;
    off += 4;
    let mesh_section_count = read_u32(data, off)?;
    off += 4;
    off += 4;

    for _ in 0..mesh_section_count {
        let sub_id = read_u32(data, off)?;
        let block_size = read_u32(data, off + 4)? as usize;
        off += 12;
        off += match sub_id {
            MESH_SECTION_VERTICES | MESH_SECTION_NORMALS if block_size == 12 => uvert_count * 12,
            MESH_SECTION_TEXCOORDS if block_size == 8 => uvert_count * 8,
            MESH_SECTION_BASEVERTS if block_size == 4 => uvert_count * 4,
            _ => block_size * uvert_count,
        };
    }

    let mut collapsed = 0usize;
    let mut passed_faces = 0usize;
    while passed_faces != face_count {
        let part_face_count = read_u32(data, off)? as usize / 3;
        let part_material_id = read_u32(data, off + 8)?;
        let skip_words = read_u32(data, off + 12)? as usize;
        off += 16;
        if hidden_ids.contains(&part_material_id) {
            for _ in 0..part_face_count {
                let a = data.get(off..off + 4).ok_or_else(|| anyhow::anyhow!("xmesh: face runs past end of file"))?;
                out[off + 4..off + 8].copy_from_slice(a);
                out[off + 8..off + 12].copy_from_slice(a);
                collapsed += 1;
                off += 12;
            }
        } else {
            off += part_face_count * 12;
        }
        off += skip_words * 4;
        passed_faces += part_face_count;
    }
    Ok(collapsed)
}

fn parse_materials_section(data: &[u8], section_start: usize) -> Result<(usize, Vec<SkinnedMeshMaterial>)> {
    let read_str = |off: usize, len: usize| -> Result<String> {
        let bytes = data
            .get(off..off + len)
            .ok_or_else(|| anyhow::anyhow!("xmesh: unexpected end of file at offset {off}"))?;
        Ok(String::from_utf8_lossy(bytes).trim_end_matches('\0').to_string())
    };

    let mut off = section_start + 12;
    let material_count = read_u32(data, off)?;
    off += 4;
    off += 8;
    let mut materials = Vec::with_capacity(material_count as usize);
    for _ in 0..material_count {
        off += 95;
        let map_count = *data.get(off).ok_or_else(|| anyhow::anyhow!("xmesh: unexpected end of file at offset {off}"))?;
        off += 1;
        let name_len = read_u32(data, off)? as usize;
        off += 4;
        let name = read_str(off, name_len)?;
        off += name_len;
        let mut material = SkinnedMeshMaterial { name, diffuse: None, normal: None };
        for _ in 0..map_count {
            off += 26;
            let map_type = *data.get(off).ok_or_else(|| anyhow::anyhow!("xmesh: unexpected end of file at offset {off}"))? % 8;
            off += 1;
            off += 1;
            let path_len = read_u32(data, off)? as usize;
            off += 4;
            let path = read_str(off, path_len)?;
            off += path_len;
            match map_type {
                2 if material.diffuse.is_none() => material.diffuse = Some(path),
                5 if material.normal.is_none() => material.normal = Some(path),
                _ => {}
            }
        }
        materials.push(material);
    }
    Ok((off, materials))
}

fn parse_mesh_section(data: &[u8], section_start: usize) -> Result<RawMesh> {
    let mut off = section_start + 12;
    let node_index = read_u32(data, off)? as usize;
    off += 4;
    let vert_count = read_u32(data, off)? as usize;
    off += 4;
    let uvert_count = read_u32(data, off)? as usize;
    off += 4;
    let face_count = read_u32(data, off)? as usize / 3;
    off += 4;
    off += 4;
    let mesh_section_count = read_u32(data, off)?;
    off += 4;
    off += 4;

    let mut base_verts: Vec<u32> = (0..uvert_count as u32).collect();
    let mut raw_positions = Vec::new();
    let mut raw_normals = Vec::new();
    let mut raw_uvs = Vec::new();

    for _ in 0..mesh_section_count {
        let sub_id = read_u32(data, off)?;
        let block_size = read_u32(data, off + 4)?;
        off += 12;
        if sub_id == MESH_SECTION_VERTICES && block_size == 12 {
            raw_positions = (0..uvert_count)
                .map(|v| {
                    let o = off + v * 12;
                    Ok([read_f32(data, o)?, read_f32(data, o + 4)?, read_f32(data, o + 8)?])
                })
                .collect::<Result<Vec<_>>>()?;
            off += uvert_count * 12;
        } else if sub_id == MESH_SECTION_NORMALS && block_size == 12 {
            raw_normals = (0..uvert_count)
                .map(|v| {
                    let o = off + v * 12;
                    Ok([read_f32(data, o)?, read_f32(data, o + 4)?, read_f32(data, o + 8)?])
                })
                .collect::<Result<Vec<_>>>()?;
            off += uvert_count * 12;
        } else if sub_id == MESH_SECTION_TEXCOORDS && block_size == 8 {
            raw_uvs = (0..uvert_count)
                .map(|v| {
                    let o = off + v * 8;
                    Ok([read_f32(data, o)?, read_f32(data, o + 4)?])
                })
                .collect::<Result<Vec<_>>>()?;
            off += uvert_count * 8;
        } else if sub_id == MESH_SECTION_BASEVERTS && block_size == 4 {
            base_verts = (0..uvert_count).map(|v| read_u32(data, off + v * 4)).collect::<Result<Vec<_>>>()?;
            off += uvert_count * 4;
        } else {
            off += (block_size as usize) * uvert_count;
        }
    }

    let mut faces_raw = Vec::with_capacity(face_count);
    let mut face_material_ids = Vec::with_capacity(face_count);
    let mut passed_faces = 0usize;
    let mut passed_verts = 0u32;
    while passed_faces != face_count {
        let part_face_count = read_u32(data, off)? as usize / 3;
        off += 4;
        let part_vert_count = read_u32(data, off)?;
        off += 4;
        let part_material_id = read_u32(data, off)?;
        off += 4;
        let skip_words = read_u32(data, off)?;
        off += 4;
        for _ in 0..part_face_count {
            let a = read_u32(data, off)? + passed_verts;
            let b = read_u32(data, off + 4)? + passed_verts;
            let c = read_u32(data, off + 8)? + passed_verts;
            faces_raw.push([a, b, c]);
            face_material_ids.push(part_material_id);
            off += 12;
        }
        off += (skip_words as usize) * 4;
        passed_faces += part_face_count;
        passed_verts += part_vert_count;
    }

    Ok(RawMesh {
        node_index,
        raw_positions,
        raw_normals,
        raw_uvs,
        base_verts,
        final_vert_count: vert_count,
        faces_raw,
        face_material_ids,
    })
}

fn parse_skin_section(data: &[u8], section_start: usize) -> Result<(usize, Vec<f32>, Vec<u16>, Vec<u32>)> {
    let mut off = section_start + 12;
    let node_index = read_u32(data, off)? as usize;
    off += 4;
    off += 4;
    let weight_count = read_u32(data, off)? as usize;
    off += 4;
    off += 4;

    let mut weights = Vec::with_capacity(weight_count);
    let mut bone_indices = Vec::with_capacity(weight_count);
    for _ in 0..weight_count {
        weights.push(read_f32(data, off)?);
        off += 4;
        bone_indices.push(read_u16(data, off)?);
        off += 2;
        off += 2;
    }

    let mut per_raw_vert_count = Vec::new();
    let mut consumed = 0usize;
    while consumed < weight_count {
        off += 4;
        let count = read_u32(data, off)?;
        off += 4;
        per_raw_vert_count.push(count);
        consumed += count as usize;
    }

    Ok((node_index, weights, bone_indices, per_raw_vert_count))
}

