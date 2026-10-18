//! Decoder for the LZSS variant used by `.big` archives (compression type 1).
//!
//! The stream is a sequence of bit-packed tokens, most significant bit of each byte first:
//!
//! * `1` + 8 bits: a literal byte.
//! * `0` + 12 bits position + 4 bits length: copy `length + 2` bytes starting at `position` in the
//!   4096-byte history window. Position 0 is never a valid match source and instead marks the end of
//!   the stream.
//!
//! The window starts zero-filled and the write cursor starts at index 1.

use thiserror::Error;

const INDEX_BITS: u32 = 12;
const LENGTH_BITS: u32 = 4;
const WINDOW_SIZE: usize = 1 << INDEX_BITS;
const WINDOW_MASK: usize = WINDOW_SIZE - 1;
/// Shortest encoded match; a match token stores `length - MIN_MATCH`.
const MIN_MATCH: usize = 2;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LzssError {
    #[error("LZSS input ended before the end-of-stream marker ({produced} of {expected} bytes decoded)")]
    TruncatedInput { produced: usize, expected: usize },
    #[error("LZSS output overran the expected size of {expected} bytes")]
    Overrun { expected: usize },
    #[error("LZSS stream ended after {produced} bytes, expected {expected}")]
    SizeMismatch { produced: usize, expected: usize },
}

/// MSB-first bit reader over a byte slice.
struct BitReader<'a> {
    data: &'a [u8],
    next: usize,
    /// Pending bits, left-aligned (the next bit to read is bit 63).
    buf: u64,
    avail: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, next: 0, buf: 0, avail: 0 }
    }

    #[inline]
    fn refill(&mut self) {
        while self.avail <= 56 {
            let Some(&b) = self.data.get(self.next) else { break };
            self.next += 1;
            self.buf |= u64::from(b) << (56 - self.avail);
            self.avail += 8;
        }
    }

    /// Reads `n` (1..=32) bits, or `None` if the input is exhausted.
    #[inline]
    fn bits(&mut self, n: u32) -> Option<u32> {
        if self.avail < n {
            self.refill();
            if self.avail < n {
                return None;
            }
        }
        let v = (self.buf >> (64 - n)) as u32;
        self.buf <<= n;
        self.avail -= n;
        Some(v)
    }
}

/// Decompresses `input`, which must decode to exactly `expected_len` bytes.
///
/// A stream whose end-of-stream marker is missing (input runs out mid-token) is accepted only if
/// exactly `expected_len` bytes had already been produced.
pub fn decompress(input: &[u8], expected_len: usize) -> Result<Vec<u8>, LzssError> {
    let mut out = Vec::with_capacity(expected_len);
    match decode_into(input, expected_len, &mut out) {
        Ok(()) if out.len() == expected_len => Ok(out),
        Ok(()) => Err(LzssError::SizeMismatch { produced: out.len(), expected: expected_len }),
        Err(LzssError::TruncatedInput { .. }) if out.len() == expected_len => Ok(out),
        Err(e) => Err(e),
    }
}

