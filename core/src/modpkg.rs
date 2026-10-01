//! Getting built files into the game, two ways.
//!
//! * A **mod package** to hand to other players: `files/` mirrors the game folder (resources plus
//!   the three directories = the archive's + our records), with `INSTALL.bat` / `ROLLBACK.bat`.
//!   It needs nothing but Windows. INSTALL refuses to overwrite a file it did not put there, so
//!   two packages that both carry directories cannot silently undo each other.
//! * **Install into this game** from Blender: every mod installed this way is listed in
//!   `%LOCALAPPDATA%\RisenImpExp\<hash of the canonical game folder>\installed.json`, and the directories are rebuilt from the
//!   archive plus the files of *all* listed mods, so mods add up and uninstall cleanly.
//!
//! Both rely on the game reading loose files (`NoPhysical=false` in `bin\mountlist_packed.ini`).

use crate::export::{self, file_mtime};
use crate::game::GameCtx;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn win(rel: &str) -> String { rel.replace('/', "\\") }

/// A path as it stands in a batch file: `%` starts a variable there (clip names carry `_%_`), so it is doubled.
fn bat(rel: &str) -> String { win(rel).replace('%', "%%") }

pub fn write_package(g: &GameCtx, dir: &Path, title: &str, files: &[(String, Vec<u8>)]) -> Result<Vec<String>> {
    let fdir = dir.join("files");
    if fdir.exists() { std::fs::remove_dir_all(&fdir).with_context(|| format!("clear {}", fdir.display()))?; }
    export::write_all(&fdir, files)?;
    let dirs = export::directories(g, files, false, |rel| file_mtime(&fdir.join(rel)))?;
    export::write_all(&fdir, &dirs)?;
    let all: Vec<String> = files.iter().chain(&dirs).map(|(p, _)| p.clone()).collect();
    let list = all.iter().map(|p| format!("\"{}\"", bat(p))).collect::<Vec<_>>().join(" ");
    std::fs::write(dir.join("INSTALL.bat"), INSTALL.replace("{TITLE}", title).replace("{FILES}", &list).replace('\n', "\r\n"))?;
    std::fs::write(dir.join("ROLLBACK.bat"), ROLLBACK.replace("{TITLE}", title).replace("{FILES}", &list).replace('\n', "\r\n"))?;
    std::fs::write(dir.join("README.txt"), README.replace("{TITLE}", title).replace("{FILES}", &all.iter().map(|p| format!("  {}", win(p))).collect::<Vec<_>>().join("\r\n")).replace('\n', "\r\n"))?;
    Ok(all)
}

// Batch notes: %GAME% holds "(x86)", so it is only ever used inside quotes; `copy` keeps the file
// time, which the directory records carry.
const INSTALL: &str = r#"@echo off
setlocal
rem {TITLE} - made with Risen ImpExp (Blender). Copies the files below into Risen; archives are never touched.
rem Refuses and changes nothing if one of the files already exists and is not this mod's. Undo with ROLLBACK.bat.
set "GAME=C:\Program Files (x86)\Steam\steamapps\common\Risen"
if not "%~1"=="" set "GAME=%~1"
set "HERE=%~dp0files"
set "INI=%GAME%\bin\mountlist_packed.ini"
set FILES={FILES}
if not exist "%GAME%\bin\Risen.exe" (set "MSG=Risen not found at %GAME% - run INSTALL.bat "<game folder>"" & goto :fail)
tasklist /FI "IMAGENAME eq Risen.exe" | "%SystemRoot%\System32\find.exe" /I "Risen.exe" >nul && (set "MSG=Close Risen first." & goto :fail)
set "TODO=0"
for %%F in (%FILES%) do (
  if not exist "%HERE%\%%~F" (set "MSG=Package file missing: %%~F" & goto :fail)
  if exist "%GAME%\%%~F" (
    fc /b "%HERE%\%%~F" "%GAME%\%%~F" >nul || (set "MSG=Refusing: %%~F already exists and belongs to another mod. Nothing changed." & goto :fail)
  ) else set "TODO=1"
)
if "%TODO%"=="0" (echo Already installed. & goto :done)
findstr /R /C:"^NoPhysical=true" "%INI%" >nul && (
  copy /Y "%INI%" "%INI%.risenimpexp-stock" >nul || (set "MSG=Could not back up mountlist_packed.ini" & goto :fail)
  powershell -NoProfile -Command "$p='%INI%'; $t=[IO.File]::ReadAllText($p); [IO.File]::WriteAllText($p, ($t -replace '(?m)^NoPhysical=true','NoPhysical=false'))" || (set "MSG=Could not edit mountlist_packed.ini" & goto :fail)
  echo NoPhysical=false set - stock file kept as mountlist_packed.ini.risenimpexp-stock
)
for %%F in (%FILES%) do (
  if not exist "%GAME%\%%~F" (
    for %%P in ("%GAME%\%%~F") do if not exist "%%~dpP" mkdir "%%~dpP"
    copy /Y "%HERE%\%%~F" "%GAME%\%%~F" >nul || (set "MSG=Copy failed: %%~F" & goto :fail)
    fc /b "%HERE%\%%~F" "%GAME%\%%~F" >nul || (set "MSG=Copy verification failed: %%~F" & goto :fail)
    echo Installed %%~F
  )
)
echo.
echo Done: {TITLE}. Start Risen.
goto :done
:fail
echo.
echo %MSG%
echo.
pause
exit /b 1
:done
echo.
pause
exit /b 0
"#;

