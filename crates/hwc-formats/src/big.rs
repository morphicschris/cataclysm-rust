//! Reader for `.big` archives (`RBF1.23`).
//!
//! Layout:
//!
//! * Header: the 7 ASCII bytes `RBF1.23`, then `i32` file count and `i32` flags (1 = TOC sorted by
//!   name CRC).
//! * Table of contents: one 32-byte little-endian record per file (see [`TOC_ENTRY_SIZE`]).
//! * At each record's offset: the obfuscated file name (`name_len` bytes plus a NUL), immediately
//!   followed by `stored_len` bytes of data, LZSS-compressed when the compression byte is 1.
//!
//! Names are stored with backslash separators. Each record carries two CRC-32s of the lowercased
//! name (first half and second half), which [`BigArchive::find`] uses for lookups.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::lzss::{self, LzssError};

const MAGIC: &[u8; 7] = b"RBF1.23";
const HEADER_SIZE: u64 = 7 + 4 + 4;
/// Size of one table-of-contents record in bytes.
pub const TOC_ENTRY_SIZE: usize = 32;
/// Seed for the running-XOR name obfuscation.
const NAME_KEY: u8 = 213;

#[derive(Debug, Error)]
pub enum BigError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("not a .big archive (bad signature)")]
    BadMagic,
    #[error("corrupt archive: {0}")]
    Corrupt(String),
    #[error("{name}: {source}")]
    Decompress { name: String, source: LzssError },
    #[error("{name}: stored size {stored} differs from real size {real} for an uncompressed entry")]
    SizeMismatch { name: String, stored: u32, real: u32 },
}

pub type Result<T, E = BigError> = std::result::Result<T, E>;

/// One file stored in an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigEntry {
    /// Name as stored (decrypted), backslash-separated, original case.
    pub name: String,
    pub name_crc1: u32,
    pub name_crc2: u32,
    /// Bytes occupied in the archive.
    pub stored_len: u32,
    /// Bytes after decompression.
    pub real_len: u32,
    /// Absolute file offset of the entry's data (just past its name).
    pub data_offset: u64,
    pub timestamp: u32,
    pub compressed: bool,
}

/// An open `.big` archive. Data is read on demand with positioned reads, so the archive can be
/// shared between threads.
#[derive(Debug)]
pub struct BigArchive {
    path: PathBuf,
    file: File,
    sorted_by_crc: bool,
    entries: Vec<BigEntry>,
    by_crc: HashMap<(u32, u32), usize>,
}

impl BigArchive {
    /// Opens an archive and reads its table of contents and file names.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        let file_len = file.metadata()?.len();

        let mut header = [0u8; HEADER_SIZE as usize];
        read_exact_at(&file, &mut header, 0).map_err(|_| BigError::BadMagic)?;
        if &header[..7] != MAGIC {
            return Err(BigError::BadMagic);
        }
        let count = i32::from_le_bytes(header[7..11].try_into().unwrap());
        let flags = i32::from_le_bytes(header[11..15].try_into().unwrap());
        let count = usize::try_from(count)
            .map_err(|_| BigError::Corrupt(format!("negative file count {count}")))?;
        let toc_bytes = count
            .checked_mul(TOC_ENTRY_SIZE)
            .filter(|&n| HEADER_SIZE + n as u64 <= file_len)
            .ok_or_else(|| BigError::Corrupt(format!("table of {count} entries exceeds file size")))?;

        let mut toc = vec![0u8; toc_bytes];
        read_exact_at(&file, &mut toc, HEADER_SIZE)?;

        let mut entries = Vec::with_capacity(count);
        let mut name_buf = Vec::new();
        for (index, rec) in toc.as_chunks::<TOC_ENTRY_SIZE>().0.iter().enumerate() {
            let u32_at = |o: usize| u32::from_le_bytes(rec[o..o + 4].try_into().unwrap());
            let name_len = u16::from_le_bytes([rec[8], rec[9]]) as usize;
            let stored_len = u32_at(12);
            let real_len = u32_at(16);
            let offset = u64::from(u32_at(20));
            let timestamp = u32_at(24);
            let compression = rec[28];

            let data_offset = offset + name_len as u64 + 1;
            if data_offset + u64::from(stored_len) > file_len {
                return Err(BigError::Corrupt(format!("entry {index} extends past end of file")));
            }
            let compressed = match compression {
                0 => false,
                1 => true,
                other => {
                    return Err(BigError::Corrupt(format!("entry {index}: unknown compression type {other}")))
                }
            };

            name_buf.resize(name_len, 0);
            read_exact_at(&file, &mut name_buf, offset)?;
            decrypt_name(&mut name_buf);
            // Names are ASCII in practice; anything else is kept losslessly as Latin-1.
            let name: String = name_buf.iter().map(|&b| b as char).collect();

            entries.push(BigEntry {
                name,
                name_crc1: u32_at(0),
                name_crc2: u32_at(4),
                stored_len,
                real_len,
                data_offset,
                timestamp,
                compressed,
            });
        }

