//! The constant tables of the Bink 1 video codec: the sixteen Huffman codebooks, the DCT
//! coefficient scan, the sixteen run-length scan orders and the intra/inter dequantisation
//! tables.
//!
//! The project does not ship these. They are taken from the player's own copy of RAD's
//! `binkw32.dll` (installed with the game), located by their structure rather than by address,
//! and accepted only when each table's checksum matches the one expected. The importer can store
//! the result as a small file ([`import_tables`] / [`BinkTables::from_serialized`]) so the DLL is
//! only read once.

use std::path::Path;

use crate::{Error, Result};

/// File name the importer should use for [`import_tables`] output in the data directory.
pub const TABLES_FILE_NAME: &str = "bink_tables.bin";

const MAGIC: &[u8; 8] = b"HWCBINKT";
const FORMAT_VERSION: u32 = 1;

/// Number of codebooks, symbols per codebook, quantiser levels and coefficients per block.
const CODEBOOKS: usize = 16;
const SYMBOLS: usize = 16;
const QUANT_LEVELS: usize = 16;
const COEFFS: usize = 64;
const RUN_SCAN_COUNT: usize = 16;
/// Longest code in any codebook; also the width of the decode lookup.
pub(crate) const HUFF_PEEK: u32 = 7;
/// The DLL stores each codebook as a direct lookup of at most 2^8 entries.
const MAX_LOOKUP_BITS: u32 = 8;

const CODEBOOK_BYTES: usize = CODEBOOKS * SYMBOLS * 2;
const SCAN_BYTES: usize = COEFFS;
const RUN_SCAN_BYTES: usize = RUN_SCAN_COUNT * COEFFS;
const QUANT_BYTES: usize = QUANT_LEVELS * COEFFS * 4;
const SERIALIZED_LEN: usize =
    8 + 4 + CODEBOOK_BYTES + SCAN_BYTES + RUN_SCAN_BYTES + 2 * QUANT_BYTES + 8;

// FNV-1a 64 checksums of each table in its canonical byte form (see `canonical_*`).
const SUM_CODEBOOKS: u64 = 0xc829_0a7c_d5de_dd37;
const SUM_DCT_SCAN: u64 = 0xb64a_a0b2_5997_94e5;
const SUM_RUN_SCANS: u64 = 0xb9b7_4dd9_b0b8_86e5;
const SUM_INTRA_QUANT: u64 = 0x64d8_1e2e_4338_2bae;
const SUM_INTER_QUANT: u64 = 0x9b81_3dd7_a89e_7b3f;

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

type Codebooks = [[(u8, u8); SYMBOLS]; CODEBOOKS];
type QuantTable = [[i32; COEFFS]; QUANT_LEVELS];

/// Codec tables needed by [`crate::VideoDecoder`].
#[derive(Clone)]
pub struct BinkTables {
    /// `(code, length)` per symbol slot; codes are read LSB-first.
    pub(crate) codebooks: Codebooks,
    /// Per codebook, indexed by the next [`HUFF_PEEK`] bits: `(symbol slot, code length)`.
    pub(crate) lookup: [[(u8, u8); 1 << HUFF_PEEK]; CODEBOOKS],
    /// Scan index -> raster position for DCT and residue coefficients.
    pub(crate) dct_scan: [u8; COEFFS],
    /// Scan index -> raster position for run-length blocks.
    pub(crate) run_scans: [[u8; COEFFS]; RUN_SCAN_COUNT],
    /// Dequantisation multipliers per quantiser and raster position (11 fractional bits).
    pub(crate) intra_quant: QuantTable,
    pub(crate) inter_quant: QuantTable,
}

impl std::fmt::Debug for BinkTables {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BinkTables").finish_non_exhaustive()
    }
}

fn canonical_codebooks(c: &Codebooks) -> Vec<u8> {
    c.iter()
        .flatten()
        .flat_map(|&(code, len)| [code, len])
        .collect()
}

fn canonical_run_scans(s: &[[u8; COEFFS]; RUN_SCAN_COUNT]) -> Vec<u8> {
    s.iter().flatten().copied().collect()
}

