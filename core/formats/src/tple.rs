//! The Genome property-token stream shared by .tple templates and resource files.

use serde::Serialize;
use std::collections::HashSet;

fn known_schema_field(s: &str) -> bool {
    const NAMES: &[&str] = &[
        "Amount", "GoldValue", "MissionItem", "Permanent", "SortValue", "Category", "IconImage",
        "HoldOffset", "Dropped", "ItemWorld", "ItemInventory", "Spell", "RequiredSkills",
        "ModifySkills", "Modifier", "Skill", "UseType", "CanEquipScript", "EquipScript",
        "UnEquipScript", "EffectMaterial", "HoldType", "IsDangerousWeapon", "CombatHitRangeOffset",
        "EnterROIScript", "ExitROIScript", "TouchScript", "IntersectScript", "UntouchScript",
        "TriggerScript", "UntriggerScript", "DamageScript", "CanAttachSlotScript",
        "AttachedSlotScript", "DetachedSlotScript", "RoomChangedScript", "RoutineTask",
        "GroundBias", "FocusPriority", "FocusNameType", "FocusNameBone", "FocusViewOffset",
        "FocusWorldOffset", "FocusPriorityScript", "Slots", "InteractionCounter", "Owner",
        "Type", "CanInteractScript", "PreInteractScript", "InteractScript", "PostInteractScript",
        "CanInteract_Magic_Item", "PreInteract_Magic_Item", "CanQuickUse_Player",
        "QuickUse_Player", "NavTestResult", "StaticIlluminated", "ShadowCasterType",
        "CastDirLightShadows", "CastPntLightShadows", "CastStaticShadows",
        "CastDirLightShadowsOverwrite", "CastPntLightShadowsOverwrite",
        "CastStaticShadowsOverwrite", "MeshFileName", "MaterialSwitch", "SubMeshCulling",
        "Lightmaped", "EnableRadiosity", "MaxSubMeshTriangles", "UnitsPerLightmapTexel",
        "LevelOfDetailRange0", "LevelOfDetailRange1", "LevelOfDetailRange2", "EnableDecals",
        "HitByProjectile", "TotalMass", "MassSpaceInertia", "StartVelocity",
        "StartAngularVelocity", "StartForce", "StartTorque", "WakeUpCounter", "LinearDamping",
        "AngularDamping", "MaxAngularVelocity", "CenterOfMass", "CCDMotionTreshold", "BodyFlag",
        "PhysicsEnabled", "Group", "Range", "DisableCollision", "DisableResponse",
        "IgnoredByTraceRay", "IsUnique", "IsClimbable", "ShapeType", "Material",
        "ShapeAABBAdaptMode", "EnableCCD", "OverrideEntityAABB", "TriggersOnTouch",
        "TriggersOnUntouch", "TriggersOnIntersect", "SkinWidth", "IsLazyGenerated", "FileVersion",
        "eCScriptProxyScript", "gCScriptProxyAIFunction", "gCScriptProxyAIState", "eCEntityProxy",
        "eCTemplateEntityProxy", "eCGuiBitmapProxy2", "bCString", "bCVector", "bool", "int",
        "long", "short", "float",
    ];
    NAMES.contains(&s)
}

