//! Materials (._xmat): the texture file names a material samples.

use crate::tple;

const MAGIC: &[u8; 8] = b"GR01SM01";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MaterialTextures {
    pub diffuse: Option<String>,
    pub normal: Option<String>,
    pub specular: Option<String>,
    pub all: Vec<String>,
}

pub fn is_xmat(data: &[u8]) -> bool {
    data.len() >= MAGIC.len() && &data[..MAGIC.len()] == MAGIC
}

pub fn parse(data: &[u8]) -> Option<MaterialTextures> {
    if !is_xmat(data) {
        return None;
    }
    let tokens = tple::read_tokens(data);
    let mut out = MaterialTextures::default();
    for (i, (_, name)) in tokens.iter().enumerate() {
        if name != "ImageFilePath" {
            continue;
        }
        let Some((_, type_name)) = tokens.get(i + 1) else { continue };
        if !type_name.starts_with("bC") || !type_name.ends_with("String") {
            continue;
        }
        let Some((_, value)) = tokens.get(i + 2) else { continue };
        let file_name = value.rsplit(['/', '\\']).next().unwrap_or(value).to_string();
        if file_name.is_empty() {
            continue;
        }
        let lower = file_name.to_ascii_lowercase();
        if !out.all.iter().any(|e| e.eq_ignore_ascii_case(&file_name)) {
            out.all.push(file_name.clone());
        }
        if lower.contains("_normal") {
            out.normal.get_or_insert(file_name);
        } else if lower.contains("_comp") || lower.contains("_specular") {
            out.specular.get_or_insert(file_name);
        } else if lower.contains("_diffuse") {
            out.diffuse.get_or_insert(file_name);
        }
    }
    if out.diffuse.is_none() {
        out.diffuse = out.all.first().cloned();
    }
    Some(out)
}