fn canonical_quant(q: &QuantTable) -> Vec<u8> {
    q.iter().flatten().flat_map(|v| v.to_le_bytes()).collect()
}

fn is_permutation(bytes: &[u8]) -> bool {
    if bytes.len() != COEFFS {
        return false;
    }
    let mut seen = 0u64;
    for &b in bytes {
        if b >= COEFFS as u8 {
            return false;
        }
        seen |= 1 << b;
    }
    seen == u64::MAX
}

/// Interprets `bytes[pos..]` as a direct-lookup Huffman table whose entries are
/// `(length << 4) | symbol`, as RAD stores them. Returns the codes and the table size.
fn parse_lookup(bytes: &[u8], pos: usize) -> Option<([(u8, u8); SYMBOLS], usize)> {
    'size: for bits in 4..=MAX_LOOKUP_BITS {
        let size = 1usize << bits;
        let table = bytes.get(pos..pos + size)?;
        let mut codes = [(0u8, 0u8); SYMBOLS];
        let mut found = [false; SYMBOLS];
        let mut longest = 0;
        for (i, &e) in table.iter().enumerate() {
            let (len, sym) = ((e >> 4) as u32, (e & 15) as usize);
            if len == 0 || len > bits {
                continue 'size;
            }
            longest = longest.max(len);
            let code = (i & ((1 << len) - 1)) as u8;
            if found[sym] {
                if codes[sym] != (code, len as u8) {
                    return None;
                }
            } else {
                found[sym] = true;
                codes[sym] = (code, len as u8);
            }
        }
        if longest != bits || !found.iter().all(|&f| f) {
            return None;
        }
        // Complete prefix code: every slot covered exactly once.
        let kraft: usize = codes.iter().map(|&(_, l)| size >> l).sum();
        return (kraft == size).then_some((codes, size));
    }
    None
}

/// Sixteen consecutive lookup tables starting at `pos`.
fn parse_codebooks(bytes: &[u8], pos: usize) -> Option<Codebooks> {
    let mut out = [[(0u8, 0u8); SYMBOLS]; CODEBOOKS];
    let mut at = pos;
    for book in out.iter_mut() {
        let (codes, size) = parse_lookup(bytes, at)?;
        *book = codes;
        at += size;
    }
    Some(out)
}

fn find_codebooks(dll: &[u8]) -> Option<Codebooks> {
    (0..dll.len())
        .filter(|&p| matches!(dll[p] >> 4, 1..=8))
        .filter_map(|p| parse_codebooks(dll, p))
        .find(|c| fnv1a64(&canonical_codebooks(c)) == SUM_CODEBOOKS)
}

fn find_run_scans(dll: &[u8]) -> Option<[[u8; COEFFS]; RUN_SCAN_COUNT]> {
    let span = RUN_SCAN_BYTES;
    (0..dll.len().saturating_sub(span - 1))
        .filter(|&p| {
            (0..RUN_SCAN_COUNT).all(|k| is_permutation(&dll[p + k * COEFFS..p + (k + 1) * COEFFS]))
        })
        .map(|p| {
            std::array::from_fn(|k| {
                dll[p + k * COEFFS..p + (k + 1) * COEFFS]
                    .try_into()
                    .unwrap()
            })
        })
        .find(|s| fnv1a64(&canonical_run_scans(s)) == SUM_RUN_SCANS)
}

fn find_dct_scan(dll: &[u8]) -> Option<[u8; COEFFS]> {
    dll.windows(COEFFS)
        .filter(|w| is_permutation(w))
        .find(|w| fnv1a64(w) == SUM_DCT_SCAN)
        .map(|w| w.try_into().unwrap())
}

