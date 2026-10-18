//! LSB-first bit reader: Bink packs bits starting from the least significant bit of each
//! little-endian byte.

pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    /// Position in bits from the start of `data`.
    pos: usize,
    len_bits: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            len_bits: data.len() * 8,
        }
    }

    /// 64 bits starting at the current byte, shifted so bit 0 is the next unread bit.
    /// At least 57 meaningful bits; anything past the end of the data reads as zero.
    #[inline(always)]
    fn window(&self) -> u64 {
        let byte = self.pos >> 3;
        let word = if let Some(chunk) = self.data.get(byte..byte + 8) {
            u64::from_le_bytes(chunk.try_into().unwrap())
        } else {
            let mut buf = [0u8; 8];
            if byte < self.data.len() {
                let tail = &self.data[byte..];
                buf[..tail.len()].copy_from_slice(tail);
            }
            u64::from_le_bytes(buf)
        };
        word >> (self.pos & 7)
    }

    /// Reads `n` bits (0..=32).
    #[inline(always)]
    pub fn read(&mut self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        if n == 0 {
            return 0;
        }
        let v = (self.window() & ((1u64 << n) - 1)) as u32;
        self.pos += n as usize;
        v
    }

    #[inline(always)]
    pub fn bit(&mut self) -> bool {
        self.read(1) != 0
    }

    /// The next `n` bits (n <= 32) without consuming them.
    #[inline(always)]
    pub fn peek(&self, n: u32) -> u32 {
        (self.window() & ((1u64 << n) - 1)) as u32
    }

    #[inline(always)]
    pub fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn len_bits(&self) -> usize {
        self.len_bits
    }

    /// True once more bits have been consumed than the packet holds.
    pub fn overrun(&self) -> bool {
        self.pos > self.len_bits
    }

    /// Advances to the next multiple of 32 bits.
    pub fn align32(&mut self) {
        let rem = self.pos & 31;
        if rem != 0 {
            self.pos += 32 - rem;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lsb_first() {
        let data = [0b1010_0110u8, 0xff, 0x01];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(1), 0);
        assert_eq!(r.read(2), 0b11);
        assert_eq!(r.read(5), 0b10100);
        assert_eq!(r.read(9), 0x1ff);
        assert_eq!(r.position(), 17);
        r.align32();
        assert_eq!(r.position(), 32);
        assert_eq!(r.read(32), 0);
        assert!(r.overrun());
    }
}
