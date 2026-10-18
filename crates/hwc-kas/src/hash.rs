//! KAS label hashing.
//!
//! Cataclysm's kas2c replaced every KAS label (FSM and state names, variables, timers, teams,
//! selections, volumes, paths, points, messages) with a 32-bit hash. The runtime hashes the
//! label text it reads from the level files with the same function, so mission code and level
//! data meet on the hash value.

/// A hashed KAS label.
pub type Hash = u32;

/// kas2c sets this bit on the hash of a `G_` (global) variable or timer name; the runtime strips it
/// and files the variable at mission scope.
pub const GLOBAL_FLAG: Hash = 0x8000_0000;

/// The label hash: an ELF-style hash over the label bytes with each byte masked by `0xDF`, which
/// makes it case-insensitive for letters. The result is always below `0x1000_0000`.
pub fn kas_hash(name: &str) -> Hash {
    kas_hash_bytes(name.as_bytes())
}

/// [`kas_hash`] over raw bytes (labels are 8-bit text).
pub fn kas_hash_bytes(name: &[u8]) -> Hash {
    let mut h: u32 = 0;
    for &c in name {
        h = (h << 4).wrapping_add(u32::from(c & 0xDF));
        let hi = h & 0xF000_0000;
        if hi != 0 {
            h ^= hi ^ (hi >> 24);
        }
    }
    h
}

/// The hash kas2c stores for a variable or timer name: [`kas_hash`] plus [`GLOBAL_FLAG`] when the
/// name starts with `G_` (case-insensitive, like the hash itself).
pub fn var_hash(name: &str) -> Hash {
    let b = name.as_bytes();
    let global = b.len() >= 2 && (b[0] & 0xDF) == b'G' && b[1] == b'_';
    kas_hash(name) | if global { GLOBAL_FLAG } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_hashes() {
        assert_eq!(kas_hash("Mission01"), 0x087e_6ef1);
        assert_eq!(kas_hash("mission01"), 0x087e_6ef1);
        assert_eq!(kas_hash("StartTimer"), 0x079d_69c2);
        assert_eq!(kas_hash("G_RecentlyLoaded"), 0x058d_b5e4);
        assert_eq!(var_hash("G_RecentlyLoaded"), 0x858d_b5e4);
        assert_eq!(var_hash("StartTimer"), 0x079d_69c2);
        assert_eq!(kas_hash("_VIFONCE001"), 0x0443_cca1);
        assert_eq!(kas_hash(""), 0);
    }

    #[test]
    fn always_below_bit_28() {
        let long = "ThisIsAVeryLongLabelNameThatFoldsManyTimes_0123456789";
        for n in 1..long.len() {
            assert!(kas_hash(&long[..n]) < 0x1000_0000);
        }
    }
}
