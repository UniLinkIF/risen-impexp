//! Actors (._xmac, EMotionFX XAC): the node hierarchy — the skeleton with bind-pose transforms.

use anyhow::{bail, Result};
use serde::Serialize;

const SECTION_NODES: u32 = 11;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkeletonNode {
    pub name: String,
    pub parent_index: Option<usize>,
    pub rotation: [f32; 4],
    pub position: [f32; 3],
}

fn read_u32(data: &[u8], off: usize, big_endian: bool) -> Result<u32> {
    let bytes: [u8; 4] = data
        .get(off..off + 4)
        .ok_or_else(|| anyhow::anyhow!("xmac: unexpected end of file at offset {off}"))?
        .try_into()
        .unwrap();
    Ok(if big_endian { u32::from_be_bytes(bytes) } else { u32::from_le_bytes(bytes) })
}

fn read_f32(data: &[u8], off: usize, big_endian: bool) -> Result<f32> {
    Ok(f32::from_bits(read_u32(data, off, big_endian)?))
}

fn read_string(data: &[u8], off: usize, len: usize) -> Result<String> {
    let bytes = data
        .get(off..off + len)
        .ok_or_else(|| anyhow::anyhow!("xmac: unexpected end of file reading a {len}-byte string at {off}"))?;
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

pub fn parse_skeleton(data: &[u8]) -> Result<Vec<SkeletonNode>> {
    if data.len() < 150 {
        bail!("xmac: file too short to be a valid ._xmac actor ({} bytes)", data.len());
    }
    let end_section_offset = read_u32(data, 136, false)? as usize + 140;
    if read_string(data, 140, 3)? != "XAC" {
        bail!("xmac: missing 'XAC' magic at offset 140");
    }
    let big_endian = data[146] != 0;

    let mut nodes = Vec::new();
    let mut next_section = 148usize;
    while next_section < end_section_offset {
        let section_id = read_u32(data, next_section, big_endian)?;
        let section_size = read_u32(data, next_section + 4, big_endian)? as usize + 12;
        let section_start = next_section;
        next_section += section_size;

        if section_id != SECTION_NODES {
            continue;
        }

        let node_count = read_u32(data, section_start + 12, big_endian)? as usize;
        let mut off = section_start + 12 + 4 + 4;
        for _ in 0..node_count {
            let rotation = [
                read_f32(data, off, big_endian)?,
                read_f32(data, off + 4, big_endian)?,
                read_f32(data, off + 8, big_endian)?,
                read_f32(data, off + 12, big_endian)?,
            ];
            off += 16 + 16;
            let position = [
                read_f32(data, off, big_endian)?,
                read_f32(data, off + 4, big_endian)?,
                read_f32(data, off + 8, big_endian)?,
            ];
            off += 12 + 32;
            let parent_raw = read_u32(data, off, big_endian)?;
            off += 4 + 76;
            let name_len = read_u32(data, off, big_endian)? as usize;
            off += 4;
            let name = read_string(data, off, name_len)?;
            off += name_len;

            nodes.push(SkeletonNode {
                name,
                parent_index: if (parent_raw as usize) < node_count { Some(parent_raw as usize) } else { None },
                rotation,
                position,
            });
        }
    }
    Ok(nodes)
}

