use anyhow::{anyhow, bail, Context, Result};
use risen_formats::gamepath::{discover_archives, discover_game_root, resolve_shortcut};
use risen_formats::pak::{FileEntry, PakArchive};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin { Archive, Physical }

#[derive(Debug, Clone)]
struct Located { archive: PathBuf, group: String, stem: String, entry: FileEntry }

pub struct GameCtx {
    #[allow(dead_code)]
    pub exe: PathBuf,
    pub root: PathBuf,
    /// lowercase entry path -> location; later volumes (.p01 > .pak) override earlier ones.
    index: HashMap<String, Located>,
    /// Loose files no archive has (new meshes/textures, installed PM_* layers) in any archive's
    /// mirror folder `data/<group>/<stem>/`: lowercase entry -> (canonical entry, file).
    physical_only: HashMap<String, (String, PathBuf)>,
    open: RefCell<HashMap<PathBuf, PakArchive>>,
}

fn walk_physical(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { walk_physical(base, &p, out); continue; }
        if let Ok(rel) = p.strip_prefix(base) { out.push(format!("/{}", rel.to_string_lossy().replace('\\', "/"))); }
    }
}

impl GameCtx {
    pub fn open(exe: &Path) -> Result<Self> {
        let exe = resolve_shortcut(exe).with_context(|| format!("resolve {}", exe.display()))?;
        let root = discover_game_root(&exe).ok_or_else(|| anyhow!("no Risen data/ folder above {}", exe.display()))?;
        let mut archives = discover_archives(&root)?;
        // .pak first, then .p01, .p02 ... so later volumes override.
        archives.sort_by_key(|a| a.path.to_string_lossy().to_lowercase());
        let mut index = HashMap::new();
        for a in archives {
            let stem = a.path.file_stem().unwrap_or_default().to_string_lossy().to_string();
            let pak = PakArchive::open(&a.path).with_context(|| format!("open {}", a.path.display()))?;
            for e in pak.files() {
                if e.is_deleted() { index.remove(&e.path.to_lowercase()); continue; }
                index.insert(e.path.to_lowercase(), Located { archive: a.path.clone(), group: a.group.clone(), stem: stem.clone(), entry: e });
            }
        }
        let mut mirrors: Vec<PathBuf> = index.values().map(|l: &Located| root.join("data").join(&l.group).join(&l.stem)).collect();
        mirrors.sort();
        mirrors.dedup();
        let mut physical_only = HashMap::new();
        for dir in mirrors {
            let mut phys = vec![];
            walk_physical(&dir, &dir, &mut phys);
            for e in phys {
                if index.contains_key(&e.to_lowercase()) || e.ends_with(".rc-tmp") { continue; }
                let file = dir.join(e.trim_start_matches('/').replace('/', std::path::MAIN_SEPARATOR_STR));
                physical_only.insert(e.to_lowercase(), (e, file));
            }
        }
        Ok(GameCtx { exe, root, index, physical_only, open: RefCell::new(HashMap::new()) })
    }

    pub fn entries_with_suffix(&self, suffix: &str) -> Vec<String> {
        let s = suffix.to_lowercase();
        let mut v: Vec<String> = self.index.iter().filter(|(k, _)| k.ends_with(&s)).map(|(_, l)| l.entry.path.clone()).collect();
        v.extend(self.physical_only.iter().filter(|(k, _)| k.ends_with(&s)).map(|(_, (e, _))| e.clone()));
        v.sort();
        v
    }

    pub fn find_one(&self, suffix: &str) -> Result<String> {
        let v = self.entries_with_suffix(suffix);
        match v.len() { 1 => Ok(v[0].clone()), 0 => bail!("expected exactly one entry ending with {suffix}, found 0"), n => bail!("expected exactly one entry ending with {suffix}, found {n}: {}", v.iter().take(5).cloned().collect::<Vec<_>>().join(", ")) }
    }

    pub fn physical_path(&self, entry: &str) -> Result<PathBuf> {
        if let Some((_, file)) = self.physical_only.get(&entry.to_lowercase()) { return Ok(file.clone()); }
        let l = self.index.get(&entry.to_lowercase()).ok_or_else(|| anyhow!("unknown entry {entry}"))?;
        let rel = l.entry.path.trim_start_matches('/');
        Ok(self.root.join("data").join(&l.group).join(&l.stem).join(rel.replace('/', std::path::MAIN_SEPARATOR_STR)))
    }

    /// Where an entry's loose copy lives, relative to the game folder, with forward slashes.
    pub fn rel(&self, entry: &str) -> Result<String> {
        let p = self.physical_path(entry)?;
        Ok(p.strip_prefix(&self.root)?.to_string_lossy().replace('\\', "/"))
    }

    /// Physical file first (what the game itself reads with NoPhysical=false), then the archive.
    pub fn read(&self, entry: &str) -> Result<(Vec<u8>, Origin)> {
        let phys = self.physical_path(entry)?;
        if phys.is_file() { return Ok((std::fs::read(&phys)?, Origin::Physical)); }
        if !self.index.contains_key(&entry.to_lowercase()) { bail!("entry {entry} is gone: its physical file was removed"); }
        Ok((self.read_archive(entry)?, Origin::Archive))
    }

    /// The archive bytes only, ignoring any physical override (what the vanilla library indices refer to).
    pub fn read_archive(&self, entry: &str) -> Result<Vec<u8>> {
        let l = self.index.get(&entry.to_lowercase()).ok_or_else(|| anyhow!("entry {entry} is not in a game archive"))?;
        let mut open = self.open.borrow_mut();
        if !open.contains_key(&l.archive) { open.insert(l.archive.clone(), PakArchive::open(&l.archive)?); }
        let pak = open.get_mut(&l.archive).unwrap();
        pak.read_file(&l.entry).with_context(|| format!("read {entry}"))
    }
}


/// The game for tests that need real data: `RISEN_GAME` = the folder holding `bin\Risen.exe`.
#[cfg(test)]
pub fn test_game() -> Option<GameCtx> {
    let dir = std::env::var_os("RISEN_GAME")?;
    Some(GameCtx::open(&Path::new(&dir).join("bin").join("Risen.exe")).expect("RISEN_GAME points at a readable game"))
}