const ROLLBACK: &str = r#"@echo off
setlocal
rem Removes {TITLE}: deletes each file only if it is still this mod's copy.
set "GAME=C:\Program Files (x86)\Steam\steamapps\common\Risen"
if not "%~1"=="" set "GAME=%~1"
set "HERE=%~dp0files"
set "INI=%GAME%\bin\mountlist_packed.ini"
set FILES={FILES}
tasklist /FI "IMAGENAME eq Risen.exe" | "%SystemRoot%\System32\find.exe" /I "Risen.exe" >nul && (echo Close Risen first. & pause & exit /b 1)
for %%F in (%FILES%) do (
  if exist "%GAME%\%%~F" (
    fc /b "%HERE%\%%~F" "%GAME%\%%~F" >nul && (del "%GAME%\%%~F" && echo Removed %%~F) || echo Kept %%~F - it was changed after install
  )
)
if exist "%INI%.risenimpexp-stock" (move /Y "%INI%.risenimpexp-stock" "%INI%" >nul && echo mountlist_packed.ini restored)
echo.
echo Done.
pause
"#;

const README: &str = "{TITLE}\n\nMade with Risen ImpExp (Blender add-on).\n\nInstall: close Risen, run INSTALL.bat (or INSTALL.bat \"<game folder>\" for a non-Steam install).\nRemove: ROLLBACK.bat.\n\nFiles copied into the game folder:\n{FILES}\n";

// ---- install into this game, with a registry so several mods add up ----

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Registry { /// mod name → game-relative files (resources only; directories are derived)
    mods: BTreeMap<String, Vec<String>>, dirs: Vec<String>,
    /// The canonical game folder this registry belongs to (absent in registries before 0.9.2).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    root: String,
    /// Directories another mod (an INSTALL.bat package) had already put in the game: loose path → our
    /// saved copy of it. Our records are built on that copy instead of the archive, so its records
    /// stay; when no mod of ours is left the copy goes back.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    bases: BTreeMap<String, String> }

fn fnv(s: &str) -> String { format!("{:08x}", s.bytes().fold(0x811c9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x01000193))) }

/// One spelling per game folder: the registry is keyed by it, and the same folder reached as
/// `C:/…/Risen`, `c:\…\RISEN\`, through `bin\Risen.exe` or an 8.3 name must find the SAME registry,
/// or an install is "lost" and every later one refuses over its files.
fn canonical_root(root: &Path) -> String {
    let p = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let s = p.to_string_lossy().replace('/', "\\");
    let s = if let Some(unc) = s.strip_prefix("\\\\?\\UNC\\") { format!("\\\\{unc}") } else { s.strip_prefix("\\\\?\\").unwrap_or(&s).to_string() };
    s.trim_end_matches('\\').to_lowercase()
}

fn registry_base() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("RisenImpExp")
}

