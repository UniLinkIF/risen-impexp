//! The engine's resource directories: `compiled_meshes.bin`, `compiled_materials.bin`,
//! `compiled_images.bin`. The game takes a mesh's materials (and a material's images) from these
//! records, not from the resource file itself — a new `._xmsh` without its record keeps the old look.
//!
//! ```text
//! "GENOMFLE" u16 1 · u32 pool offset · "GH04" · u32 record count · records · pool
//! record   4-char kind (MS02 / MA02 / IM04) · the resource file's property section with every string
//!          replaced by a u16 pool index · u16 resource name · u32 data size (file@0x14)
//!          · FILETIME (file@0x18) · FILETIME (the loose file's mtime)
//! pool     u32 0xDEADBEEF · u8 1 · u32 count · count × (u16 length, Latin-1)
//! ```
//! Every shipped record is re-created identically from its file (see the tests).

use crate::gr01::{self, Reader, Resource, Section, Strings};
use anyhow::{bail, Result};
use std::collections::HashMap;

const TRAILER: usize = 22;

pub struct Pool { pub names: Vec<String>, index: HashMap<String, u16> }

impl Pool {
    fn new(names: Vec<String>) -> Pool {
        let index = names.iter().enumerate().map(|(i, s)| (s.clone(), i as u16)).collect();
        Pool { names, index }
    }
    /// Existing strings keep their index; new ones are appended, so untouched records stay valid.
    pub fn idx(&mut self, s: &str) -> u16 {
        if let Some(&i) = self.index.get(s) { return i; }
        let i = self.names.len() as u16;
        self.names.push(s.to_string());
        self.index.insert(s.to_string(), i);
        i
    }
}

impl Strings for Pool {
    fn read(&mut self, r: &mut Reader) -> Result<String> {
        let at = r.at;
        let i = r.u16()? as usize;
        match self.names.get(i) { Some(s) => Ok(s.clone()), None => bail!("pool index {i} out of {}: offset 0x{at:x}", self.names.len()) }
    }
    fn write(&mut self, s: &str, out: &mut Vec<u8>) { out.extend(self.idx(s).to_le_bytes()); }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Record { pub kind: [u8; 4], pub section: Section, pub name: String, pub data_size: u32, pub filetime: [u8; 8], pub mtime: u64 }

pub struct Cache { head: Vec<u8>, pub kind: [u8; 4], pub records: Vec<Record>, pub pool: Pool }

impl Cache {
    pub fn parse(b: &[u8]) -> Result<Cache> {
        if b.len() < 0x16 || &b[..8] != b"GENOMFLE" { bail!("not a GENOMFLE resource directory"); }
        let mut r = Reader { d: b, at: 10 };
        let pool_off = r.u32()? as usize;
        r.at = 18;
        let count = r.u32()? as usize;
        let mut p = Reader { d: b, at: pool_off };
        if p.u32()? != 0xdeadbeef || p.u8()? != 1 { bail!("string pool marker missing: offset 0x{pool_off:x}"); }
        let n = p.u32()?;
        let mut names = Vec::with_capacity(n as usize);
        for _ in 0..n { let l = p.u16()? as usize; names.push(gr01::latin1(p.bytes(l)?)); }
        if p.at != b.len() { bail!("bytes after the string pool: offset 0x{:x}", p.at); }
        let mut pool = Pool::new(names);
        let kind: [u8; 4] = b[0x16..0x1a].try_into().unwrap();
        // Records carry no length; each one ends where the next one's kind starts.
        let mut starts = vec![];
        let mut at = 0x16;
        while at + 4 <= pool_off { if b[at..at + 4] == kind { starts.push(at); at += 4 } else { at += 1 } }
        if starts.len() != count { bail!("found {} records, header says {count}", starts.len()); }
        let mut records = Vec::with_capacity(count);
        for (k, &s) in starts.iter().enumerate() {
            let end = starts.get(k + 1).copied().unwrap_or(pool_off);
            let mut r = Reader { d: b, at: s + 4 };
            let section = gr01::read_section(&mut r, &mut pool, end - TRAILER)?;
            let name = pool.read(&mut r)?;
            let data_size = r.u32()?;
            let filetime = r.bytes(8)?.try_into().unwrap();
            let mtime = u64::from_le_bytes(r.bytes(8)?.try_into().unwrap());
            records.push(Record { kind, section, name, data_size, filetime, mtime });
        }
        Ok(Cache { head: b[..0x16].to_vec(), kind, records, pool })
    }

    pub fn find(&self, name: &str) -> Option<usize> { self.records.iter().position(|r| r.name.eq_ignore_ascii_case(name)) }