/// A 16×64 table of positive 32-bit multipliers whose checksum is `sum`.
fn find_quant(dll: &[u8], sum: u64) -> Option<QuantTable> {
    let words = QUANT_LEVELS * COEFFS;
    for align in 0..4 {
        let n = dll.len().saturating_sub(align) / 4;
        let plausible: Vec<bool> = (0..n)
            .map(|i| {
                let p = align + 4 * i;
                let v = i32::from_le_bytes(dll[p..p + 4].try_into().unwrap());
                (1..0x0100_0000).contains(&v)
            })
            .collect();
        // Length of the plausible run starting at each word.
        let mut run = vec![0usize; n + 1];
        for i in (0..n).rev() {
            run[i] = if plausible[i] { run[i + 1] + 1 } else { 0 };
        }
        for (i, &len) in run.iter().enumerate().take(n) {
            if len >= words {
                let p = align + 4 * i;
                let bytes = &dll[p..p + QUANT_BYTES];
                if fnv1a64(bytes) == sum {
                    return Some(std::array::from_fn(|q| {
                        std::array::from_fn(|c| {
                            let o = (q * COEFFS + c) * 4;
                            i32::from_le_bytes(bytes[o..o + 4].try_into().unwrap())
                        })
                    }));
                }
            }
        }
    }
    None
}

impl BinkTables {
    /// Locates the tables inside the bytes of RAD's `binkw32.dll` (the copy installed with the
    /// game). Fails if any table cannot be found with the expected checksum.
    pub fn from_binkw32(dll: &[u8]) -> Result<Self> {
        let codebooks = find_codebooks(dll).ok_or(Error::TablesNotFound("Huffman codebooks"))?;
        let run_scans = find_run_scans(dll).ok_or(Error::TablesNotFound("run scan orders"))?;
        let dct_scan = find_dct_scan(dll).ok_or(Error::TablesNotFound("DCT scan order"))?;
        let intra_quant =
            find_quant(dll, SUM_INTRA_QUANT).ok_or(Error::TablesNotFound("intra quantiser"))?;
        let inter_quant =
            find_quant(dll, SUM_INTER_QUANT).ok_or(Error::TablesNotFound("inter quantiser"))?;
        Self::from_parts(codebooks, dct_scan, run_scans, intra_quant, inter_quant)
    }

    /// Reads `binkw32.dll` from a game install directory.
    pub fn from_install(game_dir: impl AsRef<Path>) -> Result<Self> {
        Self::from_binkw32(&std::fs::read(game_dir.as_ref().join("binkw32.dll"))?)
    }

    /// Loads tables previously written by [`import_tables`] / [`Self::serialize`].
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_serialized(&std::fs::read(path)?)
    }

    fn from_parts(
        codebooks: Codebooks,
        dct_scan: [u8; COEFFS],
        run_scans: [[u8; COEFFS]; RUN_SCAN_COUNT],
        intra_quant: QuantTable,
        inter_quant: QuantTable,
    ) -> Result<Self> {
        let checks = [
            (
                fnv1a64(&canonical_codebooks(&codebooks)),
                SUM_CODEBOOKS,
                "Huffman codebooks",
            ),
            (fnv1a64(&dct_scan), SUM_DCT_SCAN, "DCT scan order"),
            (
                fnv1a64(&canonical_run_scans(&run_scans)),
                SUM_RUN_SCANS,
                "run scan orders",
            ),
            (
                fnv1a64(&canonical_quant(&intra_quant)),
                SUM_INTRA_QUANT,
                "intra quantiser",
            ),
            (
                fnv1a64(&canonical_quant(&inter_quant)),
                SUM_INTER_QUANT,
                "inter quantiser",
            ),
        ];
        for (got, want, what) in checks {
            if got != want {
                return Err(Error::TablesMismatch(what));
            }
        }
        Ok(Self::build(
            codebooks,
            dct_scan,
            run_scans,
            intra_quant,
            inter_quant,
        ))
    }

    /// Assembles tables without checksum validation (structure is still required to be sane by
    /// the callers). Builds the decode lookups.
    fn build(
        codebooks: Codebooks,
        dct_scan: [u8; COEFFS],
        run_scans: [[u8; COEFFS]; RUN_SCAN_COUNT],
        intra_quant: QuantTable,
        inter_quant: QuantTable,
    ) -> Self {
        let mut lookup = [[(0u8, 0u8); 1 << HUFF_PEEK]; CODEBOOKS];
        for (table, codes) in lookup.iter_mut().zip(codebooks.iter()) {
            for (slot, &(code, len)) in codes.iter().enumerate() {
                let len = len.min(HUFF_PEEK as u8);
                let mut i = code as usize;
                while i < table.len() {
                    table[i] = (slot as u8, len);
                    i += 1 << len;
                }
            }
        }
        Self {
            codebooks,
            lookup,
            dct_scan,
            run_scans,
            intra_quant,
            inter_quant,
        }
    }

    /// Compact file form: magic, version, the tables in canonical byte order, FNV-1a checksum.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SERIALIZED_LEN);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&canonical_codebooks(&self.codebooks));
        out.extend_from_slice(&self.dct_scan);
        out.extend_from_slice(&canonical_run_scans(&self.run_scans));
        out.extend_from_slice(&canonical_quant(&self.intra_quant));
        out.extend_from_slice(&canonical_quant(&self.inter_quant));
        let sum = fnv1a64(&out);
        out.extend_from_slice(&sum.to_le_bytes());
        out
    }

    /// Parses the form produced by [`Self::serialize`], re-checking every table checksum.
    pub fn from_serialized(bytes: &[u8]) -> Result<Self> {
        let Parsed(codebooks, dct_scan, run_scans, intra, inter) = parse_serialized(bytes)?;
        Self::from_parts(codebooks, dct_scan, run_scans, intra, inter)
    }
}

