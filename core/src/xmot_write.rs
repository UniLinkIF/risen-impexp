//! Writing Risen 1 motion clips (`._xmot`).
//!
//! ```text
//! GR01MO01  eCMotionResource2 { Effects: (key frame, effect name)* }   ← kept from the template
//! data      u32 1 · u32 bytes from "XSM " to the end · "XSM " u8 hi u8 lo u8 endian u8 pad
//!           chunk 201 (u32 id, u32 size, u32 version 2): clip info (fps, exporter, source) ← template
//!           chunk 202 (u32 id, u32 field, u32 version 1) · u32 submotions · submotion*
//! submotion f32 poseRot[4] bindPoseRot[4] poseScaleRot[4] bindScaleRot[4]
//!           posePos[3] poseScale[3] bindPos[3] bindScale[3]
//!           u32 #pos #rot #scale #scaleRot · f32 maxError · u32 name length · name
//!           pos keys (x y z t) · rot keys (x y z w t) · scale keys · scale-rotation keys
//! ```
//! Chunk 202's `field` is not its size (shipped clips carry 1.5–4× the real byte count and no
//! formula over counts fits); the engine loads clips whatever it holds, so we keep the template's
//! ratio, generously. New keys come from a glTF made by Blender's exporter, converted back through
//! the actor importer's axes (mirror in Z, metres → centimetres).

use crate::actor::{conv_p, conv_q};
use crate::gr01::Resource;
use anyhow::{bail, ensure, Context, Result};
use risen_formats::xmac::SkeletonNode;
use risen_formats::xmot::BoneMotion;

const HEADER: usize = 132;

struct Sub { name: String, header: Vec<u8> }

struct Xsm { lead: [u8; 16], info: Vec<u8>, field: u32, subs: Vec<Sub>, payload: usize }

fn u32_at(d: &[u8], a: usize) -> Result<u32> { Ok(u32::from_le_bytes(d.get(a..a + 4).context("motion clip runs short")?.try_into().unwrap())) }

fn read_xsm(data: &[u8]) -> Result<Xsm> {
    ensure!(data.len() > 32 && &data[8..12] == b"XSM ", "not an XSM motion");
    let lead: [u8; 16] = data[..16].try_into().unwrap();
    ensure!(u32_at(data, 16)? == 201, "no info chunk");
    let info_end = 28 + u32_at(data, 20)? as usize;
    let info = data.get(16..info_end).context("info chunk runs short")?.to_vec();
    ensure!(u32_at(data, info_end)? == 202, "no submotion chunk");
    let field = u32_at(data, info_end + 4)?;
    let n = u32_at(data, info_end + 12)?;
    let mut at = info_end + 16;
    let mut subs = vec![];
    for _ in 0..n {
        let header = data.get(at..at + HEADER).context("submotion header runs short")?.to_vec();
        let c: Vec<usize> = (0..4).map(|k| u32_at(&header, 112 + k * 4).map(|v| v as usize)).collect::<Result<_>>()?;
        let nl = u32_at(data, at + HEADER)? as usize;
        let name = String::from_utf8_lossy(data.get(at + HEADER + 4..at + HEADER + 4 + nl).context("name runs short")?).into_owned();
        at += HEADER + 4 + nl + c[0] * 16 + c[1] * 20 + c[2] * 16 + c[3] * 20;
        subs.push(Sub { name, header });
    }
    ensure!(at == data.len(), "submotions end at {at} of {}", data.len());
    Ok(Xsm { lead, info, field, subs, payload: data.len() - 8 })
}

fn bind_header(n: &SkeletonNode) -> Vec<u8> {
    let mut h = vec![];
    for q in [n.rotation, n.rotation, [0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0]] { for x in q { h.extend(x.to_le_bytes()); } }
    for v in [n.position, [1.0; 3], n.position, [1.0; 3]] { for x in v { h.extend(x.to_le_bytes()); } }
    h.extend([0u8; 16]);
    h.extend(f32::MAX.to_le_bytes());
    h
}

