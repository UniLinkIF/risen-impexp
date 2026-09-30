//! Risen .pak archives (and their .p0x update volumes): the directory tree and file data (zlib).

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;

const DIR_ATTR: u32 = 0x0000_0010;
const DELETED_ATTR: u32 = 0x0000_8000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCompression {
    None,
    Auto,
    ZLib,
}

impl FileCompression {
    fn from_u32(v: u32) -> Self {
        match v {
            1 => FileCompression::Auto,
            2 => FileCompression::ZLib,
            _ => FileCompression::None,
        }
    }
    fn to_u32(self) -> u32 {
        match self {
            FileCompression::None => 0,
            FileCompression::Auto => 1,
            FileCompression::ZLib => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PakHeader {
    pub version: u32,
    pub product: u32,
    pub revision: u32,
    pub encryption: u32,
    pub compression: u32,
    pub data_offset: u64,
    pub root_offset: u64,
    pub volume_size: u64,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub data_offset: u64,
    pub file_attributes: u32,
    pub compression: FileCompression,
    pub data_size: u32,
    pub file_size: u32,
}

impl FileEntry {
    pub fn is_deleted(&self) -> bool {
        self.file_attributes & DELETED_ATTR != 0
    }
}

#[derive(Debug)]
enum Node {
    File(FileEntry),
    Dir {
        #[allow(dead_code)]
        name: String,
        entries: Vec<Node>,
    },
}

pub struct PakArchive {
    pub header: PakHeader,
    file: BufReader<File>,
    root: Node,
}

fn read_u32<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64<R: Read>(r: &mut R) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn read_name<R: Read>(r: &mut R) -> io::Result<String> {
    let len = read_u32(r)? as usize;
    if len == 0 {
        return Ok(String::new());
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let mut term = [0u8; 1];
    r.read_exact(&mut term)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn read_file_entry<R: Read>(r: &mut R, path_prefix: &str) -> io::Result<FileEntry> {
    let name = read_name(r)?;
    let data_offset = read_u64(r)?;
    let _t_created = read_u64(r)?;
    let _t_accessed = read_u64(r)?;
    let _t_modified = read_u64(r)?;
    let file_attributes = read_u32(r)?;
    let _encryption = read_u32(r)?;
    let compression = FileCompression::from_u32(read_u32(r)?);
    let data_size = read_u32(r)?;
    let file_size = read_u32(r)?;
    Ok(FileEntry {
        path: format!("{path_prefix}/{name}"),
        data_offset,
        file_attributes,
        compression,
        data_size,
        file_size,
    })
}

fn read_directory<R: Read>(r: &mut R, path_prefix: &str) -> io::Result<Node> {
    let name = read_name(r)?;
    let my_path = if name.is_empty() {
        path_prefix.to_string()
    } else {
        format!("{path_prefix}/{name}")
    };
    let _t_created = read_u64(r)?;
    let _t_accessed = read_u64(r)?;
    let _t_modified = read_u64(r)?;
    let _file_attributes = read_u32(r)?;
    let count = read_u32(r)?;
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let attributes = read_u32(r)?;
        if attributes & DIR_ATTR != 0 {
            entries.push(read_directory(r, &my_path)?);
        } else {
            entries.push(Node::File(read_file_entry(r, &my_path)?));
        }
    }
    Ok(Node::Dir {
        name: my_path,
        entries,
    })
}

fn flatten_node(node: &Node, out: &mut Vec<FileEntry>) {
    match node {
        Node::File(f) => out.push(f.clone()),
        Node::Dir { entries, .. } => {
            for e in entries {
                flatten_node(e, out);
            }
        }
    }
}

impl PakArchive {
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let mut file = BufReader::new(File::open(path)?);

        let version = read_u32(&mut file)?;
        let product = read_u32(&mut file)?;
        let revision = read_u32(&mut file)?;
        let encryption = read_u32(&mut file)?;
        let compression = read_u32(&mut file)?;
        let _reserved = read_u32(&mut file)?;
        let data_offset = read_u64(&mut file)?;
        let root_offset = read_u64(&mut file)?;
        let volume_size = read_u64(&mut file)?;

        let header = PakHeader {
            version,
            product,
            revision,
            encryption,
            compression,
            data_offset,
            root_offset,
            volume_size,
        };

        file.seek(SeekFrom::Start(header.root_offset))?;
        let root = read_directory(&mut file, "")?;

        Ok(PakArchive {
            header,
            file,
            root,
        })
    }

    pub fn is_valid_g3v0(&self) -> bool {
        self.header.product == 0x3056_3347
    }

    pub fn files(&self) -> Vec<FileEntry> {
        let mut out = Vec::new();
        flatten_node(&self.root, &mut out);
        out
    }

    pub fn read_file_raw(&mut self, entry: &FileEntry) -> io::Result<Vec<u8>> {
        self.file.seek(SeekFrom::Start(entry.data_offset))?;
        let mut raw = vec![0u8; entry.data_size as usize];
        self.file.read_exact(&mut raw)?;
        Ok(raw)
    }

    pub fn read_file(&mut self, entry: &FileEntry) -> io::Result<Vec<u8>> {
        let raw = self.read_file_raw(entry)?;
        match entry.compression {
            FileCompression::ZLib => {
                let mut decoder = ZlibDecoder::new(&raw[..]);
                let mut out = Vec::with_capacity(entry.file_size as usize);
                decoder.read_to_end(&mut out)?;
                Ok(out)
            }
            _ => Ok(raw),
        }
    }

    pub fn extract_all<P: AsRef<Path>>(&mut self, out_dir: P) -> io::Result<usize> {
        let out_dir = out_dir.as_ref();
        let entries = self.files();
        let mut count = 0;
        for entry in &entries {
            if entry.is_deleted() {
                continue;
            }
            let data = self.read_file(entry)?;
            let rel = entry.path.trim_start_matches('/');
            let dest = out_dir.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, &data)?;
            count += 1;
        }
        Ok(count)
    }
}

pub fn write_archive_from_dir<P: AsRef<Path>>(src_dir: P, out_path: P) -> io::Result<()> {
    write_archive_from_dir_with(src_dir.as_ref(), out_path.as_ref(), FileCompression::None)
}

pub fn write_archive_from_dir_with(src_dir: &Path, out_path: &Path, compression: FileCompression) -> io::Result<()> {
    let mut file_list = Vec::new();
    collect_files(src_dir, src_dir, &mut file_list)?;

    let mut out = File::create(out_path)?;

    const HEADER_SIZE: u64 = 48;
    out.write_all(&1u32.to_le_bytes())?;
    out.write_all(&0x3056_3347u32.to_le_bytes())?;
    out.write_all(&0u32.to_le_bytes())?;
    out.write_all(&0u32.to_le_bytes())?;
    out.write_all(&compression.to_u32().to_le_bytes())?;
    out.write_all(&0u32.to_le_bytes())?;
    out.write_all(&HEADER_SIZE.to_le_bytes())?;
    let root_offset_pos = out.stream_position()?;
    out.write_all(&0u64.to_le_bytes())?;
    let volume_size_pos = out.stream_position()?;
    out.write_all(&0u64.to_le_bytes())?;

    let mut written = Vec::new();
    for rel_path in &file_list {
        let full = src_dir.join(rel_path);
        let data = std::fs::read(&full)?;
        let stored = match compression {
            FileCompression::ZLib => zlib_compress(&data)?,
            _ => data.clone(),
        };
        let offset = out.stream_position()?;
        out.write_all(&stored)?;
        written.push((rel_path.clone(), offset, stored.len() as u32, data.len() as u32, compression));
    }

    let root_offset = out.stream_position()?;
    let tree = build_write_tree(&written);
    write_directory_tree(&mut out, &tree)?;
    let volume_size = out.stream_position()?;

    out.seek(SeekFrom::Start(root_offset_pos))?;
    out.write_all(&root_offset.to_le_bytes())?;
    out.seek(SeekFrom::Start(volume_size_pos))?;
    out.write_all(&volume_size.to_le_bytes())?;

    Ok(())
}

pub fn merge_into_full_pak(base: &mut PakArchive, patch: &mut PakArchive, out_path: &Path) -> io::Result<usize> {
    let base_entries = base.files();
    let patch_entries = patch.files();
    let patch_by_path: std::collections::HashMap<&str, &FileEntry> =
        patch_entries.iter().map(|e| (e.path.as_str(), e)).collect();

    let mut out = File::create(out_path)?;
    const HEADER_SIZE: u64 = 48;
    out.write_all(&base.header.version.to_le_bytes())?;
    out.write_all(&base.header.product.to_le_bytes())?;
    out.write_all(&base.header.revision.to_le_bytes())?;
    out.write_all(&base.header.encryption.to_le_bytes())?;
    out.write_all(&base.header.compression.to_le_bytes())?;
    out.write_all(&0u32.to_le_bytes())?;
    out.write_all(&HEADER_SIZE.to_le_bytes())?;
    let root_offset_pos = out.stream_position()?;
    out.write_all(&0u64.to_le_bytes())?;
    let volume_size_pos = out.stream_position()?;
    out.write_all(&0u64.to_le_bytes())?;

    let mut written = Vec::new();
    for entry in &base_entries {
        if let Some(p) = patch_by_path.get(entry.path.as_str()) {
            if p.is_deleted() {
                continue;
            }
            let raw = patch.read_file_raw(p)?;
            let offset = out.stream_position()?;
            out.write_all(&raw)?;
            written.push((entry.path.trim_start_matches('/').to_string(), offset, p.data_size, p.file_size, p.compression));
        } else {
            if entry.is_deleted() {
                continue;
            }
            let raw = base.read_file_raw(entry)?;
            let offset = out.stream_position()?;
            out.write_all(&raw)?;
            written.push((entry.path.trim_start_matches('/').to_string(), offset, entry.data_size, entry.file_size, entry.compression));
        }
    }
    for entry in &patch_entries {
        if entry.is_deleted() || base_entries.iter().any(|b| b.path == entry.path) {
            continue;
        }
        let raw = patch.read_file_raw(entry)?;
        let offset = out.stream_position()?;
        out.write_all(&raw)?;
        written.push((entry.path.trim_start_matches('/').to_string(), offset, entry.data_size, entry.file_size, entry.compression));
    }

    let count = written.len();
    let root_offset = out.stream_position()?;
    let tree = build_write_tree(&written);
    write_directory_tree(&mut out, &tree)?;
    let volume_size = out.stream_position()?;

    out.seek(SeekFrom::Start(root_offset_pos))?;
    out.write_all(&root_offset.to_le_bytes())?;
    out.seek(SeekFrom::Start(volume_size_pos))?;
    out.write_all(&volume_size.to_le_bytes())?;

    Ok(count)
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            out.push(rel);
        }
    }
    Ok(())
}

