//! Motion clips (._xmot, EMotionFX XSM): per-bone position / rotation / scale keys.

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;

const TAIL_BLOCK_LEN: usize = 132;
const MAX_KEY_DELTA_SECONDS: f32 = 0.5;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoneMotion {
    pub bone_name: String,
    pub position_keys: Vec<[f32; 4]>,
    pub rotation_keys: Vec<[f32; 5]>,
    pub scale_keys: Vec<[f32; 4]>,
}

impl BoneMotion {
    pub fn duration(&self) -> f32 {
        let mut d: f32 = 0.0;
        if let Some(k) = self.position_keys.last() {
            d = d.max(k[3]);
        }
        if let Some(k) = self.rotation_keys.last() {
            d = d.max(k[4]);
        }
        if let Some(k) = self.scale_keys.last() {
            d = d.max(k[3]);
        }
        d
    }
}

fn read_f32(data: &[u8], off: usize) -> Option<f32> {
    data.get(off..off + 4).map(|b| f32::from_le_bytes(b.try_into().unwrap()))
}

fn accept_time_step(prev_t: Option<f32>, t: f32) -> bool {
    match prev_t {
        None => t.abs() < 1e-5,
        Some(p) => {
            let dt = t - p;
            dt > 0.0 && dt < MAX_KEY_DELTA_SECONDS
        }
    }
}

fn scan_vec3_time_track(data: &[u8], start: usize, end: usize) -> (Vec<[f32; 4]>, usize) {
    let mut keys = Vec::new();
    let mut off = start;
    let mut prev_t: Option<f32> = None;
    while off + 16 <= end {
        let (Some(x), Some(y), Some(z), Some(t)) =
            (read_f32(data, off), read_f32(data, off + 4), read_f32(data, off + 8), read_f32(data, off + 12))
        else {
            break;
        };
        if !accept_time_step(prev_t, t) || x.abs() > 1.0e6 || y.abs() > 1.0e6 || z.abs() > 1.0e6 {
            break;
        }
        keys.push([x, y, z, t]);
        prev_t = Some(t);
        off += 16;
    }
    (keys, off - start)
}

fn scan_quat_time_track(data: &[u8], start: usize, end: usize) -> (Vec<[f32; 5]>, usize) {
    let mut keys = Vec::new();
    let mut off = start;
    let mut prev_t: Option<f32> = None;
    while off + 20 <= end {
        let (Some(x), Some(y), Some(z), Some(w), Some(t)) = (
            read_f32(data, off),
            read_f32(data, off + 4),
            read_f32(data, off + 8),
            read_f32(data, off + 12),
            read_f32(data, off + 16),
        ) else {
            break;
        };
        let magnitude = (x * x + y * y + z * z + w * w).sqrt();
        if !(0.9..1.1).contains(&magnitude) || !accept_time_step(prev_t, t) {
            break;
        }
        keys.push([x, y, z, w, t]);
        prev_t = Some(t);
        off += 20;
    }
    (keys, off - start)
}

fn find_length_prefixed_name(data: &[u8], name: &str, from_offset: usize) -> Option<usize> {
    let needle = name.as_bytes();
    if needle.is_empty() || from_offset >= data.len() {
        return None;
    }
    let len_bytes = (needle.len() as u32).to_le_bytes();
    let haystack = &data[from_offset..];
    let mut search_from = 0usize;
    while search_from + needle.len() <= haystack.len() {
        let rel = haystack[search_from..].windows(needle.len()).position(|w| w == needle)?;
        let name_off = search_from + rel;
        if name_off >= 4 && haystack[name_off - 4..name_off] == len_bytes {
            return Some(from_offset + name_off - 4);
        }
        search_from = name_off + 1;
    }
    None
}

const HEADER_KEY_COUNTS_OFFSET: usize = 112;
const MAX_PLAUSIBLE_KEYS: u32 = 100_000;