/// A clip for `nodes` with `motion` (game space), shaped like `template` (a clip of the same
/// actor: its info chunk, events and per-bone headers are kept).
pub fn write_clip(template: &[u8], nodes: &[SkeletonNode], motion: &[BoneMotion]) -> Result<Vec<u8>> {
    let mut res = Resource::parse(template).context("template clip")?;
    ensure!(&res.magic == b"GR01MO01", "template is not a motion clip");
    let x = read_xsm(&res.data)?;
    // Template order first (the engine's own order for this actor), then skeleton bones it lacks.
    let mut order: Vec<(String, Vec<u8>)> = x.subs.iter().map(|s| (s.name.clone(), s.header.clone())).collect();
    for n in nodes { if !order.iter().any(|(name, _)| name == &n.name) { order.push((n.name.clone(), bind_header(n))); } }
    let mut recs = vec![];
    for (name, header) in &order {
        let empty = BoneMotion::default();
        let m = motion.iter().find(|m| &m.bone_name == name).unwrap_or(&empty);
        let mut h = header.clone();
        for (k, c) in [m.position_keys.len(), m.rotation_keys.len(), m.scale_keys.len(), 0].into_iter().enumerate() { h[112 + k * 4..116 + k * 4].copy_from_slice(&(c as u32).to_le_bytes()); }
        recs.extend(h);
        recs.extend((name.len() as u32).to_le_bytes());
        recs.extend(name.as_bytes());
        for k in &m.position_keys { for v in k { recs.extend(v.to_le_bytes()); } }
        for k in &m.rotation_keys { for v in k { recs.extend(v.to_le_bytes()); } }
        for k in &m.scale_keys { for v in k { recs.extend(v.to_le_bytes()); } }
    }
    let mut data = x.lead.to_vec();
    data.extend(&x.info);
    let payload = data.len() - 8 + 16 + recs.len();
    let ratio = (x.field as f64 / x.payload.max(1) as f64).max(4.0);
    data.extend(202u32.to_le_bytes());
    data.extend(((payload as f64 * ratio) as u32).max(x.field).to_le_bytes());
    data.extend(1u32.to_le_bytes());
    data.extend((order.len() as u32).to_le_bytes());
    data.extend(recs);
    let size = (data.len() - 8) as u32;
    data[4..8].copy_from_slice(&size.to_le_bytes());
    res.data = data;
    res.filetime = crate::cache::filetime(std::time::SystemTime::now()).to_le_bytes();
    Ok(res.write())
}