fn write_name<W: Write>(w: &mut W, name: &str) -> io::Result<()> {
    let bytes = name.as_bytes();
    w.write_all(&(bytes.len() as u32).to_le_bytes())?;
    if !bytes.is_empty() {
        w.write_all(bytes)?;
        w.write_all(&[0u8])?;
    }
    Ok(())
}

enum WriteNode {
    File { name: String, offset: u64, stored_size: u32, original_size: u32, compression: FileCompression },
    Dir { name: String, children: Vec<WriteNode> },
}

fn build_write_tree(files: &[(String, u64, u32, u32, FileCompression)]) -> Vec<WriteNode> {
    let mut root: Vec<WriteNode> = Vec::new();
    for (rel_path, offset, stored_size, original_size, compression) in files {
        let parts: Vec<&str> = rel_path.split('/').collect();
        insert_write_node(&mut root, &parts, *offset, *stored_size, *original_size, *compression);
    }
    root
}

fn insert_write_node(nodes: &mut Vec<WriteNode>, parts: &[&str], offset: u64, stored_size: u32, original_size: u32, compression: FileCompression) {
    match parts {
        [] => {}
        [name] => nodes.push(WriteNode::File {
            name: (*name).to_string(),
            offset,
            stored_size,
            original_size,
            compression,
        }),
        [dir_name, rest @ ..] => {
            let idx = nodes
                .iter()
                .position(|n| matches!(n, WriteNode::Dir { name, .. } if name == dir_name))
                .unwrap_or_else(|| {
                    nodes.push(WriteNode::Dir {
                        name: (*dir_name).to_string(),
                        children: Vec::new(),
                    });
                    nodes.len() - 1
                });
            if let WriteNode::Dir { children, .. } = &mut nodes[idx] {
                insert_write_node(children, rest, offset, stored_size, original_size, compression);
            }
        }
    }
}

