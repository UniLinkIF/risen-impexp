//! Textures (._ximg): the Genome header and the DDS payload inside it.

use std::io;

const MAGIC: &[u8; 8] = b"GR01IM04";

#[derive(Debug)]
#[allow(dead_code)]
pub struct XimgInfo {
    pub header_const: i32,
    pub property_block_size: i32,
    pub dds_offset: usize,
    pub width: i32,
    pub height: i32,
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

struct PropertySlot {
    datalen_offset: usize,
    value_offset: usize,
    data_len: usize,
}

fn find_property_slot(data: &[u8], name: &str) -> Option<PropertySlot> {
    let name_idx = find_subslice(data, name.as_bytes())?;
    let after_name = name_idx + name.len();
    let type_len = u16::from_le_bytes(data.get(after_name..after_name + 2)?.try_into().ok()?) as usize;
    let after_type = after_name + 2 + type_len;
    let datalen_offset = after_type + 2;
    let data_len = u32::from_le_bytes(data.get(datalen_offset..datalen_offset + 4)?.try_into().ok()?) as usize;
    Some(PropertySlot {
        datalen_offset,
        value_offset: datalen_offset + 4,
        data_len,
    })
}

fn find_property_value_offset(data: &[u8], name: &str) -> Option<usize> {
    find_property_slot(data, name).map(|s| s.value_offset)
}

pub fn read_pixel_format(data: &[u8]) -> io::Result<String> {
    let slot = find_property_slot(data, "PixelFormat")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "PixelFormat property not found"))?;
    let value = &data[slot.value_offset..slot.value_offset + slot.data_len];
    if value.len() < 2 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "PixelFormat value too short"));
    }
    Ok(String::from_utf8_lossy(&value[2..]).into_owned())
}

fn patch_pixel_format(data: &mut Vec<u8>, new_format: &str) -> io::Result<isize> {
    let slot = find_property_slot(data, "PixelFormat")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "PixelFormat property not found"))?;
    if slot.data_len < 2 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "PixelFormat value too short"));
    }
    let marker = [data[slot.value_offset], data[slot.value_offset + 1]];
    let mut new_value = marker.to_vec();
    new_value.extend_from_slice(new_format.as_bytes());
    let delta = new_value.len() as isize - slot.data_len as isize;

    data.splice(slot.value_offset..slot.value_offset + slot.data_len, new_value);
    let new_data_len = (slot.data_len as isize + delta) as u32;
    data[slot.datalen_offset..slot.datalen_offset + 4].copy_from_slice(&new_data_len.to_le_bytes());
    Ok(delta)
}

pub fn parse(data: &[u8]) -> io::Result<XimgInfo> {
    if data.len() < 20 || &data[0..8] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a Risen 1/2 GR01IM04 ._ximg file",
        ));
    }
    let header_const = i32::from_le_bytes(data[8..12].try_into().unwrap());
    let property_block_size = i32::from_le_bytes(data[12..16].try_into().unwrap());
    let dds_offset = i32::from_le_bytes(data[16..20].try_into().unwrap()) as usize;

    if data.len() < dds_offset + 4 || &data[dds_offset..dds_offset + 4] != b"DDS " {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "dds_offset field does not point at a DDS signature",
        ));
    }

    let width_off = find_property_value_offset(data, "Width")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Width property not found"))?;
    let height_off = find_property_value_offset(data, "Height")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Height property not found"))?;
    let width = i32::from_le_bytes(
        data.get(width_off..width_off + 4)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Width value truncated"))?
            .try_into()
            .unwrap(),
    );
    let height = i32::from_le_bytes(
        data.get(height_off..height_off + 4)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Height value truncated"))?
            .try_into()
            .unwrap(),
    );

    Ok(XimgInfo {
        header_const,
        property_block_size,
        dds_offset,
        width,
        height,
    })
}

pub fn extract_dds(data: &[u8]) -> io::Result<&[u8]> {
    let info = parse(data)?;
    Ok(&data[info.dds_offset..])
}

#[derive(Debug, Default)]
pub struct ReplaceOptions<'a> {
    pub width: i32,
    pub height: i32,
    pub skip_mips: Option<i32>,
    pub pixel_format: Option<&'a str>,
}

pub fn replace_dds(original: &[u8], opts: ReplaceOptions, new_dds: &[u8]) -> io::Result<Vec<u8>> {
    let info = parse(original)?;
    let mut out = original[..info.dds_offset].to_vec();

    let width_off = find_property_value_offset(&out, "Width").unwrap();
    out[width_off..width_off + 4].copy_from_slice(&opts.width.to_le_bytes());
    let height_off = find_property_value_offset(&out, "Height").unwrap();
    out[height_off..height_off + 4].copy_from_slice(&opts.height.to_le_bytes());

    if let Some(mips) = opts.skip_mips {
        let off = find_property_value_offset(&out, "SkipMips").ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "SkipMips property not found")
        })?;
        out[off..off + 4].copy_from_slice(&mips.to_le_bytes());
    }

    let mut shift: isize = 0;
    if let Some(fmt) = opts.pixel_format {
        shift = patch_pixel_format(&mut out, fmt)?;
    }

    if shift != 0 {
        let new_prop_block_size = (info.property_block_size as isize + shift) as i32;
        out[12..16].copy_from_slice(&new_prop_block_size.to_le_bytes());
        let new_dds_offset = (info.dds_offset as isize + shift) as i32;
        out[16..20].copy_from_slice(&new_dds_offset.to_le_bytes());
    }

    out.extend_from_slice(new_dds);
    Ok(out)
}

