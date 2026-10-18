//! SHA-256 helpers. Hashes are stored as lowercase hex.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

/// Chunk size for streaming reads and copies.
pub const CHUNK: usize = 1 << 20;

/// Lowercase hex of a digest.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(DIGITS[usize::from(b >> 4)] as char);
        s.push(DIGITS[usize::from(b & 15)] as char);
    }
    s
}

/// SHA-256 of a byte slice.
pub fn sha256_bytes(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// SHA-256 of a file, read in chunks; `on_bytes` is called with each chunk's length.
pub fn sha256_file(path: &Path, on_bytes: &mut dyn FnMut(u64)) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        on_bytes(n as u64);
    }
    Ok(hex(&hasher.finalize()))
}

/// Copies `src` to `dst` (which must not exist or will be truncated), hashing the bytes on the
/// way. Returns the byte count and the hash.
pub fn copy_hashed(src: &Path, dst: &Path, on_bytes: &mut dyn FnMut(u64)) -> io::Result<(u64, String)> {
    let mut input = File::open(src)?;
    let mut output = File::create(dst)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        output.write_all(&buf[..n])?;
        total += n as u64;
        on_bytes(n as u64);
    }
    output.sync_all()?;
    Ok((total, hex(&hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_digest() {
        assert_eq!(sha256_bytes(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    }
}
