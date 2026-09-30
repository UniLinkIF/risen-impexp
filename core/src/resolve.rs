//! Which model a placed entity draws with. A `.lrent` entity carries a TEMPLATE name; the template
//! (`.tple`) names its visual in `eCMesh_PS::MeshFileName` (a static mesh) or in
//! `ResourceFilePath` (`.xac` = an actor, `.spt` = a SpeedTree plant). Entities whose name is a mesh
//! name, or a template name with an `_S<n>` variant suffix, fall back to the name itself.

use crate::game::GameCtx;
use anyhow::Result;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Visual { StaticMesh(String), Actor(String), SpeedTree(String), None }

#[derive(Clone, Copy)]
enum Kind { Mesh, Actor, SpeedTree }

pub struct Resolver { templates: HashMap<String, (String, Kind)>, meshes: HashMap<String, String>, actors: HashMap<String, String>, speedtrees: HashMap<String, String> }

fn stem(path: &str) -> String { let f = path.rsplit(['/', '\\']).next().unwrap_or(path); f.split('.').next().unwrap_or(f).to_lowercase() }

/// `Name_S2` → `Name`.
fn strip_s_suffix(name: &str) -> Option<&str> {
    let (head, tail) = name.rsplit_once("_S")?;
    (!tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit())).then_some(head)
}

impl Resolver {
    pub fn build(g: &GameCtx) -> Result<Resolver> {
        let mut templates = HashMap::new();
        for e in g.entries_with_suffix(".tple") {
            let Ok((d, _)) = g.read(&e) else { continue };
            let (raw, kind) = match risen_formats::tple::find_string_property(&d, "MeshFileName") {
                Some(m) => (m, Kind::Mesh),
                None => match risen_formats::tple::find_string_property(&d, "ResourceFilePath") {
                    Some(r) if r.to_ascii_lowercase().ends_with(".spt") => (r, Kind::SpeedTree),
                    Some(r) if r.to_ascii_lowercase().ends_with(".xac") => (r, Kind::Actor),
                    _ => continue,
                },
            };
            let visual = stem(raw.trim());
            if !visual.is_empty() { templates.insert(stem(&e), (visual, kind)); }
        }
        let index = |suffix: &str| g.entries_with_suffix(suffix).into_iter().map(|e| (stem(&e), e)).collect::<HashMap<_, _>>();
        Ok(Resolver { templates, meshes: index("._xmsh"), actors: index("._xmac"), speedtrees: index("._xspt") })
    }

    pub fn resolve(&self, entity_name: &str) -> Visual {
        for candidate in [Some(entity_name), strip_s_suffix(entity_name)].into_iter().flatten() {
            let key = candidate.to_lowercase();
            if let Some((visual, kind)) = self.templates.get(&key) {
                let v = match kind {
                    Kind::Mesh => self.meshes.get(visual).map(|e| Visual::StaticMesh(e.clone())),
                    Kind::Actor => self.actors.get(visual).map(|e| Visual::Actor(e.clone())),
                    Kind::SpeedTree => self.speedtrees.get(visual).map(|e| Visual::SpeedTree(e.clone())),
                };
                if let Some(v) = v { return v; }
            }
            if let Some(e) = self.meshes.get(&key) { return Visual::StaticMesh(e.clone()); }
            if let Some(e) = self.actors.get(&key) { return Visual::Actor(e.clone()); }
        }
        Visual::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_placed_objects() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        let r = Resolver::build(&g).unwrap();
        match r.resolve("Obj_Flag_Wall_Monastery_01") { Visual::StaticMesh(e) => assert!(e.to_lowercase().ends_with("obj_flag_wall_monastery_01._xmsh"), "{e}"), other => panic!("{other:?}") }
        assert!(matches!(r.resolve("FP_Nonexistent_Marker_XYZ"), Visual::None));
    }
}