const INTERESTING_HOOKS: &[&str] =
    &["CanInteractScript", "PreInteractScript", "InteractScript", "PostInteractScript"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BindingStatus {
    Bound,
    Unbound,
    Undetermined,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptBinding {
    pub property: String,
    pub bound_value: Option<String>,
    pub status: BindingStatus,
}

pub fn pool_strings(data: &[u8]) -> Vec<String> {
    read_tokens(data).into_iter().map(|(_, s)| s).collect()
}

pub(crate) fn read_tokens(data: &[u8]) -> Vec<(usize, String)> {
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i + 2 <= data.len() {
        let len = u16::from_le_bytes([data[i], data[i + 1]]) as usize;
        if len > 0 && i + 2 + len <= data.len() {
            let chunk = &data[i + 2..i + 2 + len];
            if chunk.iter().all(|&b| (0x20..0x7f).contains(&b)) {
                let s = String::from_utf8_lossy(chunk).into_owned();
                tokens.push((i, s));
                i += 2 + len;
                continue;
            }
        }
        i += 1;
    }
    tokens
}

fn is_string_type_marker(token: &str) -> bool {
    token.starts_with("bC") && token.ends_with("String")
}

pub fn find_string_property(data: &[u8], property_name: &str) -> Option<String> {
    let tokens = read_tokens(data);
    let idx = tokens.iter().position(|(_, name)| name == property_name)?;
    let (_, next) = tokens.get(idx + 1)?;
    if is_string_type_marker(next) {
        return tokens.get(idx + 2).map(|(_, value)| value.clone());
    }
    Some(next.clone())
}

pub fn replace_pool_string(
    data: &[u8],
    old_value: &str,
    new_value: &str,
    occurrence: usize,
) -> Result<Vec<u8>, String> {
    let tokens = read_tokens(data);
    let matches: Vec<&(usize, String)> =
        tokens.iter().filter(|(_, s)| s == old_value).collect();
    let Some(&(offset, _)) = matches.get(occurrence).copied() else {
        return Err(format!(
            "'{old_value}' occurrence {occurrence} not found (only {} match(es))",
            matches.len()
        ));
    };
    let old_len = old_value.len();
    let new_bytes = new_value.as_bytes();
    let mut out = Vec::with_capacity(data.len() - old_len + new_bytes.len());
    out.extend_from_slice(&data[..offset]);
    out.extend_from_slice(&(new_bytes.len() as u16).to_le_bytes());
    out.extend_from_slice(new_bytes);
    out.extend_from_slice(&data[offset + 2 + old_len..]);
    Ok(out)
}

pub fn scan_script_bindings(data: &[u8]) -> Vec<ScriptBinding> {
    let tokens = read_tokens(data);
    let mut out = Vec::new();
    let hooks: HashSet<&str> = INTERESTING_HOOKS.iter().copied().collect();
    for (idx, (_, name)) in tokens.iter().enumerate() {
        if !hooks.contains(name.as_str()) {
            continue;
        }
        let bound_value = tokens.get(idx + 1).and_then(|(_, next)| {
            let is_schema = next.contains('<') || known_schema_field(next);
            if is_schema {
                None
            } else {
                Some(next.clone())
            }
        });
        let status = if bound_value.is_some() { BindingStatus::Bound } else { BindingStatus::Undetermined };
        out.push(ScriptBinding { property: name.clone(), bound_value, status });
    }
    out
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionBlock {
    pub interaction_type: Option<String>,
    pub bindings: Vec<ScriptBinding>,
}

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    data.get(offset..offset + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn find_index_occurrences(data: &[u8], value: u16) -> Vec<usize> {
    let needle = value.to_le_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 2 <= data.len() {
        if data[i] == needle[0] && data[i + 1] == needle[1] {
            out.push(i);
        }
        i += 1;
    }
    out
}

const UNBOUND_GAP: i64 = 13;
const BOUND_GAP: i64 = 15;
const POST_TO_NEXT_TYPE_UNBOUND_GAP: i64 = 40;
const POST_TO_NEXT_TYPE_BOUND_GAP: i64 = 42;

pub fn scan_interaction_blocks(data: &[u8]) -> Vec<InteractionBlock> {
    if data.len() < 14 || &data[0..8] != b"GENOMFLE" {
        return Vec::new();
    }
    let Some(pool_off_field) = data.get(10..14).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize) else {
        return Vec::new();
    };
    let pool_start = 5 + pool_off_field;
    if pool_start + 4 > data.len() {
        return Vec::new();
    }
    let count = u32::from_le_bytes([
        data[pool_start],
        data[pool_start + 1],
        data[pool_start + 2],
        data[pool_start + 3],
    ]);
    let mut strings = Vec::with_capacity(count as usize);
    let mut off = pool_start + 4;
    for _ in 0..count {
        let Some(len) = u16_at(data, off) else { break };
        off += 2;
        let len = len as usize;
        let Some(bytes) = data.get(off..off + len) else { break };
        strings.push(String::from_utf8_lossy(bytes).into_owned());
        off += len;
    }
    let index_of = |name: &str| strings.iter().position(|s| s == name).map(|i| i as u16);

    let (Some(gci_idx), Some(type_idx), Some(can_idx), Some(pre_idx), Some(interact_idx), Some(post_idx)) = (
        index_of("gCInteraction"),
        index_of("Type"),
        index_of("CanInteractScript"),
        index_of("PreInteractScript"),
        index_of("InteractScript"),
        index_of("PostInteractScript"),
    ) else {
        return Vec::new();
    };

    let content = &data[14..pool_start];
    let block_starts = find_index_occurrences(content, gci_idx);
    let type_positions = find_index_occurrences(content, type_idx);

    let resolve = |idx: u16| strings.get(idx as usize).cloned();

    let first_after = |needle_idx: u16, after: usize| -> Option<usize> {
        find_index_occurrences(content, needle_idx).into_iter().find(|&p| p >= after)
    };

    let mut out = Vec::with_capacity(block_starts.len());
    for (block_i, &block_start) in block_starts.iter().enumerate() {
        let type_pos = first_after(type_idx, block_start);
        let interaction_type = type_pos.map(|_| format!("slot {block_i}"));

        let can_pos = first_after(can_idx, block_start);
        let pre_pos = first_after(pre_idx, block_start);
        let interact_pos = first_after(interact_idx, block_start);
        let post_pos = first_after(post_idx, block_start);

        let mut bindings = Vec::with_capacity(4);
        let mut push_binding =
            |property: &str, pos: Option<usize>, next_pos: Option<usize>, gap_unbound: i64, gap_bound: i64| {
                let Some(pos) = pos else { return };
                let (bound_value, status) = match next_pos {
                    None => (None, BindingStatus::Undetermined),
                    Some(next) => {
                        let delta = next as i64 - pos as i64;
                        if delta == gap_bound {
                            match u16_at(content, next - 2).and_then(resolve) {
                                Some(value) => (Some(value), BindingStatus::Bound),
                                None => (None, BindingStatus::Undetermined),
                            }
                        } else if delta == gap_unbound {
                            (None, BindingStatus::Unbound)
                        } else {
                            (None, BindingStatus::Undetermined)
                        }
                    }
                };
                bindings.push(ScriptBinding { property: property.to_string(), bound_value, status });
            };

        push_binding("CanInteractScript", can_pos, pre_pos, UNBOUND_GAP, BOUND_GAP);
        push_binding("PreInteractScript", pre_pos, interact_pos, UNBOUND_GAP, BOUND_GAP);
        push_binding("InteractScript", interact_pos, post_pos, UNBOUND_GAP, BOUND_GAP);

        let next_block_type_pos = type_positions.iter().find(|&&p| Some(p) > post_pos).copied();
        push_binding(
            "PostInteractScript",
            post_pos,
            next_block_type_pos,
            POST_TO_NEXT_TYPE_UNBOUND_GAP,
            POST_TO_NEXT_TYPE_BOUND_GAP,
        );

        out.push(InteractionBlock { interaction_type, bindings });
    }
    out
}

pub fn bind_interaction_hook(
    data: &[u8],
    block_index: usize,
    property: &str,
    new_value: &str,
) -> Result<Vec<u8>, String> {
    if data.len() < 14 || &data[0..8] != b"GENOMFLE" {
        return Err("not a GENOMFLE file".to_string());
    }
    let pool_off_field = data
        .get(10..14)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        .ok_or("truncated GENOMFLE header")?;
    let pool_start = 5 + pool_off_field;
    if pool_start + 4 > data.len() {
        return Err("pool offset out of range".to_string());
    }
    let count = u32::from_le_bytes([
        data[pool_start],
        data[pool_start + 1],
        data[pool_start + 2],
        data[pool_start + 3],
    ]);
    let mut strings: Vec<String> = Vec::with_capacity(count as usize);
    let mut off = pool_start + 4;
    for _ in 0..count {
        let len = u16_at(data, off).ok_or("truncated pool")? as usize;
        off += 2;
        let bytes = data.get(off..off + len).ok_or("truncated pool entry")?;
        strings.push(String::from_utf8_lossy(bytes).into_owned());
        off += len;
    }
    let index_of = |name: &str| strings.iter().position(|s| s == name).map(|i| i as u16);
    let gci_idx = index_of("gCInteraction").ok_or("no gCInteraction blocks in this file")?;
    let type_idx = index_of("Type").ok_or("malformed: gCInteraction with no Type field")?;
    let can_idx = index_of("CanInteractScript").ok_or("no CanInteractScript field")?;
    let pre_idx = index_of("PreInteractScript").ok_or("no PreInteractScript field")?;
    let interact_idx = index_of("InteractScript").ok_or("no InteractScript field")?;
    let post_idx = index_of("PostInteractScript").ok_or("no PostInteractScript field")?;

    let content = &data[14..pool_start];
    let block_starts = find_index_occurrences(content, gci_idx);
    let &block_start = block_starts
        .get(block_index)
        .ok_or_else(|| format!("block {block_index} not found (only {} block(s))", block_starts.len()))?;
    let type_positions = find_index_occurrences(content, type_idx);

    let first_after = |needle_idx: u16, after: usize| -> Option<usize> {
        find_index_occurrences(content, needle_idx).into_iter().find(|&p| p >= after)
    };
    let can_pos = first_after(can_idx, block_start);
    let pre_pos = first_after(pre_idx, block_start);
    let interact_pos = first_after(interact_idx, block_start);
    let post_pos = first_after(post_idx, block_start);
    let next_block_type_pos = type_positions.iter().find(|&&p| Some(p) > post_pos).copied();

    let (pos, anchor, gap_unbound, gap_bound) = match property {
        "CanInteractScript" => (can_pos, pre_pos, UNBOUND_GAP, BOUND_GAP),
        "PreInteractScript" => (pre_pos, interact_pos, UNBOUND_GAP, BOUND_GAP),
        "InteractScript" => (interact_pos, post_pos, UNBOUND_GAP, BOUND_GAP),
        "PostInteractScript" => {
            (post_pos, next_block_type_pos, POST_TO_NEXT_TYPE_UNBOUND_GAP, POST_TO_NEXT_TYPE_BOUND_GAP)
        }
        other => return Err(format!("unknown property '{other}' (expected one of the four interaction hooks)")),
    };
    let pos = pos.ok_or_else(|| format!("{property} not found in block {block_index}"))?;
    let anchor = anchor.ok_or_else(|| {
        format!("{property} in block {block_index} has no resolvable anchor to bind against (likely the file's last block/hook)")
    })?;
    let delta = anchor as i64 - pos as i64;

    let value_index = match strings.iter().position(|s| s == new_value) {
        Some(i) => i as u16,
        None => strings.len() as u16,
    };
    let needs_new_pool_entry = value_index as usize == strings.len();

    let mut new_content = content.to_vec();
    if delta == gap_bound {
        new_content[anchor - 2..anchor].copy_from_slice(&value_index.to_le_bytes());
    } else if delta == gap_unbound {
        new_content.splice(anchor..anchor, value_index.to_le_bytes());
    } else {
        return Err(format!(
            "{property} in block {block_index} has an unexpected gap ({delta}, expected {gap_unbound} unbound or {gap_bound} bound) — layout assumption doesn't hold for this file, refusing to guess"
        ));
    }

    let mut out = Vec::with_capacity(data.len() + 2);
    out.extend_from_slice(b"GENOMFLE");
    out.extend_from_slice(&data[8..10]);
    let new_pool_off_field = (new_content.len() + 9) as u32;
    out.extend_from_slice(&new_pool_off_field.to_le_bytes());
    out.extend_from_slice(&new_content);
    let new_count = if needs_new_pool_entry { count + 1 } else { count };
    out.extend_from_slice(&new_count.to_le_bytes());
    for s in &strings {
        out.extend_from_slice(&(s.len() as u16).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }
    if needs_new_pool_entry {
        out.extend_from_slice(&(new_value.len() as u16).to_le_bytes());
        out.extend_from_slice(new_value.as_bytes());
    }
    Ok(out)
}