    /// The record the engine would hold for `file` under `name`, stamped with `mtime` (FILETIME).
    pub fn record_for(&self, file: &[u8], name: &str, mtime: u64) -> Result<Record> {
        let res = Resource::parse(file)?;
        if res.magic[4..] != self.kind { bail!("{name}: a {} file does not belong in a {} directory", gr01::latin1(&res.magic[4..]), gr01::latin1(&self.kind)); }
        Ok(Record { kind: self.kind, section: res.section, name: name.to_string(), data_size: (res.data.len()) as u32, filetime: res.filetime, mtime })
    }

    /// Replace the record with the same name, or append a new one.
    pub fn put(&mut self, rec: Record) -> bool {
        match self.find(&rec.name) { Some(i) => { self.records[i] = rec; true } None => { self.records.push(rec); false } }
    }

    pub fn write(&mut self) -> Vec<u8> {
        let mut recs = vec![];
        for r in &self.records {
            recs.extend(r.kind);
            recs.extend(gr01::write_section(&r.section, &mut self.pool));
            recs.extend(self.pool.idx(&r.name).to_le_bytes());
            recs.extend(r.data_size.to_le_bytes());
            recs.extend(r.filetime);
            recs.extend(r.mtime.to_le_bytes());
        }
        let mut o = self.head.clone();
        o[10..14].copy_from_slice(&((0x16 + recs.len()) as u32).to_le_bytes());
        o[18..22].copy_from_slice(&(self.records.len() as u32).to_le_bytes());
        o.extend(recs);
        o.extend(0xdeadbeefu32.to_le_bytes());
        o.push(1);
        o.extend((self.pool.names.len() as u32).to_le_bytes());
        for s in &self.pool.names { let b = gr01::to_latin1(s); o.extend((b.len() as u16).to_le_bytes()); o.extend(b); }
        o
    }
}

/// Where each directory lives, as an archive entry and as the loose path the game reads over it.
pub const DIRECTORIES: [(&str, &str, &str); 6] = [
    ("._xmac", "/compiled_motion_actors.bin", "data/compiled/animations/compiled_motion_actors.bin"),
    ("._xmot", "/compiled_motions.bin", "data/compiled/animations/compiled_motions.bin"),
    ("._xcom", "/compiled_collision_meshes.bin", "data/common/physics/compiled_collision_meshes.bin"),
    ("._xmsh", "/compiled_meshes.bin", "data/common/meshes/compiled_meshes.bin"),
    ("._xmat", "/compiled_materials.bin", "data/compiled/materials/compiled_materials.bin"),
    ("._ximg", "/compiled_images.bin", "data/compiled/images/compiled_images.bin"),
];

/// Unix time → Windows FILETIME (100 ns since 1601).
pub fn filetime(t: std::time::SystemTime) -> u64 {
    let d = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    d.as_secs() * 10_000_000 + d.subsec_nanos() as u64 / 100 + 116_444_736_000_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped directories round-trip, and a record rebuilt from each resource file equals the
    /// shipped record — so records we build for new files have exactly the engine's shape.
    #[test]
    fn rebuilds_every_shipped_record_from_its_file() {
        let Some(g) = crate::game::test_game() else { eprintln!("skip: RISEN_GAME not set"); return };
        for (suffix, entry, _) in DIRECTORIES {
            let bytes = g.read_archive(&g.find_one(entry).unwrap()).unwrap();
            let mut c = Cache::parse(&bytes).unwrap();
            assert_eq!(c.write(), bytes, "{entry} does not round-trip");
            let files: HashMap<String, String> = g.entries_with_suffix(suffix).into_iter()
                .map(|e| (e.rsplit('/').next().unwrap().split('.').next().unwrap().to_lowercase(), e)).collect();
            let (mut same, mut missing, mut diff) = (0, 0, vec![]);
            for r in &c.records {
                let Some(e) = files.get(&r.name.to_lowercase()) else { missing += 1; continue };
                let rebuilt = c.record_for(&g.read_archive(e).unwrap(), &r.name, r.mtime).unwrap();
                if &rebuilt == r { same += 1 } else { diff.push(r.name.clone()) }
            }
            eprintln!("{entry}: {} records, {same} rebuilt identically, {missing} without a file, {} differ {:?}", c.records.len(), diff.len(), &diff[..diff.len().min(5)]);
            // 35 actor records predate their files (a later patch rebuilt the files, not the records).
            if suffix != "._xmac" { assert!(diff.is_empty()); }
        }
    }
}

#[cfg(test)]
mod where_tests {
    #[test]
    fn directory_loose_paths_mirror_their_archives() {
        let Some(g) = crate::game::test_game() else { return };
        for (_, entry, loose) in super::DIRECTORIES {
            let p = g.physical_path(&g.find_one(entry).unwrap()).unwrap();
            let rel = p.strip_prefix(&g.root).unwrap().to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
            assert_eq!(rel.to_lowercase(), loose.to_lowercase());
        }
    }
}

