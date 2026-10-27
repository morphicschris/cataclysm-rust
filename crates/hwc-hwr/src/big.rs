//! Reader for Homeworld 2 / Remastered `.big` archives (`_ARCHIVE`, version 2).
//!
//! Layout (all little-endian):
//!
//! * Archive header, 180 bytes: the 8 bytes `_ARCHIVE`, `u32` version (2), a 16-byte tool
//!   signature, the archive name (64 UTF-16 units), a 16-byte archive signature, then `u32`
//!   section-header size and `u32` file-data offset (absolute).
//! * Section header at 180: four `(u32 offset, u16 count)` pairs locating the table of
//!   contents, folders, files and the name table, offsets relative to 180.
//! * Table-of-contents entries (138 bytes): alias and name (64 bytes each), then `u16` first and
//!   last folder, first and last file, start folder.
//! * Folders (12 bytes): `u32` name offset, `u16` first and last subfolder, first and last file.
//!   Folder names are full paths with backslashes.
//! * Files (17 bytes): `u32` name offset, `u8` compression (0 stored, otherwise zlib), `u32` data
//!   offset (relative to the file-data offset), `u32` stored and `u32` real length.
//!
//! Paths come out normalised by [`normalize`]: lowercase, `/` separators.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const MAGIC: &[u8; 8] = b"_ARCHIVE";
const SECTION_HEADER: u64 = 180;

/// One stored file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Normalised path (`art/fx/textures/foo.tga`).
    pub path: String,
    pub compressed: bool,
    /// Absolute offset of the stored bytes.
    pub offset: u64,
    pub stored_len: u32,
    pub real_len: u32,
}

/// An opened archive. Reads go through the file each time.
#[derive(Debug)]
pub struct Archive {
    file: File,
    entries: Vec<Entry>,
    by_path: HashMap<String, usize>,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// A path as the archive index keys it: lowercase, `/` separators, no `data:` prefix or
/// leading slash.
pub fn normalize(path: &str) -> String {
    let mut p = path.trim().to_ascii_lowercase().replace('\\', "/");
    if let Some(rest) = p.strip_prefix("data:") {
        p = rest.to_string();
    }
    while let Some(rest) = p.strip_prefix('/') {
        p = rest.to_string();
    }
    while p.contains("//") {
        p = p.replace("//", "/");
    }
    p
}

fn u16_at(b: &[u8], o: usize) -> io::Result<u16> {
    b.get(o..o + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| bad("section header truncated"))
}

fn u32_at(b: &[u8], o: usize) -> io::Result<u32> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| bad("section header truncated"))
}

fn cstr(b: &[u8], o: usize) -> io::Result<String> {
    let s = b.get(o..).ok_or_else(|| bad("name offset out of range"))?;
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    Ok(s[..end].iter().map(|&c| c as char).collect())
}

/// Parses the index from the bytes of the archive header and section header
/// (`header` holds at least the first 180 bytes, `section` the section header).
pub fn parse_index(header: &[u8], section: &[u8]) -> io::Result<Vec<Entry>> {
    if header.len() < SECTION_HEADER as usize || &header[..8] != MAGIC {
        return Err(bad("not a Homeworld 2 archive"));
    }
    let version = u32_at(header, 8)?;
    if version != 2 {
        return Err(bad(format!("archive version {version} (only 2 is known)")));
    }
    let data_offset = u64::from(u32_at(header, 176)?);
    let s = section;
    let folder_off = u32_at(s, 6)? as usize;
    let folder_count = u16_at(s, 10)? as usize;
    let file_off = u32_at(s, 12)? as usize;
    let file_count = u16_at(s, 16)? as usize;
    let name_off = u32_at(s, 18)? as usize;
    let names = s
        .get(name_off..)
        .ok_or_else(|| bad("name table out of range"))?;
    let mut out = Vec::with_capacity(file_count);
    for f in 0..folder_count {
        let o = folder_off + f * 12;
        let folder = cstr(names, u32_at(s, o)? as usize)?;
        let (first, last) = (u16_at(s, o + 8)? as usize, u16_at(s, o + 10)? as usize);
        for i in first..last.min(file_count) {
            let fo = file_off + i * 17;
            let name = cstr(names, u32_at(s, fo)? as usize)?;
            let comp = *s.get(fo + 4).ok_or_else(|| bad("file entry truncated"))?;
            let path = if folder.is_empty() {
                normalize(&name)
            } else {
                normalize(&format!("{folder}\\{name}"))
            };
            out.push(Entry {
                path,
                compressed: comp != 0,
                offset: data_offset + u64::from(u32_at(s, fo + 5)?),
                stored_len: u32_at(s, fo + 9)?,
                real_len: u32_at(s, fo + 13)?,
            });
        }
    }
    Ok(out)
}