fn registry_path_in(base: &Path, root: &Path) -> PathBuf { base.join(fnv(&canonical_root(root))).join("installed.json") }

/// Registries written before the key was canonical (hash of the root as spelled, lowercased): every
/// spelling this game folder is likely to have been reached by.
fn legacy_paths(base: &Path, root: &Path) -> Vec<PathBuf> {
    let mut forms = vec![root.to_string_lossy().into_owned()];
    if let Ok(c) = std::fs::canonicalize(root) { let c = c.to_string_lossy().into_owned(); forms.push(c.strip_prefix("\\\\?\\").unwrap_or(&c).to_string()); forms.push(c); }
    let mut spellings = vec![];
    for f in forms {
        for s in [f.clone(), f.replace('/', "\\"), f.replace('\\', "/")] {
            let t = s.trim_end_matches(['\\', '/']).to_string();
            spellings.extend([t.clone(), format!("{t}\\"), format!("{t}/")]);
        }
    }
    let new = registry_path_in(base, root);
    let mut out: Vec<PathBuf> = spellings.into_iter().map(|s| base.join(fnv(&s.to_lowercase())).join("installed.json")).filter(|p| *p != new).collect();
    // Also any registry that says (0.9.2+) it belongs to this folder.
    let canon = canonical_root(root);
    if let Ok(rd) = std::fs::read_dir(base) {
        for e in rd.flatten() {
            let p = e.path().join("installed.json");
            if p == new || out.contains(&p) || !p.is_file() { continue; }
            if let Ok(r) = serde_json::from_slice::<Registry>(&std::fs::read(&p).unwrap_or_default()) { if !r.root.is_empty() && canonical_root(Path::new(&r.root)) == canon { out.push(p); } }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The registry of `root` under `base`, adopting (merging, once) any registry an older spelling of
/// the same folder left behind; the old file is renamed `installed.json.migrated`, not deleted.
fn load_in(base: &Path, root: &Path) -> Result<Registry> {
    let p = registry_path_in(base, root);
    let mut reg: Registry = if p.is_file() { serde_json::from_slice(&std::fs::read(&p)?)? } else { Registry::default() };
    let mut migrated = false;
    for old in legacy_paths(base, root).into_iter().filter(|o| o.is_file()) {
        let r: Registry = serde_json::from_slice(&std::fs::read(&old)?).with_context(|| format!("read {}", old.display()))?;
        for (name, files) in r.mods {
            let e = reg.mods.entry(name).or_default();
            for f in files { if !e.contains(&f) { e.push(f); } }
        }
        for d in r.dirs { if !reg.dirs.contains(&d) { reg.dirs.push(d); } }
        std::fs::rename(&old, old.with_extension("json.migrated")).with_context(|| format!("retire {}", old.display()))?;
        migrated = true;
    }
    reg.root = canonical_root(root);
    if migrated { save_in(base, root, &reg)?; }
    Ok(reg)
}

fn save_in(base: &Path, root: &Path, r: &Registry) -> Result<()> {
    let p = registry_path_in(base, root);
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(&p, serde_json::to_vec_pretty(r)?)?;
    Ok(())
}

fn load(g: &GameCtx) -> Result<Registry> { load_in(&registry_base(), &g.root) }

fn save(g: &GameCtx, r: &Registry) -> Result<()> { save_in(&registry_base(), &g.root, r) }

fn ensure_loose_files_read(root: &Path) -> Result<Option<String>> {
    let ini = root.join("bin").join("mountlist_packed.ini");
    let t = std::fs::read_to_string(&ini).with_context(|| format!("read {}", ini.display()))?;
    if !t.lines().any(|l| l.trim() == "NoPhysical=true") { return Ok(None); }
    let stock = ini.with_extension("ini.risenimpexp-stock");
    if !stock.exists() { std::fs::copy(&ini, &stock)?; }
    std::fs::write(&ini, t.replace("NoPhysical=true", "NoPhysical=false"))?;
    Ok(Some("NoPhysical=false set in bin\\mountlist_packed.ini (stock copy kept)".into()))
}

/// Rebuild the directories from the archive plus every registered mod's files (as they now are
/// in the game folder); remove them when no mod is left.
fn sync_directories(g: &GameCtx, reg: &mut Registry) -> Result<()> {
    let mut all = vec![];
    for files in reg.mods.values() { for rel in files { all.push((rel.clone(), std::fs::read(g.root.join(rel)).with_context(|| format!("read installed {rel}"))?)); } }
    let bases = reg.bases.clone();
    let base = |loose: &str| bases.get(loose).and_then(|copy| std::fs::read(copy).ok());
    let dirs = export::directories_on(g, &all, &base, false, |rel| file_mtime(&g.root.join(rel)))?;
    for old in &reg.dirs {
        if dirs.iter().any(|(p, _)| p == old) { continue; }
        // Ours no longer: an adopted directory goes back as it was, any other is removed.
        match reg.bases.get(old) {
            Some(copy) => { std::fs::copy(copy, g.root.join(old)).with_context(|| format!("restore {old}"))?; }
            None => { let _ = std::fs::remove_file(g.root.join(old)); }
        }
    }
    reg.bases.retain(|loose, _| dirs.iter().any(|(p, _)| p == loose));
    export::write_all(&g.root, &dirs)?;
    reg.dirs = dirs.into_iter().map(|(p, _)| p).collect();
    Ok(())
}

/// A directory another mod left in the game becomes the base ours are built on (see `Registry::bases`).
fn adopt(g: &GameCtx, reg: &mut Registry, loose: &str) -> Result<String> {
    let dir = registry_path_in(&registry_base(), &g.root).parent().unwrap().join("bases");
    std::fs::create_dir_all(&dir)?;
    let copy = dir.join(loose.replace(['/', '\\'], "__"));
    std::fs::copy(g.root.join(loose), &copy).with_context(|| format!("keep a copy of {loose}"))?;
    reg.bases.insert(loose.to_string(), copy.to_string_lossy().into_owned());
    reg.dirs.push(loose.to_string());
    Ok(format!("{loose} came from another mod: kept, and ours are added to it (its copy goes back when ours are removed)"))
}

pub fn install(g: &GameCtx, name: &str, files: &[(String, Vec<u8>)]) -> Result<Vec<String>> {
    if is_running() { bail!("close Risen first"); }
    let mut reg = load(g)?;
    let mut notes = vec![];
    let previous = reg.mods.remove(name).unwrap_or_default();
    for (_, _, loose) in crate::cache::DIRECTORIES {
        if g.root.join(loose).exists() && !reg.dirs.iter().any(|d| d == loose) {
            let n = adopt(g, &mut reg, loose)?;
            notes.push(n);
        }
    }
    for (rel, bytes) in files {
        let p = g.root.join(rel);
        if reg.mods.values().any(|v| v.contains(rel)) { bail!("{rel} belongs to another Risen ImpExp mod; uninstall it first"); }
        if p.exists() && !previous.contains(rel) && std::fs::read(&p)? != *bytes { bail!("{rel} already exists and belongs to another mod; nothing changed"); }
    }
    if let Some(n) = ensure_loose_files_read(&g.root)? { notes.push(n); }
    for rel in previous.iter().filter(|r| !files.iter().any(|(p, _)| p == *r)) { let _ = std::fs::remove_file(g.root.join(rel)); }
    export::write_all(&g.root, files)?;
    reg.mods.insert(name.to_string(), files.iter().map(|(p, _)| p.clone()).collect());
    sync_directories(g, &mut reg)?;
    save(g, &reg)?;
    notes.extend(files.iter().map(|(p, _)| format!("installed {p}")));
    Ok(notes)
}

pub fn uninstall(g: &GameCtx, name: &str) -> Result<Vec<String>> {
    if is_running() { bail!("close Risen first"); }
    let mut reg = load(g)?;
    let Some(files) = reg.mods.remove(name) else { bail!("{name} is not installed by Risen ImpExp") };
    let mut notes = vec![];
    for rel in &files { if std::fs::remove_file(g.root.join(rel)).is_ok() { notes.push(format!("removed {rel}")); } }
    sync_directories(g, &mut reg)?;
    save(g, &reg)?;
    Ok(notes)
}

pub fn installed(g: &GameCtx) -> Result<Vec<(String, Vec<String>)>> { Ok(load(g)?.mods.into_iter().collect()) }

fn is_running() -> bool {
    std::process::Command::new("tasklist").args(["/FI", "IMAGENAME eq Risen.exe", "/NH"]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase().contains("risen.exe")).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    /// Every spelling of one game folder finds one registry, and a registry left under an old
    /// (as-spelled) key is adopted once and retired. Temp folders only: never the real game.
    #[test]
    fn one_registry_per_game_folder_whatever_the_spelling() {
        use std::path::Path;
        let tmp = std::env::temp_dir().join(format!("rc-reg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (base, game) = (tmp.join("appdata"), tmp.join("Program Files (x86)").join("Risen"));
        std::fs::create_dir_all(game.join("bin")).unwrap();
        let s = game.to_string_lossy().into_owned();
        let spellings = [s.clone(), s.replace('\\', "/"), format!("{s}\\"), s.to_uppercase(), game.join("bin").join("..").to_string_lossy().into_owned()];
        let want = super::registry_path_in(&base, &game);
        for sp in &spellings { assert_eq!(super::registry_path_in(&base, Path::new(sp)), want, "spelling {sp}"); }

        // A 0.9.1 registry under the forward-slash spelling's old key, another under the as-is key.
        let old = |sp: &str| base.join(super::fnv(&sp.to_lowercase())).join("installed.json");
        let (o1, o2) = (old(&s.replace('\\', "/")), old(&format!("{s}\\")));
        for (p, body) in [(&o1, r#"{"mods":{"Landscape":["data/a"]},"dirs":["data/d1"]}"#), (&o2, r#"{"mods":{"Landscape":["data/b"],"Sword":["data/c"]},"dirs":["data/d1"]}"#)] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let r = super::load_in(&base, &game).unwrap();
        // Old registries merge in path order, and the paths hash a temp folder named after the process.
        let mut landscape = r.mods["Landscape"].clone();
        landscape.sort();
        assert_eq!(landscape, vec!["data/a".to_string(), "data/b".to_string()]);
        assert_eq!(r.mods["Sword"], vec!["data/c".to_string()]);
        assert_eq!(r.dirs, vec!["data/d1".to_string()]);
        assert!(want.is_file() && !o1.is_file() && !o2.is_file(), "adopted registries must be saved under the new key and retired");
        // Loading again through another spelling changes nothing.
        let again = super::load_in(&base, Path::new(&s.replace('\\', "/"))).unwrap();
        assert_eq!(again.mods, r.mods);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A clip name with `%` survives the installer: the batch file doubles it, cmd reads it back once.
    #[cfg(windows)]
    #[test]
    fn installer_copies_names_with_percent() {
        let root = std::env::temp_dir().join(format!("rc-pct-{}", std::process::id()));
        let (pkg, game) = (root.join("pkg"), root.join("Program Files (x86)").join("Risen"));
        std::fs::create_dir_all(game.join("bin")).unwrap();
        std::fs::write(game.join("bin").join("Risen.exe"), b"stub").unwrap();
        std::fs::write(game.join("bin").join("mountlist_packed.ini"), b"NoPhysical=false\r\n").unwrap();
        let rel = "data/compiled/animations/Hero_Stand_None_None_P0_Ambient_Loop_N_Fwd_00_%_00_P0_0._xmot";
        std::fs::create_dir_all(pkg.join("files").join("data/compiled/animations")).unwrap();
        std::fs::write(pkg.join("files").join(rel), b"clip").unwrap();
        let list = format!("\"{}\"", super::bat(rel));
        std::fs::write(pkg.join("INSTALL.bat"), super::INSTALL.replace("{TITLE}", "t").replace("{FILES}", &list).replace('\n', "\r\n")).unwrap();
        let out = std::process::Command::new("cmd").arg("/c").arg(pkg.join("INSTALL.bat")).arg(&game).stdin(std::process::Stdio::null()).output().unwrap();
        let installed = game.join(rel).is_file();
        let _ = std::fs::remove_dir_all(&root);
        assert!(installed, "not installed:\n{}", String::from_utf8_lossy(&out.stdout));
    }
}