fn decode_into(input: &[u8], expected_len: usize, out: &mut Vec<u8>) -> Result<(), LzssError> {
    let mut window = [0u8; WINDOW_SIZE];
    let mut pos = 1usize;
    let mut bits = BitReader::new(input);

    macro_rules! take {
        ($n:expr) => {
            match bits.bits($n) {
                Some(v) => v,
                None => {
                    return Err(LzssError::TruncatedInput { produced: out.len(), expected: expected_len })
                }
            }
        };
    }

    loop {
        if take!(1) == 1 {
            let byte = take!(8) as u8;
            if out.len() >= expected_len {
                return Err(LzssError::Overrun { expected: expected_len });
            }
            out.push(byte);
            window[pos] = byte;
            pos = (pos + 1) & WINDOW_MASK;
        } else {
            let src = take!(INDEX_BITS) as usize;
            if src == 0 {
                return Ok(());
            }
            let len = take!(LENGTH_BITS) as usize + MIN_MATCH;
            if out.len() + len > expected_len {
                return Err(LzssError::Overrun { expected: expected_len });
            }
            // Byte-by-byte so that overlapping copies (src just behind pos) repeat correctly.
            for i in 0..len {
                let byte = window[(src + i) & WINDOW_MASK];
                out.push(byte);
                window[pos] = byte;
                pos = (pos + 1) & WINDOW_MASK;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds MSB-first bit streams for tests.
    #[derive(Default)]
    struct BitWriter {
        bytes: Vec<u8>,
        used: u32,
    }

    impl BitWriter {
        fn put(&mut self, value: u32, n: u32) -> &mut Self {
            for i in (0..n).rev() {
                if self.used == 0 {
                    self.bytes.push(0);
                }
                let bit = ((value >> i) & 1) as u8;
                *self.bytes.last_mut().unwrap() |= bit << (7 - self.used);
                self.used = (self.used + 1) % 8;
            }
            self
        }
        fn literal(&mut self, b: u8) -> &mut Self {
            self.put(1, 1).put(u32::from(b), 8)
        }
        fn copy(&mut self, src: u32, len: u32) -> &mut Self {
            self.put(0, 1).put(src, 12).put(len - MIN_MATCH as u32, 4)
        }
        fn end(&mut self) -> Vec<u8> {
            self.put(0, 1).put(0, 12);
            std::mem::take(&mut self.bytes)
        }
    }

    #[test]
    fn empty_stream() {
        let data = BitWriter::default().end();
        assert_eq!(decompress(&data, 0).unwrap(), b"");
    }

    #[test]
    fn literals_only() {
        let mut w = BitWriter::default();
        for &b in b"Hi!" {
            w.literal(b);
        }
        let data = w.end();
        assert_eq!(decompress(&data, 3).unwrap(), b"Hi!");
    }

    #[test]
    fn hand_packed_bytes() {
        // '1' + 0x41 ('A'), then the end marker '0' + 12 zero bits, padded with zeros.
        // 1010_0000 1000_0000 0000_0000 -> A0 80 00
        assert_eq!(decompress(&[0xA0, 0x80, 0x00], 1).unwrap(), b"A");
    }

    #[test]
    fn back_reference() {
        // "abc" lands at window[1..4]; copy 3 bytes from position 1.
        let data = BitWriter::default().literal(b'a').literal(b'b').literal(b'c').copy(1, 3).end();
        assert_eq!(decompress(&data, 6).unwrap(), b"abcabc");
    }

    #[test]
    fn overlapping_run() {
        // One 'x' at window[1], then copying 17 bytes from 1 replicates it as it is written.
        let data = BitWriter::default().literal(b'x').copy(1, 17).end();
        assert_eq!(decompress(&data, 18).unwrap(), vec![b'x'; 18]);
    }

    #[test]
    fn reference_to_unwritten_window_is_zero() {
        let data = BitWriter::default().copy(100, 2).end();
        assert_eq!(decompress(&data, 2).unwrap(), vec![0, 0]);
    }

    #[test]
    fn window_wraps() {
        let mut w = BitWriter::default();
        let src: Vec<u8> = (0..5000u32).map(|i| (i * 7 % 251) as u8).collect();
        for &b in &src {
            w.literal(b);
        }
        // Literal k lands at window[(k + 1) % 4096], so after 5000 literals window[1] holds
        // literal 4096 (the first one written on the second lap).
        w.copy(1, 4);
        let data = w.end();
        let mut expected = src.clone();
        expected.extend_from_slice(&src[4096..4100]);
        assert_eq!(decompress(&data, expected.len()).unwrap(), expected);
    }

    #[test]
    fn errors() {
        let data = BitWriter::default().literal(b'a').literal(b'b').end();
        assert_eq!(decompress(&data, 1), Err(LzssError::Overrun { expected: 1 }));
        assert_eq!(decompress(&data, 3), Err(LzssError::SizeMismatch { produced: 2, expected: 3 }));
        let cut = &data[..1];
        assert!(matches!(decompress(cut, 2), Err(LzssError::TruncatedInput { .. })));
    }

    #[test]
    fn missing_terminator_tolerated_when_complete() {
        let mut w = BitWriter::default();
        w.literal(b'z');
        let data = std::mem::take(&mut w.bytes);
        assert_eq!(decompress(&data, 1).unwrap(), b"z");
    }
}