        let by_crc = entries
            .iter()
            .enumerate()
            .map(|(i, e)| ((e.name_crc1, e.name_crc2), i))
            .collect();

        Ok(Self { path, file, sorted_by_crc: flags == 1, entries, by_crc })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the header flags say the table of contents is sorted by name CRC.
    pub fn sorted_by_crc(&self) -> bool {
        self.sorted_by_crc
    }

    pub fn entries(&self) -> &[BigEntry] {
        &self.entries
    }

    /// Looks up a file by name, ignoring ASCII case and accepting `/` or `\` as separators.
    pub fn find(&self, name: &str) -> Option<&BigEntry> {
        let key = normalize_name(name);
        let (c1, c2) = name_crcs(key.as_bytes());
        let entry = &self.entries[*self.by_crc.get(&(c1, c2))?];
        // Guard against CRC collisions (or a TOC whose CRCs were computed differently).
        (normalize_name(&entry.name) == key).then_some(entry)
    }

    /// Reads an entry's raw stored bytes (still compressed if `entry.compressed`).
    pub fn read_raw(&self, entry: &BigEntry) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; entry.stored_len as usize];
        read_exact_at(&self.file, &mut buf, entry.data_offset)?;
        Ok(buf)
    }

    /// Reads and, if needed, decompresses an entry.
    pub fn read(&self, entry: &BigEntry) -> Result<Vec<u8>> {
        let raw = self.read_raw(entry)?;
        if entry.compressed {
            lzss::decompress(&raw, entry.real_len as usize)
                .map_err(|source| BigError::Decompress { name: entry.name.clone(), source })
        } else if entry.stored_len != entry.real_len {
            Err(BigError::SizeMismatch {
                name: entry.name.clone(),
                stored: entry.stored_len,
                real: entry.real_len,
            })
        } else {
            Ok(raw)
        }
    }

    /// Convenience: [`find`](Self::find) then [`read`](Self::read).
    pub fn read_file(&self, name: &str) -> Result<Option<Vec<u8>>> {
        self.find(name).map(|e| self.read(e)).transpose()
    }
}

/// Reverses the archive's name obfuscation in place: each byte is XORed with the previous
/// *plain* byte, the first with a fixed key.
pub fn decrypt_name(bytes: &mut [u8]) {
    let mut key = NAME_KEY;
    for b in bytes {
        *b ^= key;
        key = *b;
    }
}

/// Canonical form used for lookups and CRCs: ASCII lowercase with backslash separators.
pub fn normalize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c == '/' { '\\' } else { c.to_ascii_lowercase() })
        .collect()
}

/// The two TOC CRCs for an already-normalized name: CRC-32 of the first `len/2` bytes and of the
/// `len/2` bytes after them (for odd lengths the final byte is not covered).
pub fn name_crcs(name: &[u8]) -> (u32, u32) {
    let half = name.len() / 2;
    (crc32(&name[..half]), crc32(&name[half..half * 2]))
}

/// Standard reflected CRC-32 (polynomial 0xEDB88320, init and final XOR 0xFFFFFFFF).
pub fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            table[i] = c;
            i += 1;
        }
        table
    };
    !data
        .iter()
        .fold(!0u32, |crc, &b| TABLE[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8))
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)
}

#[cfg(not(any(windows, unix)))]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    // `&File` implements Read + Seek; callers on these platforms must not share across threads.
    let mut f = file;
    f.seek(SeekFrom::Start(offset))?;
    f.read_exact(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encrypt(plain: &[u8]) -> Vec<u8> {
        let mut key = NAME_KEY;
        plain
            .iter()
            .map(|&p| {
                let c = p ^ key;
                key = p;
                c
            })
            .collect()
    }

    #[test]
    fn name_round_trip() {
        let plain = b"multiplayer\\armed8\\type1.dist";
        let mut bytes = encrypt(plain);
        assert_ne!(&bytes[..], &plain[..]);
        decrypt_name(&mut bytes);
        assert_eq!(&bytes[..], &plain[..]);
    }

    #[test]
    fn name_decrypt_known_bytes() {
        // 'a' = 0x61 -> 0x61 ^ 213 = 0xB4; 'b' = 0x62 -> 0x62 ^ 0x61 = 0x03.
        let mut bytes = [0xB4, 0x03];
        decrypt_name(&mut bytes);
        assert_eq!(&bytes, b"ab");
    }

    #[test]
    fn crc32_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn normalization() {
        assert_eq!(normalize_name("Multiplayer/Armed8\\TYPE1.dist"), "multiplayer\\armed8\\type1.dist");
    }

    #[test]
    fn odd_length_crc_skips_last_byte() {
        assert_eq!(name_crcs(b"abcde"), name_crcs(b"abcdX"));
        assert_eq!(name_crcs(b"abcd"), (crc32(b"ab"), crc32(b"cd")));
    }
}