fn write_directory_tree<W: Write>(w: &mut W, nodes: &[WriteNode]) -> io::Result<()> {
    write_name(w, "")?;
    w.write_all(&0u64.to_le_bytes())?;
    w.write_all(&0u64.to_le_bytes())?;
    w.write_all(&0u64.to_le_bytes())?;
    w.write_all(&DIR_ATTR.to_le_bytes())?;
    w.write_all(&(nodes.len() as u32).to_le_bytes())?;
    for node in nodes {
        write_node(w, node)?;
    }
    Ok(())
}

fn write_node<W: Write>(w: &mut W, node: &WriteNode) -> io::Result<()> {
    match node {
        WriteNode::File { name, offset, stored_size, original_size, compression } => {
            let file_attr = 0x0000_0020u32;
            w.write_all(&file_attr.to_le_bytes())?;
            write_name(w, name)?;
            w.write_all(&offset.to_le_bytes())?;
            w.write_all(&0u64.to_le_bytes())?;
            w.write_all(&0u64.to_le_bytes())?;
            w.write_all(&0u64.to_le_bytes())?;
            w.write_all(&file_attr.to_le_bytes())?;
            w.write_all(&0u32.to_le_bytes())?;
            w.write_all(&compression.to_u32().to_le_bytes())?;
            w.write_all(&stored_size.to_le_bytes())?;
            w.write_all(&original_size.to_le_bytes())?;
        }
        WriteNode::Dir { name, children } => {
            w.write_all(&DIR_ATTR.to_le_bytes())?;
            write_name(w, name)?;
            w.write_all(&0u64.to_le_bytes())?;
            w.write_all(&0u64.to_le_bytes())?;
            w.write_all(&0u64.to_le_bytes())?;
            w.write_all(&DIR_ATTR.to_le_bytes())?;
            w.write_all(&(children.len() as u32).to_le_bytes())?;
            for child in children {
                write_node(w, child)?;
            }
        }
    }
    Ok(())
}

fn zlib_compress(data: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

