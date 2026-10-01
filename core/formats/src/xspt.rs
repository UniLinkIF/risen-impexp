//! SpeedTree recipes (._xspt): the plant's bounding box and the textures it names.

const MAGIC: &[u8; 8] = b"GR01ST01";

#[allow(dead_code)]
const PAYLOAD_MARKER: &[u8] = b"__IdvSpt_02_";

const VALUE_MARKER: u16 = 0x1E;

#[derive(Debug, Clone, PartialEq)]
pub struct SpeedTreeInfo {
    pub bounds: Option<([f32; 3], [f32; 3])>,
    pub textures: Vec<String>,
}

pub fn is_xspt(data: &[u8]) -> bool {
    data.len() >= MAGIC.len() && &data[..MAGIC.len()] == MAGIC
}

pub fn parse(data: &[u8]) -> Option<SpeedTreeInfo> {
    if !is_xspt(data) {
        return None;
    }
    Some(SpeedTreeInfo { bounds: find_bounds(data), textures: find_textures(data) })
}

fn find_bounds(data: &[u8]) -> Option<([f32; 3], [f32; 3])> {
    let at = find(data, b"Boundary")?;
    let after_name = at + b"Boundary".len();
    let type_len = read_u16(data, after_name)? as usize;
    let type_at = after_name + 2;
    if data.get(type_at..type_at + type_len)? != b"bCBox" {
        return None;
    }
    let marker_at = type_at + type_len;
    if read_u16(data, marker_at)? != VALUE_MARKER {
        return None;
    }
    let len_at = marker_at + 2;
    let value_len = read_u32(data, len_at)? as usize;
    if value_len < 24 {
        return None;
    }
    let v = len_at + 4;
    let f = |i: usize| read_f32(data, v + i * 4);
    Some((
        [f(0)?, f(1)?, f(2)?],
        [f(3)?, f(4)?, f(5)?],
    ))
}

fn find_textures(data: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i + 4 <= data.len() {
        let Some(len) = read_u32(data, i).map(|l| l as usize) else { break };
        if (5..=260).contains(&len) && i + 4 + len <= data.len() {
            let chunk = &data[i + 4..i + 4 + len];
            if chunk.iter().all(|&b| (0x20..0x7f).contains(&b)) {
                let s = String::from_utf8_lossy(chunk);
                let lower = s.to_ascii_lowercase();
                if lower.ends_with(".tga") || lower.ends_with(".dds") || lower.ends_with(".png") {
                    let file_name = s.rsplit(['/', '\\']).next().unwrap_or(&s).to_string();
                    if !out.iter().any(|existing| existing.eq_ignore_ascii_case(&file_name)) {
                        out.push(file_name);
                    }
                }
                i += 4 + len;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn read_f32(data: &[u8], at: usize) -> Option<f32> {
    Some(f32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

