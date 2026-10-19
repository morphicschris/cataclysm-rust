//! Little-endian cursor shared by the binary formats.

use std::fmt;

/// A malformed or truncated file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError {
    /// Byte offset where the problem was found.
    pub offset: usize,
    pub message: String,
}

impl FormatError {
    pub(crate) fn new(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset,
            message: message.into(),
        }
    }
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at byte {:#x}: {}", self.offset, self.message)
    }
}

impl std::error::Error for FormatError {}

impl From<FormatError> for std::io::Error {
    fn from(e: FormatError) -> Self {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e)
    }
}

pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub fn bytes(&mut self, n: usize, what: &str) -> Result<&'a [u8], FormatError> {
        if self.remaining() < n {
            return Err(FormatError::new(
                self.pos,
                format!("truncated {what}: need {n} bytes, {} left", self.remaining()),
            ));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn array<const N: usize>(&mut self, what: &str) -> Result<[u8; N], FormatError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N, what)?);
        Ok(out)
    }

    pub fn u8(&mut self, what: &str) -> Result<u8, FormatError> {
        Ok(self.array::<1>(what)?[0])
    }

    pub fn u16(&mut self, what: &str) -> Result<u16, FormatError> {
        Ok(u16::from_le_bytes(self.array(what)?))
    }

    pub fn i16(&mut self, what: &str) -> Result<i16, FormatError> {
        Ok(i16::from_le_bytes(self.array(what)?))
    }

    pub fn u32(&mut self, what: &str) -> Result<u32, FormatError> {
        Ok(u32::from_le_bytes(self.array(what)?))
    }

    pub fn i32(&mut self, what: &str) -> Result<i32, FormatError> {
        Ok(i32::from_le_bytes(self.array(what)?))
    }

    pub fn f32(&mut self, what: &str) -> Result<f32, FormatError> {
        Ok(f32::from_le_bytes(self.array(what)?))
    }
}

pub(crate) fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn le_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

pub(crate) fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
