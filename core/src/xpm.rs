//! Lip-sync clips (`._xmot` under `infos/`, EMotionFX "XPM "): how far each mouth shape of the
//! face is on over time, generated from the voice line.
//!
//! ```text
//! resource  GR01MO01 eCMotionResource2 · data: "XPM " u8 hi u8 lo u8 endian u8 pad · chunks
//! chunk     u32 id · u32 size · u32 version · body
//!   101 info: u32 fps · exporter, source file (the .wav), date, name strings
//!   102 sub-motions (its size field is not the size: read through to the end): u32 n · then
//!       per sub-motion:
//!       f32 pose weight · f32 min · f32 max · u32 phoneme set · u32 keys · u32 length · name
//!       · (f32 time, u16 value, u16 pad)[keys]       weight = min + (max - min) · value / 65535
//! ```
//! A sub-motion drives every morph target whose phoneme set shares a bit with its own
//! (`morph::Target::phonemes`): BS_AA_AO_OW is 0x14, so both the AA and the AW lines move it.

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SubMotion { pub phonemes: u32, pub name: String, pub keys: Vec<(f32, f32)> }

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LipSync { pub fps: u32, pub sound: String, pub subs: Vec<SubMotion> }

pub fn read(resource: &[u8]) -> Result<LipSync> {
    let Some(at) = resource.windows(4).position(|w| w == b"XPM ") else { bail!("not a lip-sync clip (no XPM data)") };
    let d = &resource[at + 8..];
    let u32_at = |o: usize| -> Result<u32> { Ok(u32::from_le_bytes(d.get(o..o + 4).context("lip-sync clip runs short")?.try_into().unwrap())) };
    let f32_at = |o: usize| -> Result<f32> { Ok(f32::from_bits(u32_at(o)?)) };
    let string_at = |o: usize| -> Result<(String, usize)> { let n = u32_at(o)? as usize; Ok((crate::gr01::latin1(d.get(o + 4..o + 4 + n).context("string runs short")?), o + 4 + n)) };
    let (mut out, mut o) = (LipSync { fps: 30, sound: String::new(), subs: vec![] }, 0);
    while o + 12 <= d.len() {
        let (id, size) = (u32_at(o)?, u32_at(o + 4)? as usize);
        let body = o + 12;
        match id {
            101 => {
                out.fps = u32_at(body)?;
                // fps · two u8 exporter versions + pad · exporter name · source file
                let (_, p) = string_at(body + 8)?;
                out.sound = string_at(p)?.0;
            }
            102 => {
                let n = u32_at(body)?;
                let mut p = body + 4;
                for _ in 0..n {
                    let (lo, hi, phonemes, keys) = (f32_at(p + 4)?, f32_at(p + 8)?, u32_at(p + 12)?, u32_at(p + 16)? as usize);
                    let (name, k) = string_at(p + 20)?;
                    let keys = (0..keys).map(|i| -> Result<(f32, f32)> {
                        let v = u32_at(k + i * 8 + 4)? & 0xffff;
                        Ok((f32_at(k + i * 8)?, lo + (hi - lo) * v as f32 / 65535.0))
                    }).collect::<Result<Vec<_>>>()?;
                    p = k + keys.len() * 8;
                    out.subs.push(SubMotion { phonemes, name, keys });
                }
                o = p;
                continue;
            }
            _ => {}
        }
        o = body + size;
    }
    Ok(out)
}

/// Per morph target (name, phoneme set): its weight keys, the sub-motions that share a phoneme
/// bit merged (the strongest wins at each time).
pub fn for_targets(clip: &LipSync, targets: &[(String, u32)]) -> Vec<(String, Vec<(f32, f32)>)> {
    let sample = |keys: &[(f32, f32)], t: f32| -> f32 {
        match keys.iter().position(|k| k.0 >= t) {
            None => keys.last().map(|k| k.1).unwrap_or(0.0),
            Some(0) => keys[0].1,
            Some(i) => { let (a, b) = (keys[i - 1], keys[i]); if b.0 - a.0 < 1e-6 { b.1 } else { a.1 + (b.1 - a.1) * (t - a.0) / (b.0 - a.0) } }
        }
    };
    targets.iter().filter(|(_, ph)| *ph != 0).filter_map(|(name, ph)| {
        let subs: Vec<&SubMotion> = clip.subs.iter().filter(|s| s.phonemes & ph != 0 && !s.keys.is_empty()).collect();
        if subs.is_empty() { return None; }
        let mut times: Vec<f32> = subs.iter().flat_map(|s| s.keys.iter().map(|k| k.0)).collect();
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        times.dedup_by(|a, b| (*a - *b).abs() < 1e-5);
        Some((name.clone(), times.iter().map(|&t| (t, subs.iter().map(|s| sample(&s.keys, t)).fold(0f32, f32::max))).collect()))
    }).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn reads_lipsync() {
        let g = crate::game::test_game().unwrap();
        let (mut n, mut bad) = (0, 0);
        for e in g.entries_with_suffix("._xmot").into_iter().filter(|e| e.starts_with("/infos/")).step_by(97) {
            n += 1;
            match super::read(&g.read(&e).unwrap().0) {
                Ok(c) => if n == 1 { eprintln!("{e}: {} fps, {}, {:?}", c.fps, c.sound, c.subs.iter().map(|s| (s.phonemes, s.keys.len())).collect::<Vec<_>>()) },
                Err(err) => { bad += 1; eprintln!("{e}: {err:#}"); }
            }
        }
        eprintln!("{n} clips, {bad} unreadable");
        assert_eq!(bad, 0);
    }
}