/// One animation of a glb (by name, or the first) as game-space tracks, times from 0. Tracks that
/// only hold the bind value are dropped (Blender's exporter keys every bone).
pub fn tracks_from_glb(glb: &[u8], nodes: &[SkeletonNode], anim: Option<&str>) -> Result<(String, Vec<BoneMotion>)> {
    let g = gltf::Gltf::from_slice(glb).context("read glb")?;
    let blob = g.blob.clone().context("glb has no binary chunk")?;
    let a = match anim {
        Some(n) => g.animations().find(|a| a.name() == Some(n)).with_context(|| format!("no animation {n} in the glb"))?,
        None => g.animations().next().context("the glb has no animation")?,
    };
    let name = a.name().unwrap_or("Motion").to_string();
    let mut out: Vec<BoneMotion> = vec![];
    let mut t0 = f32::MAX;
    for ch in a.channels() {
        let r = ch.reader(|_| Some(&blob));
        if let Some(t) = r.read_inputs() { t0 = t0.min(t.fold(f32::MAX, f32::min)); }
    }
    if t0 == f32::MAX { bail!("animation {name} has no keys"); }
    for ch in a.channels() {
        let Some(bone) = ch.target().node().name().map(String::from) else { continue };
        let Some(node) = nodes.iter().find(|n| n.name == bone) else { continue };
        let r = ch.reader(|_| Some(&blob));
        let times: Vec<f32> = r.read_inputs().context("channel without times")?.map(|t| t - t0).collect();
        let idx = match out.iter().position(|m| m.bone_name == bone) { Some(i) => i, None => { out.push(BoneMotion { bone_name: bone.clone(), ..Default::default() }); out.len() - 1 } };
        use gltf::animation::util::ReadOutputs;
        match r.read_outputs().context("channel without values")? {
            ReadOutputs::Translations(v) => {
                let keys: Vec<[f32; 4]> = v.zip(&times).map(|(p, &t)| { let g = conv_p(p, 1.0 / crate::actor::UNIT); [g[0], g[1], g[2], t] }).collect();
                let still = keys.iter().all(|k| (0..3).all(|i| (k[i] - node.position[i]).abs() < 1e-2));
                if !still { out[idx].position_keys = keys; }
            }
            ReadOutputs::Rotations(v) => {
                let mut prev: Option<[f32; 4]> = None;
                out[idx].rotation_keys = v.into_f32().zip(&times).map(|(q, &t)| {
                    let mut g = conv_q(q);
                    if let Some(p) = prev { if g.iter().zip(&p).map(|(a, b)| a * b).sum::<f32>() < 0.0 { g = g.map(|x| -x); } }
                    prev = Some(g);
                    [g[0], g[1], g[2], g[3], t]
                }).collect();
            }
            ReadOutputs::Scales(v) => {
                let keys: Vec<[f32; 4]> = v.zip(&times).map(|(s, &t)| [s[0], s[1], s[2], t]).collect();
                if !keys.iter().all(|k| (0..3).all(|i| (k[i] - 1.0).abs() < 1e-4)) { out[idx].scale_keys = keys; }
            }
            ReadOutputs::MorphTargetWeights(_) => {}
        }
    }
    out.retain(|m| !(m.position_keys.is_empty() && m.rotation_keys.is_empty() && m.scale_keys.is_empty()));
    Ok((name, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor;

    /// Shipped clip → our actor glb → back to a clip: every key within float noise of the original.
    #[test]
    fn clip_survives_the_glb_round_trip() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        let clips = actor::ClipIndex::build(&g);
        for (actor_name, query) in [("Ani_Wolf_Monster_Wolf", "attack_02"), ("Ani_Hero_Armor_Player", "move_run_n_fwd")] {
            let entry = actor::resolve_actor(&g, actor_name).unwrap();
            let (clip, ce) = clips.for_actor(&actor::stem(&entry), query).into_iter().next().unwrap();
            let bytes = g.read(&entry).unwrap().0;
            let nodes = risen_formats::xmac::parse_skeleton(&bytes).unwrap();
            let mesh = risen_formats::xmesh_skin::parse_skinned_mesh(&bytes).unwrap();
            let names: Vec<String> = nodes.iter().map(|n| n.name.clone()).collect();
            let orig_bytes = g.read(&ce).unwrap().0;
            let orig = risen_formats::xmot::parse_motion(&orig_bytes, &names).unwrap();
            let (glb, _) = actor::build_glb("t", &nodes, &[actor::Part { mesh: &mesh, joint_map: (0..nodes.len()).collect() }], &[(clip.clone(), orig.clone())], &mut |_, _| Ok(None)).unwrap();
            let (_, tracks) = tracks_from_glb(&glb, &nodes, None).unwrap();
            let written = write_clip(&orig_bytes, &nodes, &tracks).unwrap();
            let back = risen_formats::xmot::parse_motion(&written, &names).unwrap();
            let (mut worst_r, mut worst_p, mut n) = (0f32, 0f32, 0);
            for o in &orig {
                let Some(b) = back.iter().find(|b| b.bone_name == o.bone_name) else { continue };
                if o.rotation_keys.len() == b.rotation_keys.len() {
                    for (x, y) in o.rotation_keys.iter().zip(&b.rotation_keys) {
                        let d = (x[0] * y[0] + x[1] * y[1] + x[2] * y[2] + x[3] * y[3]).abs();
                        worst_r = worst_r.max(1.0 - d);
                        n += 1;
                    }
                } else if !o.rotation_keys.is_empty() { panic!("{clip} {}: {} rotation keys became {}", o.bone_name, o.rotation_keys.len(), b.rotation_keys.len()); }
                for (x, y) in o.position_keys.iter().zip(&b.position_keys) { worst_p = worst_p.max((0..3).map(|i| (x[i] - y[i]).abs()).fold(0.0, f32::max)); }
            }
            eprintln!("{clip}: {n} rotation keys, worst 1-|q·q'| {worst_r:e}, worst position {worst_p} cm, {} → {} bytes", orig_bytes.len(), written.len());
            assert!(n > 100 && worst_r < 1e-5 && worst_p < 0.01);
        }
    }
}

#[derive(serde::Deserialize)]
pub struct MotionSpec {
    /// The actor the motion is for (`Ani_…`), as imported into Blender.
    pub actor: String,
    /// Clip name to write: an existing clip is replaced, a new name adds one.
    pub clip: String,
    /// glb exported by Blender, and the animation in it (none = the first).
    pub glb: String,
    pub animation: Option<String>,
}

#[derive(serde::Serialize)]
pub struct MotionReport { pub clip: String, pub path: String, pub replaced: bool, pub bones: usize, pub keys: usize, pub duration: f32, pub template: String }

pub fn build(g: &crate::game::GameCtx, spec: &MotionSpec) -> Result<(MotionReport, Vec<(String, Vec<u8>)>)> {
    ensure!(!spec.clip.is_empty() && spec.clip.chars().all(|c| c.is_ascii_alphanumeric() || "_%-".contains(c)), "clip name {:?}: letters, digits, _ % - only", spec.clip);
    let entry = crate::actor::resolve_actor(g, &spec.actor)?;
    let nodes = risen_formats::xmac::parse_skeleton(&g.read(&entry)?.0).context("skeleton")?;
    let clips = crate::actor::ClipIndex::build(g);
    let existing = clips.for_actor(&crate::actor::stem(&entry), "").into_iter().find(|(n, _)| n.eq_ignore_ascii_case(&spec.clip));
    let (tname, tentry) = match &existing { Some(c) => c.clone(), None => clips.idle_for(&crate::actor::stem(&entry)).with_context(|| format!("{}: no clip of this actor to shape a new one on", spec.actor))? };
    let (_, tracks) = tracks_from_glb(&std::fs::read(&spec.glb).with_context(|| format!("read {}", spec.glb))?, &nodes, spec.animation.as_deref())?;
    ensure!(!tracks.is_empty(), "the animation moves none of {}'s bones (bone names must match the skeleton)", spec.actor);
    let bytes = write_clip(&g.read(&tentry)?.0, &nodes, &tracks)?;
    let tpath = g.rel(&tentry)?;
    let path = match &existing { Some(_) => tpath, None => format!("{}/{}._xmot", tpath.rsplit_once('/').map(|x| x.0).unwrap_or("data/compiled/animations"), spec.clip) };
    let keys = tracks.iter().map(|t| t.position_keys.len() + t.rotation_keys.len() + t.scale_keys.len()).sum();
    let duration = tracks.iter().map(|t| t.duration()).fold(0.0, f32::max);
    Ok((MotionReport { clip: spec.clip.clone(), path: path.clone(), replaced: existing.is_some(), bones: tracks.len(), keys, duration, template: tname }, vec![(path, bytes)]))
}
