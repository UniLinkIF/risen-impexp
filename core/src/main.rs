//! risen-core: the native half of the Risen ImpExp Blender add-on. Every command prints one JSON
//! value on stdout; errors go to stderr with exit code 1.
//!
//! ```text
//! risen-core find <game_dir> <query> [limit]        meshes whose name contains <query>
//! risen-core mesh <game_dir> <name|entry> <out_dir> ._xmsh -> <out_dir>/<stem>.obj/.mtl + textures/
//! risen-core export <game_dir> <spec.json> package <dir> [title]   build a mod package in <dir>
//! risen-core export <game_dir> <spec.json> install <mod>           install into the game (registered)
//! risen-core export-motion <game_dir> <spec.json> package <dir> [title] | install <mod>
//! risen-core export-actor <game_dir> <spec.json> package <dir> [title] | install <mod>
//! risen-core layers <game_dir> <query>  ·  world-layer <game_dir> <layer> <out_dir>
//! risen-core landscape-get <game_dir> <out_dir> [current|archive]  ·  landscape-set <game_dir> <positions.bin> package <dir> | install <mod>
//! risen-core landscape-set-paint <game_dir> <positions.bin> <paint.bin> package <dir> [title] | install <mod>   heights + ground materials
//! risen-core uninstall <game_dir> <mod>  ·  risen-core installed <game_dir>
//! risen-core trees <game_dir> <query>  ·  tree <game_dir> <name> <out_dir>   SpeedTree stand-ins
//! risen-core actors <game_dir> <query> [limit]  ·  clips <game_dir> <actor> <query> [limit]
//! risen-core actor <game_dir> <name> <out.glb> [clip query: "" idle, "*" none] [limit] [skeleton|full] [head|-]
//! risen-core collision <game_dir> <mesh name|xcom> <out.obj>   a collision mesh for viewing
//! risen-core dump <game_dir> <entry|file>  debug view of a GR01 resource
//! ```

mod game;
mod actor;
mod cache;
mod cook;
mod export;
mod landscape;
mod landscape_paint;
mod modpkg;
mod nxs;
mod gr01;
mod mesh;
mod xmsh_geom;
mod xmsh_write;
mod ximg_write;
mod xmot_write;
mod xmac_write;
mod morph;
mod xpm;
mod skin_export;
mod speedtree;
mod terrain;
mod terrain_col;
#[allow(dead_code)]
mod resolve;
mod world;

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// The add-on stores the game folder (the one holding `bin\Risen.exe`); `GameCtx` wants the exe.
fn open_game(dir: &str) -> Result<game::GameCtx> {
    let p = Path::new(dir);
    let exe = if p.is_file() { p.to_path_buf() } else { p.join("bin").join("Risen.exe") };
    game::GameCtx::open(&exe).with_context(|| format!("open Risen at {dir}"))
}

#[derive(serde::Serialize)]
struct Found { name: String, entry: String }

fn find(g: &game::GameCtx, suffix: &str, query: &str, limit: usize) -> Vec<Found> {
    let q = query.to_lowercase();
    g.entries_with_suffix(suffix).into_iter()
        .map(|entry| Found { name: entry.rsplit('/').next().unwrap().split('.').next().unwrap().to_string(), entry })
        .filter(|f| f.name.to_lowercase().contains(&q))
        .take(limit)
        .collect()
}