fn read_u32(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

fn read_header_key_counts(data: &[u8], name_off: usize) -> Option<[u32; 4]> {
    let header_off = name_off.checked_sub(TAIL_BLOCK_LEN)?;
    let mut counts = [0u32; 4];
    for (i, c) in counts.iter_mut().enumerate() {
        let v = read_u32(data, header_off + HEADER_KEY_COUNTS_OFFSET + i * 4)?;
        if v > MAX_PLAUSIBLE_KEYS {
            return None;
        }
        *c = v;
    }
    Some(counts)
}

fn read_vec3_time_keys(data: &[u8], start: usize, count: u32) -> Option<(Vec<[f32; 4]>, usize)> {
    let mut keys = Vec::with_capacity(count as usize);
    let mut off = start;
    for _ in 0..count {
        keys.push([read_f32(data, off)?, read_f32(data, off + 4)?, read_f32(data, off + 8)?, read_f32(data, off + 12)?]);
        off += 16;
    }
    Some((keys, off - start))
}

fn read_quat_time_keys(data: &[u8], start: usize, count: u32) -> Option<(Vec<[f32; 5]>, usize)> {
    let mut keys = Vec::with_capacity(count as usize);
    let mut off = start;
    for _ in 0..count {
        keys.push([
            read_f32(data, off)?,
            read_f32(data, off + 4)?,
            read_f32(data, off + 8)?,
            read_f32(data, off + 12)?,
            read_f32(data, off + 16)?,
        ]);
        off += 20;
    }
    Some((keys, off - start))
}

pub fn parse_motion(data: &[u8], bone_names: &[String]) -> Result<Vec<BoneMotion>> {
    let Some(xsm_off) = data.windows(4).position(|w| w == b"XSM ") else {
        bail!("xmot: not a recognized legacy motion clip (no 'XSM ' magic found)");
    };
    let payload_start = xsm_off + 8;

    let mut out = Vec::with_capacity(bone_names.len());
    for bone_name in bone_names {
        let Some(name_off) = find_length_prefixed_name(data, bone_name, payload_start) else {
            out.push(BoneMotion { bone_name: bone_name.clone(), ..Default::default() });
            continue;
        };
        let body_start = name_off + 4 + bone_name.len();

        if let Some([num_pos, num_rot, num_scale, _num_scale_rot]) = read_header_key_counts(data, name_off) {
            if let Some((position_keys, pos_len)) = read_vec3_time_keys(data, body_start, num_pos) {
                if let Some((rotation_keys, rot_len)) = read_quat_time_keys(data, body_start + pos_len, num_rot) {
                    if let Some((scale_keys, _)) = read_vec3_time_keys(data, body_start + pos_len + rot_len, num_scale) {
                        out.push(BoneMotion { bone_name: bone_name.clone(), position_keys, rotation_keys, scale_keys });
                        continue;
                    }
                }
            }
        }

        let (position_keys, pos_len) = scan_vec3_time_track(data, body_start, data.len());
        let (rotation_keys, rot_len) = scan_quat_time_track(data, body_start + pos_len, data.len());
        let (scale_keys, _) = scan_vec3_time_track(data, body_start + pos_len + rot_len, data.len());
        out.push(BoneMotion { bone_name: bone_name.clone(), position_keys, rotation_keys, scale_keys });
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordLocation {
    pub bone_name: String,
    pub pos_off: usize,
    pub num_pos: u32,
    pub rot_off: usize,
    pub num_rot: u32,
}

pub fn locate_records(data: &[u8], bone_names: &[String]) -> Result<Vec<RecordLocation>> {
    let Some(xsm_off) = data.windows(4).position(|w| w == b"XSM ") else {
        bail!("xmot: not a recognized legacy motion clip (no 'XSM ' magic found)");
    };
    let payload_start = xsm_off + 8;
    let mut out = Vec::new();
    for bone_name in bone_names {
        let Some(name_off) = find_length_prefixed_name(data, bone_name, payload_start) else { continue };
        let Some([num_pos, num_rot, _num_scale, _num_scale_rot]) = read_header_key_counts(data, name_off) else {
            continue;
        };
        let pos_off = name_off + 4 + bone_name.len();
        let rot_off = pos_off + num_pos as usize * 16;
        if rot_off + num_rot as usize * 20 > data.len() {
            bail!("xmot: record for '{bone_name}' extends past end of file");
        }
        out.push(RecordLocation { bone_name: bone_name.clone(), pos_off, num_pos, rot_off, num_rot });
    }
    Ok(out)
}

pub fn patch_motion_keys(data: &[u8], edits: &[BoneMotion]) -> Result<Vec<u8>> {
    let names: Vec<String> = edits.iter().map(|e| e.bone_name.clone()).collect();
    let locations = locate_records(data, &names)?;
    let mut out = data.to_vec();
    for edit in edits {
        let Some(loc) = locations.iter().find(|l| l.bone_name == edit.bone_name) else {
            if edit.position_keys.is_empty() && edit.rotation_keys.is_empty() {
                continue;
            }
            bail!("xmot: bone '{}' not found/patchable in this clip", edit.bone_name);
        };
        if edit.position_keys.len() != loc.num_pos as usize {
            bail!(
                "xmot: '{}' position key count {} != file's {} (in-place patch can't resize)",
                edit.bone_name,
                edit.position_keys.len(),
                loc.num_pos
            );
        }
        if edit.rotation_keys.len() != loc.num_rot as usize {
            bail!(
                "xmot: '{}' rotation key count {} != file's {} (in-place patch can't resize)",
                edit.bone_name,
                edit.rotation_keys.len(),
                loc.num_rot
            );
        }
        for (i, key) in edit.position_keys.iter().enumerate() {
            let off = loc.pos_off + i * 16;
            for (j, v) in key.iter().enumerate() {
                out[off + j * 4..off + j * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        for (i, key) in edit.rotation_keys.iter().enumerate() {
            let off = loc.rot_off + i * 20;
            for (j, v) in key.iter().enumerate() {
                out[off + j * 4..off + j * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq)]
struct FullRecordLocation {
    bone_name: String,
    header_off: usize,
    num_scale: u32,
    scale_off: usize,
    num_scale_rot: u32,
    scale_rot_off: usize,
    record_end: usize,
}

fn locate_full_records(data: &[u8], bone_names: &[String]) -> Result<Vec<FullRecordLocation>> {
    let Some(xsm_off) = data.windows(4).position(|w| w == b"XSM ") else {
        bail!("xmot: not a recognized legacy motion clip (no 'XSM ' magic found)");
    };
    let payload_start = xsm_off + 8;
    let mut out = Vec::new();
    for bone_name in bone_names {
        let Some(name_off) = find_length_prefixed_name(data, bone_name, payload_start) else { continue };
        let Some([num_pos, num_rot, num_scale, num_scale_rot]) = read_header_key_counts(data, name_off) else {
            continue;
        };
        let Some(header_off) = name_off.checked_sub(TAIL_BLOCK_LEN) else { continue };
        let pos_off = name_off + 4 + bone_name.len();
        let rot_off = pos_off + num_pos as usize * 16;
        let scale_off = rot_off + num_rot as usize * 20;
        let scale_rot_off = scale_off + num_scale as usize * 16;
        let record_end = scale_rot_off + num_scale_rot as usize * 20;
        if record_end > data.len() {
            bail!("xmot: record for '{bone_name}' extends past end of file");
        }
        out.push(FullRecordLocation { bone_name: bone_name.clone(), header_off, num_scale, scale_off, num_scale_rot, scale_rot_off, record_end });
    }
    out.sort_by_key(|r| r.header_off);
    if let Some(first) = out.first() {
        let mut cursor = first.header_off;
        for loc in &out {
            if loc.header_off != cursor {
                bail!(
                    "xmot: records aren't contiguous (gap/overlap at byte {cursor}, next record '{}' starts at {}) — bone_names is likely missing a real bone in this file",
                    loc.bone_name,
                    loc.header_off
                );
            }
            cursor = loc.record_end;
        }
        if cursor != data.len() {
            bail!("xmot: located records end at byte {cursor} but the file is {} bytes — bone_names is likely missing a real bone in this file", data.len());
        }
    }
    Ok(out)
}

pub fn rebuild_motion_file(data: &[u8], bone_names: &[String], edits: &[BoneMotion]) -> Result<Vec<u8>> {
    let Some(xsm_off) = data.windows(4).position(|w| w == b"XSM ") else {
        bail!("xmot: not a recognized legacy motion clip (no 'XSM ' magic found)");
    };
    let size_field_off = xsm_off
        .checked_sub(4)
        .ok_or_else(|| anyhow!("xmot: no room before 'XSM ' for the container's payload-size field"))?;

    let locations = locate_full_records(data, bone_names)?;
    let prefix_end = locations.first().map(|r| r.header_off).unwrap_or(data.len());

    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&data[..prefix_end]);

    for loc in &locations {
        let Some(edit) = edits.iter().find(|e| e.bone_name == loc.bone_name) else {
            out.extend_from_slice(&data[loc.header_off..loc.record_end]);
            continue;
        };

        let mut header = data[loc.header_off..loc.header_off + TAIL_BLOCK_LEN].to_vec();
        let counts = [edit.position_keys.len() as u32, edit.rotation_keys.len() as u32, loc.num_scale, loc.num_scale_rot];
        for (i, count) in counts.into_iter().enumerate() {
            let off = HEADER_KEY_COUNTS_OFFSET + i * 4;
            header[off..off + 4].copy_from_slice(&count.to_le_bytes());
        }
        out.extend_from_slice(&header);
        out.extend_from_slice(&(loc.bone_name.len() as u32).to_le_bytes());
        out.extend_from_slice(loc.bone_name.as_bytes());
        for key in &edit.position_keys {
            for v in key {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        for key in &edit.rotation_keys {
            for v in key {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out.extend_from_slice(&data[loc.scale_off..loc.scale_off + loc.num_scale as usize * 16]);
        out.extend_from_slice(&data[loc.scale_rot_off..loc.scale_rot_off + loc.num_scale_rot as usize * 20]);
    }

    let payload_size = u32::try_from(out.len() - xsm_off).context("xmot: rebuilt payload too large for its u32 size field")?;
    out[size_field_off..size_field_off + 4].copy_from_slice(&payload_size.to_le_bytes());
    Ok(out)
}

pub fn smooth_tracks(tracks: &[BoneMotion], strength: f32) -> Vec<BoneMotion> {
    let s = strength.clamp(0.0, 1.0);
    if s == 0.0 {
        return tracks.to_vec();
    }
    tracks
        .iter()
        .map(|t| {
            let mut out = t.clone();
            for i in 1..t.position_keys.len().saturating_sub(1) {
                let (prev, cur, next) = (t.position_keys[i - 1], t.position_keys[i], t.position_keys[i + 1]);
                for c in 0..3 {
                    let mid = (prev[c] + next[c]) * 0.5;
                    out.position_keys[i][c] = cur[c] + (mid - cur[c]) * s;
                }
            }
            for i in 1..t.rotation_keys.len().saturating_sub(1) {
                let cur = t.rotation_keys[i];
                let mut prev = t.rotation_keys[i - 1];
                let mut next = t.rotation_keys[i + 1];
                let align = |q: &mut [f32; 5], reference: &[f32; 5]| {
                    let dot: f32 = q[..4].iter().zip(&reference[..4]).map(|(a, b)| a * b).sum();
                    if dot < 0.0 {
                        for v in q[..4].iter_mut() {
                            *v = -*v;
                        }
                    }
                };
                align(&mut prev, &cur);
                align(&mut next, &cur);
                let mut blended = [0.0f32; 4];
                for c in 0..4 {
                    let mid = (prev[c] + next[c]) * 0.5;
                    blended[c] = cur[c] + (mid - cur[c]) * s;
                }
                let norm = blended.iter().map(|v| v * v).sum::<f32>().sqrt();
                if norm > 1e-6 {
                    for c in 0..4 {
                        out.rotation_keys[i][c] = blended[c] / norm;
                    }
                }
            }
            out
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoneRole {
    Locomotion,
    Secondary,
    Primary,
}

const LOCOMOTION_TOKENS: [&str; 5] = ["root", "leg", "foot", "toe", "hip"];
const SECONDARY_TOKENS: [&str; 6] = ["tail", "cloth", "ear", "hair", "brow", "belt"];

pub fn classify_bone(name: &str) -> BoneRole {
    let tokens: Vec<String> = name
        .split('_')
        .map(|t| t.to_lowercase())
        .map(|t| t.trim_end_matches(|c: char| c.is_ascii_digit()).to_string())
        .collect();
    if tokens.iter().any(|t| LOCOMOTION_TOKENS.contains(&t.as_str())) {
        BoneRole::Locomotion
    } else if tokens.iter().any(|t| SECONDARY_TOKENS.iter().any(|k| t.starts_with(k))) {
        BoneRole::Secondary
    } else {
        BoneRole::Primary
    }
}

fn slerp_quat(q0: [f32; 4], q1: [f32; 4], t: f32) -> [f32; 4] {
    let mut b = q1;
    let mut dot: f32 = (0..4).map(|c| q0[c] * b[c]).sum();
    if dot < 0.0 {
        for v in b.iter_mut() {
            *v = -*v;
        }
        dot = -dot;
    }
    let dot = dot.clamp(-1.0, 1.0);
    let mut r = if dot > 0.9995 {
        let mut lin = [0.0f32; 4];
        for c in 0..4 {
            lin[c] = q0[c] + (b[c] - q0[c]) * t;
        }
        lin
    } else {
        let theta0 = dot.acos();
        let sin_theta0 = theta0.sin();
        let theta = theta0 * t;
        let s0 = (theta0 - theta).sin() / sin_theta0;
        let s1 = theta.sin() / sin_theta0;
        let mut out = [0.0f32; 4];
        for c in 0..4 {
            out[c] = q0[c] * s0 + b[c] * s1;
        }
        out
    };
    let norm = r.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 1e-6 {
        for v in r.iter_mut() {
            *v /= norm;
        }
    }
    r
}

fn track_mean_quat(keys: &[[f32; 5]]) -> Option<[f32; 4]> {
    let reference = keys.first().map(|k| [k[0], k[1], k[2], k[3]])?;
    let mut sum = [0.0f32; 4];
    for k in keys {
        let mut q = [k[0], k[1], k[2], k[3]];
        let dot: f32 = (0..4).map(|c| q[c] * reference[c]).sum();
        if dot < 0.0 {
            for v in q.iter_mut() {
                *v = -*v;
            }
        }
        for c in 0..4 {
            sum[c] += q[c];
        }
    }
    let norm = sum.iter().map(|v| v * v).sum::<f32>().sqrt();
    (norm > 1e-6).then(|| [sum[0] / norm, sum[1] / norm, sum[2] / norm, sum[3] / norm])
}

fn track_mean_vec3(keys: &[[f32; 4]]) -> Option<[f32; 3]> {
    if keys.is_empty() {
        return None;
    }
    let mut sum = [0.0f32; 3];
    for k in keys {
        for c in 0..3 {
            sum[c] += k[c];
        }
    }
    let n = keys.len() as f32;
    Some([sum[0] / n, sum[1] / n, sum[2] / n])
}

pub fn boost_expressiveness(tracks: &[BoneMotion], amount: f32) -> Vec<BoneMotion> {
    let amount = amount.max(0.0);
    if amount == 0.0 {
        return tracks.to_vec();
    }
    tracks
        .iter()
        .map(|t| {
            if classify_bone(&t.bone_name) != BoneRole::Primary {
                return t.clone();
            }
            let mut out = t.clone();
            if let Some(mean) = track_mean_quat(&t.rotation_keys) {
                for (i, k) in t.rotation_keys.iter().enumerate() {
                    let q = slerp_quat(mean, [k[0], k[1], k[2], k[3]], 1.0 + amount);
                    out.rotation_keys[i] = [q[0], q[1], q[2], q[3], k[4]];
                }
            }
            if let Some(mean) = track_mean_vec3(&t.position_keys) {
                for (i, k) in t.position_keys.iter().enumerate() {
                    let mut p = [0.0f32; 4];
                    for c in 0..3 {
                        p[c] = mean[c] + (k[c] - mean[c]) * (1.0 + amount);
                    }
                    p[3] = k[3];
                    out.position_keys[i] = p;
                }
            }
            out
        })
        .collect()
}

pub fn secondary_motion(tracks: &[BoneMotion], lag_keys: usize, amount: f32) -> Vec<BoneMotion> {
    if lag_keys == 0 && amount <= 0.0 {
        return tracks.to_vec();
    }
    tracks
        .iter()
        .map(|t| {
            if classify_bone(&t.bone_name) != BoneRole::Secondary {
                return t.clone();
            }
            let mut out = t.clone();
            let mean_q = track_mean_quat(&t.rotation_keys);
            for i in 0..t.rotation_keys.len() {
                let src = t.rotation_keys[i.saturating_sub(lag_keys)];
                let q = match mean_q {
                    Some(mean) => slerp_quat(mean, [src[0], src[1], src[2], src[3]], 1.0 + amount),
                    None => [src[0], src[1], src[2], src[3]],
                };
                out.rotation_keys[i] = [q[0], q[1], q[2], q[3], t.rotation_keys[i][4]];
            }
            let mean_p = track_mean_vec3(&t.position_keys);
            for i in 0..t.position_keys.len() {
                let src = t.position_keys[i.saturating_sub(lag_keys)];
                let p3 = match mean_p {
                    Some(mean) => {
                        let mut v = [0.0f32; 3];
                        for c in 0..3 {
                            v[c] = mean[c] + (src[c] - mean[c]) * (1.0 + amount);
                        }
                        v
                    }
                    None => [src[0], src[1], src[2]],
                };
                out.position_keys[i] = [p3[0], p3[1], p3[2], t.position_keys[i][3]];
            }
            out
        })
        .collect()
}

pub fn retime_attack(tracks: &[BoneMotion], sharpness: f32) -> Vec<BoneMotion> {
    let s = sharpness.clamp(0.0, 1.0);
    if s == 0.0 {
        return tracks.to_vec();
    }
    let duration = tracks.iter().map(|t| t.duration()).fold(0.0f32, f32::max);
    if duration <= 0.0 {
        return tracks.to_vec();
    }
    let p = 1.0 - s * 0.6;
    let warp = |t: f32| -> f32 {
        let u = (t / duration).clamp(0.0, 1.0);
        duration * u.powf(p)
    };
    tracks
        .iter()
        .map(|t| {
            let mut out = t.clone();
            for k in out.position_keys.iter_mut() {
                k[3] = warp(k[3]);
            }
            for k in out.rotation_keys.iter_mut() {
                k[4] = warp(k[4]);
            }
            out
        })
        .collect()
}

pub fn stylize_tracks(tracks: &[BoneMotion], expressiveness: f32, secondary_amount: f32, attack_sharpness: f32) -> Vec<BoneMotion> {
    let mut out = tracks.to_vec();
    if expressiveness > 0.0 {
        out = boost_expressiveness(&out, expressiveness);
    }
    if secondary_amount > 0.0 {
        out = secondary_motion(&out, 2, secondary_amount);
    }
    if attack_sharpness > 0.0 {
        out = retime_attack(&out, attack_sharpness);
    }
    out
}

pub fn resample_double_rate(tracks: &[BoneMotion]) -> Vec<BoneMotion> {
    tracks
        .iter()
        .map(|t| {
            let mut position_keys = Vec::with_capacity(t.position_keys.len() * 2);
            for w in t.position_keys.windows(2) {
                position_keys.push(w[0]);
                let mut mid = [0.0f32; 4];
                for c in 0..4 {
                    mid[c] = (w[0][c] + w[1][c]) * 0.5;
                }
                position_keys.push(mid);
            }
            if let Some(&last) = t.position_keys.last() {
                position_keys.push(last);
            }
            let mut rotation_keys = Vec::with_capacity(t.rotation_keys.len() * 2);
            for w in t.rotation_keys.windows(2) {
                rotation_keys.push(w[0]);
                let q = slerp_quat([w[0][0], w[0][1], w[0][2], w[0][3]], [w[1][0], w[1][1], w[1][2], w[1][3]], 0.5);
                rotation_keys.push([q[0], q[1], q[2], q[3], (w[0][4] + w[1][4]) * 0.5]);
            }
            if let Some(&last) = t.rotation_keys.last() {
                rotation_keys.push(last);
            }
            BoneMotion { bone_name: t.bone_name.clone(), position_keys, rotation_keys, scale_keys: t.scale_keys.clone() }
        })
        .collect()
}