impl Archive {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let mut header = vec![0u8; SECTION_HEADER as usize];
        file.read_exact(&mut header)?;
        if &header[..8] != MAGIC {
            return Err(bad("not a Homeworld 2 archive"));
        }
        let size = u32_at(&header, 172)? as usize;
        if size > 64 << 20 {
            return Err(bad("section header too large"));
        }
        let mut section = vec![0u8; size];
        file.read_exact(&mut section)?;
        let entries = parse_index(&header, &section)?;
        let by_path = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.path.clone(), i))
            .collect();
        Ok(Self {
            file,
            entries,
            by_path,
        })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn find(&self, path: &str) -> Option<&Entry> {
        self.by_path
            .get(&normalize(path))
            .map(|&i| &self.entries[i])
    }

    /// Reads and inflates one entry.
    pub fn read_entry(&mut self, e: &Entry) -> io::Result<Vec<u8>> {
        self.file.seek(SeekFrom::Start(e.offset))?;
        let mut stored = vec![0u8; e.stored_len as usize];
        self.file.read_exact(&mut stored)?;
        if !e.compressed {
            return Ok(stored);
        }
        let mut out = Vec::with_capacity(e.real_len as usize);
        flate2::read::ZlibDecoder::new(&stored[..]).read_to_end(&mut out)?;
        if out.len() != e.real_len as usize {
            return Err(bad(format!(
                "{}: inflated to {} bytes, expected {}",
                e.path,
                out.len(),
                e.real_len
            )));
        }
        Ok(out)
    }

    /// Reads the file at `path`, `None` when the archive has no such file.
    pub fn read(&mut self, path: &str) -> io::Result<Option<Vec<u8>>> {
        match self.find(path).cloned() {
            Some(e) => self.read_entry(&e).map(Some),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// Builds a small archive: one folder per distinct directory, zlib for `compress` files.
    pub fn build(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut folders: Vec<String> = Vec::new();
        for (p, _, _) in files {
            let dir = p.rsplit_once('\\').map_or("", |(d, _)| d).to_string();
            if !folders.contains(&dir) {
                folders.push(dir);
            }
        }
        let mut names = Vec::new();
        let mut name = |s: &str| {
            let o = names.len() as u32;
            names.extend_from_slice(s.as_bytes());
            names.push(0);
            o
        };
        let mut data = Vec::new();
        let mut file_recs = Vec::new();
        let mut folder_recs = Vec::new();
        for dir in &folders {
            let first = file_recs.len() as u16;
            let fname = name(dir);
            for (p, bytes, compress) in files {
                let (d, f) = p.rsplit_once('\\').unwrap_or(("", p));
                if d != dir {
                    continue;
                }
                let stored = if *compress {
                    let mut z =
                        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                    z.write_all(bytes).unwrap();
                    z.finish().unwrap()
                } else {
                    bytes.to_vec()
                };
                let mut r = Vec::new();
                r.extend_from_slice(&name(f).to_le_bytes());
                r.push(if *compress { 32 } else { 0 });
                r.extend_from_slice(&(data.len() as u32).to_le_bytes());
                r.extend_from_slice(&(stored.len() as u32).to_le_bytes());
                r.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                data.extend_from_slice(&stored);
                file_recs.push(r);
            }
            let last = file_recs.len() as u16;
            let mut r = Vec::new();
            r.extend_from_slice(&fname.to_le_bytes());
            r.extend_from_slice(&0u16.to_le_bytes());
            r.extend_from_slice(&0u16.to_le_bytes());
            r.extend_from_slice(&first.to_le_bytes());
            r.extend_from_slice(&last.to_le_bytes());
            folder_recs.push(r);
        }
        let toc_off = 24u32;
        let folder_off = toc_off + 138;
        let file_off = folder_off + 12 * folder_recs.len() as u32;
        let name_off = file_off + 17 * file_recs.len() as u32;
        let mut s = Vec::new();
        for (o, c) in [
            (toc_off, 1u16),
            (folder_off, folder_recs.len() as u16),
            (file_off, file_recs.len() as u16),
            (name_off, 0),
        ] {
            s.extend_from_slice(&o.to_le_bytes());
            s.extend_from_slice(&c.to_le_bytes());
        }
        s.resize(toc_off as usize + 138, 0);
        for r in folder_recs.iter().chain(file_recs.iter()) {
            s.extend_from_slice(r);
        }
        s.extend_from_slice(&names);
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&2u32.to_le_bytes());
        out.resize(172, 0);
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(&(180 + s.len() as u32).to_le_bytes());
        out.extend_from_slice(&s);
        out.extend_from_slice(&data);
        out
    }

    #[test]
    fn normalises_paths() {
        assert_eq!(
            normalize("DATA:ART/FX/TEXTURES/BIGFIREBALL.TGA"),
            "art/fx/textures/bigfireball.tga"
        );
        assert_eq!(
            normalize("data:/art/fx/Bullet_Splash/Shock01.tga"),
            "art/fx/bullet_splash/shock01.tga"
        );
        assert_eq!(normalize("art\\fx\\debris/a.lua"), "art/fx/debris/a.lua");
    }

    #[test]
    fn reads_stored_and_zlib_files() {
        let bytes = build(&[
            ("art\\fx\\a.lua", b"fx = {}", true),
            ("art\\fx\\b.tga", b"\x00\x01\x02", false),
            ("keeper.txt", b"keep", false),
        ]);
        let dir = std::env::temp_dir().join(format!("hwc-hwr-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.big");
        std::fs::write(&path, &bytes).unwrap();
        let mut a = Archive::open(&path).unwrap();
        assert_eq!(a.entries().len(), 3);
        assert_eq!(a.read("ART/FX/A.LUA").unwrap().unwrap(), b"fx = {}");
        assert_eq!(a.read("art/fx/b.tga").unwrap().unwrap(), b"\x00\x01\x02");
        assert_eq!(a.read("keeper.txt").unwrap().unwrap(), b"keep");
        assert!(a.read("missing").unwrap().is_none());
        drop(a);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