struct Parsed(
    Codebooks,
    [u8; COEFFS],
    [[u8; COEFFS]; RUN_SCAN_COUNT],
    QuantTable,
    QuantTable,
);

fn parse_serialized(bytes: &[u8]) -> Result<Parsed> {
    if bytes.len() != SERIALIZED_LEN || &bytes[..8] != MAGIC {
        return Err(Error::Invalid("not a Bink tables file"));
    }
    if u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != FORMAT_VERSION {
        return Err(Error::Invalid("Bink tables file version"));
    }
    let body = &bytes[..SERIALIZED_LEN - 8];
    let sum = u64::from_le_bytes(bytes[SERIALIZED_LEN - 8..].try_into().unwrap());
    if fnv1a64(body) != sum {
        return Err(Error::Invalid("Bink tables file checksum"));
    }
    let mut at = 12;
    let mut take = |n: usize| {
        let s = &bytes[at..at + n];
        at += n;
        s
    };
    let cb = take(CODEBOOK_BYTES);
    let codebooks = std::array::from_fn(|b| {
        std::array::from_fn(|s| {
            let o = (b * SYMBOLS + s) * 2;
            (cb[o], cb[o + 1])
        })
    });
    let dct_scan = take(SCAN_BYTES).try_into().unwrap();
    let rs = take(RUN_SCAN_BYTES);
    let run_scans = std::array::from_fn(|k| rs[k * COEFFS..(k + 1) * COEFFS].try_into().unwrap());
    let quant = |q: &[u8]| -> QuantTable {
        std::array::from_fn(|l| {
            std::array::from_fn(|c| {
                let o = (l * COEFFS + c) * 4;
                i32::from_le_bytes(q[o..o + 4].try_into().unwrap())
            })
        })
    };
    let intra = quant(take(QUANT_BYTES));
    let inter = quant(take(QUANT_BYTES));
    Ok(Parsed(codebooks, dct_scan, run_scans, intra, inter))
}

