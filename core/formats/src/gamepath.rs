//! Finding the game: its root folder above binRisen.exe (shortcuts resolved) and its .pak archives.

use std::io;
use std::path::{Path, PathBuf};

#[cfg(windows)]
fn decode_ansi_path(bytes: &[u8]) -> String {
    const CP_ACP: u32 = 0;
    #[link(name = "kernel32")]
    extern "system" {
        fn MultiByteToWideChar(
            codepage: u32,
            flags: u32,
            bytes: *const u8,
            byte_len: i32,
            wide: *mut u16,
            wide_len: i32,
        ) -> i32;
    }
    unsafe {
        let wide_len =
            MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), bytes.len() as i32, std::ptr::null_mut(), 0);
        if wide_len <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut wide = vec![0u16; wide_len as usize];
        MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), bytes.len() as i32, wide.as_mut_ptr(), wide_len);
        String::from_utf16_lossy(&wide)
    }
}

#[cfg(not(windows))]
fn decode_ansi_path(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

pub fn resolve_shortcut(path: &Path) -> io::Result<PathBuf> {
    let is_lnk = path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("lnk"))
        .unwrap_or(false);
    if !is_lnk {
        return Ok(path.to_path_buf());
    }
    let data = std::fs::read(path)?;
    parse_lnk_target(&data)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "could not read .lnk target path"))
}

fn parse_lnk_target(data: &[u8]) -> Option<String> {
    const HEADER_SIZE: usize = 76;
    const SHELL_LINK_CLSID: [u8; 16] = [
        0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    if data.len() < HEADER_SIZE {
        return None;
    }
    if data[0..4] != [0x4C, 0x00, 0x00, 0x00] || data[4..20] != SHELL_LINK_CLSID {
        return None;
    }
    let link_flags = u32::from_le_bytes(data[20..24].try_into().ok()?);
    const HAS_LINK_INFO: u32 = 0x0000_0002;
    const HAS_LINK_TARGET_ID_LIST: u32 = 0x0000_0001;

    let mut offset = HEADER_SIZE;
    if link_flags & HAS_LINK_TARGET_ID_LIST != 0 {
        let size = u16::from_le_bytes(data.get(offset..offset + 2)?.try_into().ok()?) as usize;
        offset += 2 + size;
    }
    if link_flags & HAS_LINK_INFO == 0 {
        return None;
    }

    let link_info_start = offset;
    let link_info_size = u32::from_le_bytes(data.get(offset..offset + 4)?.try_into().ok()?) as usize;
    let link_info = data.get(link_info_start..link_info_start + link_info_size)?;

    let link_info_flags = u32::from_le_bytes(link_info.get(8..12)?.try_into().ok()?);
    const VOLUME_ID_AND_LOCAL_BASE_PATH: u32 = 0x1;
    if link_info_flags & VOLUME_ID_AND_LOCAL_BASE_PATH == 0 {
        return None;
    }
    let local_base_path_offset =
        u32::from_le_bytes(link_info.get(16..20)?.try_into().ok()?) as usize;
    let path_bytes = &link_info[local_base_path_offset..];
    let end = path_bytes.iter().position(|&b| b == 0)?;
    Some(decode_ansi_path(&path_bytes[..end]))
}

const ARCHIVE_SUBDIRS: &[&str] = &["compiled", "common"];

pub fn discover_game_root(exe_path: &Path) -> Option<PathBuf> {
    let mut dir = exe_path.parent()?;
    loop {
        let data_dir = dir.join("data");
        if data_dir.is_dir() && ARCHIVE_SUBDIRS.iter().any(|s| data_dir.join(s).is_dir()) {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

fn looks_like_archive(path: &Path) -> bool {
    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(e) => e.to_ascii_lowercase(),
        None => return false,
    };
    if ext == "pak" {
        return true;
    }
    let rest = ext.strip_prefix('p').unwrap_or(&ext);
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

#[derive(Debug, Clone)]
pub struct DiscoveredArchive {
    pub path: PathBuf,
    pub group: String,
}

pub fn discover_archives(game_root: &Path) -> io::Result<Vec<DiscoveredArchive>> {
    let data_dir = game_root.join("data");
    let mut out = Vec::new();
    for group in ARCHIVE_SUBDIRS {
        let dir = data_dir.join(group);
        if dir.is_dir() {
            walk_for_archives(&dir, group, &mut out)?;
        }
    }
    Ok(out)
}

fn walk_for_archives(dir: &Path, group: &str, out: &mut Vec<DiscoveredArchive>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_for_archives(&path, group, out)?;
        } else if looks_like_archive(&path) {
            out.push(DiscoveredArchive {
                path,
                group: group.to_string(),
            });
        }
    }
    Ok(())
}