fn run(args: &[String]) -> Result<serde_json::Value> {
    let a = |i: usize| args.get(i).map(String::as_str);
    Ok(match (a(0), a(1), a(2), a(3)) {
        (Some("find"), Some(game), Some(q), limit) => {
            let limit = limit.map(str::parse).transpose().context("limit")?.unwrap_or(200);
            serde_json::to_value(find(&open_game(game)?, "._xmsh", q, limit))?
        }
        (Some("tree"), Some(game), Some(name), Some(out)) => {
            let g = open_game(game)?;
            serde_json::to_value(speedtree::export_obj(&g, &mesh::TextureIndex::build(&g), name, Path::new(out))?)?
        }
        (Some("trees"), Some(game), Some(q), _) => serde_json::to_value(find(&open_game(game)?, "._xspt", q, 100000))?,
        (Some("actors"), Some(game), Some(q), limit) => {
            let limit = limit.map(str::parse).transpose().context("limit")?.unwrap_or(200);
            serde_json::to_value(find(&open_game(game)?, "._xmac", q, limit))?
        }
        (Some("clips"), Some(game), Some(actor_name), Some(q)) => {
            let g = open_game(game)?;
            let limit = a(4).map(str::parse).transpose().context("limit")?.unwrap_or(500);
            let clips = actor::ClipIndex::build(&g);
            serde_json::to_value(clips.for_actor(&actor::stem(&actor::resolve_actor(&g, actor_name)?), q).into_iter().take(limit).map(|(n, _)| n).collect::<Vec<_>>())?
        }
        (Some("lipclips"), Some(game), Some(q), _) => {
            let g = open_game(game)?;
            let limit = a(3).map(str::parse).transpose().context("limit")?.unwrap_or(200);
            let words: Vec<String> = q.to_lowercase().split_whitespace().map(String::from).collect();
            serde_json::to_value(g.entries_with_suffix("._xmot").into_iter().filter(|e| e.starts_with("/infos/")).map(|e| actor::stem(&e))
                .filter(|s| { let l = s.to_lowercase(); words.iter().all(|w| l.contains(w.as_str())) }).take(limit).collect::<Vec<_>>())?
        }
        (Some("lipsync"), Some(game), Some(head), Some(clip)) => {
            // lipsync <game> <head actor> <clip> [cache dir]: weight keys per morph target, and the voice line as .mp3
            let g = open_game(game)?;
            let h = xmac_write::read(&g.read(&actor::resolve_actor(&g, head)?)?.0)?;
            let body = h.sections.iter().find_map(|s| match s { xmac_write::Section::Raw { id: 12, body, .. } => Some(body), _ => None }).context("this head has no face shapes")?;
            let targets: Vec<(String, u32)> = morph::read(body)?.targets.into_iter().map(|t| (t.name, t.phonemes)).collect();
            let entry = g.find_one(&format!("/{}._xmot", clip.trim_end_matches("._xmot")))?;
            let ls = xpm::read(&g.read(&entry)?.0)?;
            let channels = xpm::for_targets(&ls, &targets);
            let duration = ls.subs.iter().flat_map(|s| s.keys.last()).map(|k| k.0).fold(0f32, f32::max);
            let mut sound = None;
            if let (Some(dir), Ok(se)) = (a(4), g.find_one(&format!("{}._xsnd", entry.trim_end_matches("._xmot")))) {
                let r = gr01::Resource::parse(&g.read(&se)?.0)?;
                let p = Path::new(dir).join(format!("{}.mp3", actor::stem(&se)));
                std::fs::create_dir_all(dir)?;
                std::fs::write(&p, &r.data)?;
                sound = Some(p.to_string_lossy().into_owned());
            }
            serde_json::json!({ "clip": actor::stem(&entry), "fps": ls.fps, "duration": duration, "sound": sound, "channels": channels })
        }
        (Some("actor"), Some(game), Some(name), Some(out)) => {
            let g = open_game(game)?;
            let entry = actor::resolve_actor(&g, name)?;
            let clips = actor::ClipIndex::build(&g);
            let limit = a(5).map(str::parse).transpose().context("limit")?.unwrap_or(40);
            let picked = actor::pick_clips(&clips, &actor::stem(&entry), a(4).unwrap_or(""), limit);
            let with_textures = a(6) != Some("skeleton");
            let head = match a(7) { Some("-") => None, Some(h) => Some(h.to_string()), None => actor::default_head(&actor::stem(&entry)).map(String::from) };
            let cache = PathBuf::from(out).parent().map(|p| p.to_path_buf()).unwrap_or_default();
            serde_json::to_value(actor::write_glb(&g, &mesh::TextureIndex::build(&g), &entry, head.as_deref(), &picked, with_textures, Path::new(out), &cache)?)?
        }
        (Some("raw"), Some(game), Some(entry), Some(out)) => {
            let g = open_game(game)?;
            let e = if entry.starts_with('/') { entry.to_string() } else { g.find_one(&format!("/{entry}"))? };
            std::fs::write(out, g.read(&e)?.0)?;
            serde_json::json!(e)
        }
        (Some("list"), Some(game), Some(suffix), _) => {
            let g = open_game(game)?;
            serde_json::to_value(g.entries_with_suffix(suffix).into_iter().map(|e| { let n = g.read(&e).map(|d| d.0.len()).unwrap_or(0); (e, n) }).collect::<Vec<_>>())?
        }
        (Some("collision"), Some(game), Some(name), Some(out)) => {
            let g = open_game(game)?;
            let e = g.find_one(&format!("/{}", if name.to_lowercase().ends_with("._xcom") { name.to_string() } else { format!("{name}_COL._xcom") }))?;
            let x = nxs::read_xcom(&g.read(&e)?.0)?;
            let mut obj = String::from("# Risen collision, game axes mirrored in Z, centimetres
");
            let mut base = 1;
            let mut tris = 0;
            for (k, m) in x.meshes.iter().enumerate() {
                let mat = x.shape_materials.get(k).map(|s| nxs::SHAPE_MATERIALS.get(s.material as usize).copied().unwrap_or("none")).unwrap_or("none");
                obj += &format!("o {}_{k}
usemtl {mat}
", actor::stem(&e));
                for v in &m.verts { obj += &format!("v {} {} {}
", v[0] * 100.0, v[1] * 100.0, -v[2] * 100.0); }
                for t in &m.tris { obj += &format!("f {} {} {}
", t[0] + base, t[1] + base, t[2] + base); tris += 1; }
                base += m.verts.len() as u32;
            }
            std::fs::write(out, obj)?;
            serde_json::json!({ "entry": e, "obj": out, "meshes": x.meshes.len(), "triangles": tris, "materials": x.shape_materials.iter().map(|s| nxs::SHAPE_MATERIALS.get(s.material as usize).copied().unwrap_or("none")).collect::<Vec<_>>() })
        }
        (Some("dump"), Some(game), Some(entry), _) => {
            let bytes = if Path::new(entry).is_file() { std::fs::read(entry)? } else { let g = open_game(game)?; let e = if entry.starts_with('/') { entry.to_string() } else { g.find_one(&format!("/{entry}"))? }; g.read(&e)?.0 };
            let r = gr01::Resource::parse(&bytes)?;
            let hex = |v: &[u8]| v.iter().map(|x| format!("{x:02x}")).collect::<String>();
            serde_json::json!({ "entry": entry, "magic": gr01::latin1(&r.magic), "prefix": hex(&r.section.prefix), "class": r.section.class, "mid": hex(&r.section.mid), "root": gr01::to_json(&r.section.root), "tail": hex(&r.section.tail), "data_len": r.data.len(), "data_head": hex(&r.data[..r.data.len().min(0xd0)]), "data_last": hex(&r.data[r.data.len().saturating_sub(4)..]) })
        }
        (Some("mesh"), Some(game), Some(name), Some(out)) => {
            let g = open_game(game)?;
            let tex = mesh::TextureIndex::build(&g);
            serde_json::to_value(mesh::export_obj(&g, &tex, name, &PathBuf::from(out))?)?
        }
        (Some("export"), Some(game), Some(spec), Some(mode)) => {
            let g = open_game(game)?;
            let spec: export::Spec = serde_json::from_slice(&std::fs::read(spec).with_context(|| format!("read {spec}"))?)?;
            let built = export::build(&g, &spec)?;
            let target = a(4).context("export: missing target")?;
            let done = match mode {
                "package" => modpkg::write_package(&g, Path::new(target), a(5).unwrap_or(&spec.name), &built.files)?,
                "install" => modpkg::install(&g, target, &built.files)?,
                m => bail!("export mode {m}: use package or install"),
            };
            serde_json::json!({ "report": built.report, "done": done })
        }
        (Some("export-motion"), Some(game), Some(spec), Some(mode)) => {
            let g = open_game(game)?;
            let spec: xmot_write::MotionSpec = serde_json::from_slice(&std::fs::read(spec).with_context(|| format!("read {spec}"))?)?;
            let (report, files) = xmot_write::build(&g, &spec)?;
            let target = a(4).context("export-motion: missing target")?;
            let done = match mode {
                "package" => modpkg::write_package(&g, Path::new(target), a(5).unwrap_or(&spec.clip), &files)?,
                "install" => modpkg::install(&g, target, &files)?,
                m => bail!("export mode {m}: use package or install"),
            };
            serde_json::json!({ "report": report, "done": done })
        }
        (Some("export-actor"), Some(game), Some(spec), Some(mode)) => {
            let g = open_game(game)?;
            let spec: skin_export::ActorSpec = serde_json::from_slice(&std::fs::read(spec).with_context(|| format!("read {spec}"))?)?;
            let (report, files) = skin_export::build(&g, &spec)?;
            let target = a(4).context("export-actor: missing target")?;
            let done = match mode {
                "package" => modpkg::write_package(&g, Path::new(target), a(5).unwrap_or(&spec.name), &files)?,
                "install" => modpkg::install(&g, target, &files)?,
                m => bail!("export mode {m}: use package or install"),
            };
            serde_json::json!({ "report": report, "done": done })
        }
        (Some("layers"), Some(game), Some(q), _) => serde_json::to_value(world::layers(&open_game(game)?, q))?,
        (Some("world-layer"), Some(game), Some(name), Some(out)) => serde_json::to_value(world::layer(&open_game(game)?, name, Path::new(out))?)?,
        (Some("landscape-get"), Some(game), Some(out), which) => {
            let current = match which { None | Some("archive") => false, Some("current") => true, Some(w) => bail!("landscape-get: {w}? use current or archive") };
            serde_json::to_value(landscape::get(&open_game(game)?, Path::new(out), current)?)?
        }
        (Some("landscape-set"), Some(game), Some(bin), Some(mode)) => {
            let g = open_game(game)?;
            let (report, files) = landscape::set(&g, Path::new(bin), None)?;
            let target = a(4).context("landscape-set: missing target")?;
            let done = match mode {
                "package" => modpkg::write_package(&g, Path::new(target), a(5).unwrap_or("Landscape"), &files)?,
                "install" => modpkg::install(&g, target, &files)?,
                m => bail!("mode {m}: use package or install"),
            };
            serde_json::json!({ "report": report, "done": done })
        }
        (Some("landscape-set-paint"), Some(game), Some(bin), Some(paint)) => {
            let g = open_game(game)?;
            let (report, files) = landscape::set(&g, Path::new(bin), Some(Path::new(paint)))?;
            let target = a(5).context("landscape-set-paint: missing target")?;
            let done = match a(4) {
                Some("package") => modpkg::write_package(&g, Path::new(target), a(6).unwrap_or("Landscape"), &files)?,
                Some("install") => modpkg::install(&g, target, &files)?,
                m => bail!("mode {m:?}: use package or install"),
            };
            serde_json::json!({ "report": report, "done": done })
        }
        (Some("uninstall"), Some(game), Some(name), _) => serde_json::to_value(modpkg::uninstall(&open_game(game)?, name)?)?,
        (Some("installed"), Some(game), _, _) => serde_json::to_value(modpkg::installed(&open_game(game)?)?)?,
        _ => bail!("usage: see the top of core/src/main.rs (find, mesh, export, uninstall, installed, dump)"),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(v) => println!("{v}"),
        Err(e) => { eprintln!("risen-core: {e:#}"); std::process::exit(1); }
    }
}
