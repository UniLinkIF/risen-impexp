//! A world layer (`.lrent`) for Blender, as context to build against: every placed entity with its
//! transform, and one OBJ per distinct static mesh (written once, shared by all its placements).
//! Placement itself is a world editor's job; nothing here writes a layer.
//!
//! Matrices are the layer's row-vector ones (`world = local · M`, translation in [12..15], cm);
//! the add-on turns them into Blender's axes the same way the mesh importer turns vertices.

use crate::game::GameCtx;
use crate::resolve::{Resolver, Visual};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

#[derive(serde::Serialize)]
pub struct Placed { pub name: String, pub matrix: [f32; 16], pub kind: &'static str, pub mesh: Option<String> }

#[derive(serde::Serialize)]
pub struct Layer { pub layer: String, pub entities: Vec<Placed>, pub meshes: HashMap<String, String>, pub warnings: Vec<String> }

pub fn layers(g: &GameCtx, query: &str) -> Vec<String> {
    let q = query.to_lowercase();
    g.entries_with_suffix(".lrent").into_iter().map(|e| e.rsplit('/').next().unwrap().trim_end_matches(".lrent").to_string()).filter(|n| n.to_lowercase().contains(&q)).collect()
}

pub fn layer(g: &GameCtx, name: &str, out: &Path) -> Result<Layer> {
    let entry = if name.starts_with('/') { name.to_string() } else { g.find_one(&format!("/{}.lrent", name.trim_end_matches(".lrent")))? };
    let doc = risen_formats::lrent::LrentDoc::parse(&g.read(&entry)?.0).map_err(|e| anyhow::anyhow!("{e:?}")).with_context(|| format!("parse {entry}"))?;
    let r = Resolver::build(g)?;
    let tex = crate::mesh::TextureIndex::build(g);
    let (mut entities, mut meshes, mut warnings) = (vec![], HashMap::new(), vec![]);
    for e in doc.entities() {
        let (kind, mesh) = match r.resolve(&e.name) {
            Visual::StaticMesh(m) => {
                let stem = crate::actor::stem(&m);
                if !meshes.contains_key(&stem) {
                    match crate::mesh::export_obj(g, &tex, &m, out) {
                        Ok(o) => { meshes.insert(stem.clone(), o.obj); }
                        Err(err) => warnings.push(format!("{}: {err:#}", e.name)),
                    }
                }
                ("mesh", Some(stem))
            }
            Visual::Actor(a) => ("actor", Some(crate::actor::stem(&a))),
            Visual::SpeedTree(t) => ("speedtree", Some(crate::actor::stem(&t))),
            Visual::None => ("none", None),
        };
        entities.push(Placed { name: e.name.clone(), matrix: e.matrix, kind, mesh });
    }
    Ok(Layer { layer: entry, entities, meshes, warnings })
}