/// Extracts the codec tables from the bytes of the player's `binkw32.dll` and returns them in
/// the compact form [`BinkTables::from_serialized`] / [`BinkTables::load`] read back. The
/// importer writes this to `<data dir>/`[`TABLES_FILE_NAME`].
pub fn import_tables(dll_bytes: &[u8]) -> Result<Vec<u8>> {
    Ok(BinkTables::from_binkw32(dll_bytes)?.serialize())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A structurally valid table set made up here (not the real one).
    fn synthetic_codebooks() -> Codebooks {
        // Codebook b: symbol 0 gets a 1-bit code when b is odd, everything else 4/5 bits.
        std::array::from_fn(|b| {
            if b % 2 == 0 {
                std::array::from_fn(|s| (s as u8, 4))
            } else {
                // 0 -> "0" (1 bit); the other 15 share the "1" half: 14 five-bit codes + 1 four-bit
                let mut c = [(0u8, 0u8); SYMBOLS];
                c[0] = (0, 1);
                c[1] = (0b001, 4); // ...1 then three bits 000 -> code 0b0001 (LSB first)
                let tails = (1u8..=15).filter(|t| t & 7 != 0);
                for (slot, t) in c.iter_mut().skip(2).zip(tails) {
                    *slot = ((t << 1) | 1, 5);
                }
                c
            }
        })
    }

    fn lookup_bytes(codes: &[(u8, u8); SYMBOLS]) -> Vec<u8> {
        let bits = codes.iter().map(|c| c.1).max().unwrap().max(4);
        let size = 1usize << bits;
        (0..size)
            .map(|i| {
                let (sym, &(_, len)) = codes
                    .iter()
                    .enumerate()
                    .find(|(_, &(code, len))| i & ((1 << len) - 1) == code as usize)
                    .unwrap();
                (len << 4) | sym as u8
            })
            .collect()
    }

    #[test]
    fn parses_rad_style_lookup_tables() {
        let books = synthetic_codebooks();
        let mut blob = vec![0u8; 13];
        for b in &books {
            blob.extend(lookup_bytes(b));
        }
        let parsed = parse_codebooks(&blob, 13).expect("parse");
        assert_eq!(parsed, books);
        assert!(parse_codebooks(&blob, 14).is_none());
    }

    #[test]
    fn rejects_incomplete_or_inconsistent_lookups() {
        // All entries length 4 but symbol 15 missing.
        let mut t: Vec<u8> = (0..16).map(|i| 0x40 | i as u8).collect();
        t[15] = 0x40;
        assert!(parse_lookup(&t, 0).is_none());
        // Length larger than the table.
        let t: Vec<u8> = (0..16).map(|i| 0x50 | i as u8).collect();
        assert!(parse_lookup(&t, 0).is_none());
    }

    #[test]
    fn permutation_check() {
        let mut p: Vec<u8> = (0..64).rev().collect();
        assert!(is_permutation(&p));
        p[3] = p[4];
        assert!(!is_permutation(&p));
    }

    #[test]
    fn serialized_form_round_trips_structurally() {
        let t = BinkTables::build(
            synthetic_codebooks(),
            std::array::from_fn(|i| i as u8),
            [std::array::from_fn(|i| 63 - i as u8); RUN_SCAN_COUNT],
            [[1 << 11; COEFFS]; QUANT_LEVELS],
            [[2 << 11; COEFFS]; QUANT_LEVELS],
        );
        let bytes = t.serialize();
        assert_eq!(bytes.len(), SERIALIZED_LEN);
        let Parsed(cb, scan, runs, intra, inter) = parse_serialized(&bytes).unwrap();
        assert_eq!(cb, t.codebooks);
        assert_eq!(scan, t.dct_scan);
        assert_eq!(runs, t.run_scans);
        assert_eq!(intra, t.intra_quant);
        assert_eq!(inter, t.inter_quant);
        // Synthetic contents do not carry the real checksums.
        assert!(matches!(
            BinkTables::from_serialized(&bytes),
            Err(Error::TablesMismatch(_))
        ));
        let mut bad = bytes.clone();
        bad[100] ^= 1;
        assert!(BinkTables::from_serialized(&bad).is_err());
    }

    #[test]
    fn lookup_decodes_every_code() {
        let t = BinkTables::build(
            synthetic_codebooks(),
            [0; COEFFS],
            [[0; COEFFS]; RUN_SCAN_COUNT],
            [[0; COEFFS]; QUANT_LEVELS],
            [[0; COEFFS]; QUANT_LEVELS],
        );
        for (b, codes) in t.codebooks.iter().enumerate() {
            for (slot, &(code, len)) in codes.iter().enumerate() {
                assert_eq!(t.lookup[b][code as usize], (slot as u8, len));
            }
        }
    }
}
