//! Getting built files into the game, two ways.
//!
//! * A **mod package** to hand to other players: `files/` mirrors the game folder (resources plus
//!   the three directories = the archive's + our records), with `INSTALL.bat` / `ROLLBACK.bat`.
//!   It needs nothing but Windows. INSTALL refuses to overwrite a file it did not put there, so
//!   two packages that both carry directories cannot silently undo each other.
//! * **Install into this game** from Blender: every mod installed this way is listed in
//!   `%LOCALAPPDATA%\RisenImpExp\installed.json`, and the directories are rebuilt from the
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
    mods: BTreeMap<String, Vec<String>>, dirs: Vec<String> }

fn registry_path(g: &GameCtx) -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let tag = format!("{:08x}", g.root.to_string_lossy().to_lowercase().bytes().fold(0x811c9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x01000193)));
    base.join("RisenImpExp").join(tag).join("installed.json")
}

fn load(g: &GameCtx) -> Result<Registry> {
    let p = registry_path(g);
    if !p.is_file() { return Ok(Registry::default()); }
    Ok(serde_json::from_slice(&std::fs::read(&p)?)?)
}

fn save(g: &GameCtx, r: &Registry) -> Result<()> {
    let p = registry_path(g);
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(&p, serde_json::to_vec_pretty(r)?)?;
    Ok(())
}

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
    let dirs = export::directories(g, &all, false, |rel| file_mtime(&g.root.join(rel)))?;
    for old in &reg.dirs { if !dirs.iter().any(|(p, _)| p == old) { let _ = std::fs::remove_file(g.root.join(old)); } }
    export::write_all(&g.root, &dirs)?;
    reg.dirs = dirs.into_iter().map(|(p, _)| p).collect();
    Ok(())
}

pub fn install(g: &GameCtx, name: &str, files: &[(String, Vec<u8>)]) -> Result<Vec<String>> {
    if is_running() { bail!("close Risen first"); }
    let mut reg = load(g)?;
    let mut notes = vec![];
    let previous = reg.mods.remove(name).unwrap_or_default();
    for (_, dir, loose) in crate::cache::DIRECTORIES {
        if g.root.join(loose).exists() && !reg.dirs.iter().any(|d| d == loose) {
            bail!("{loose} already exists and was not made by Risen ImpExp (another mod owns the {dir} directory); remove that mod first");
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
