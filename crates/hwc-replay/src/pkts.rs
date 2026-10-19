//! Packet recordings (`.pkts`): the lockstep command stream of a game (`NetCheck.c`).
//!
//! Every game the original runs in lockstep mode (multiplayer, a game being recorded from the
//! in-game menu, or a packet playback) advances only when a *sync packet* arrives. A sync
//! packet carries every player command issued since the previous one, and each one is followed
//! by two universe updates of 1/16 s. The recorder appends every sync packet, exactly as
//! received, to a `.pkts` file, so replaying the file re-runs the simulation (and only the
//! simulation: no UI input is involved).
//!
//! # File layout (little-endian)
//!
//! ```text
//! record*   magic u32, size u32, size bytes
//!           magic "HEAD" (0x44414548): the game settings (CaptainGameInfo, 0x2b0 bytes in
//!                                     Cataclysm); only at the start of /packetRecord files
//!           magic "PACK" (0x4b434150): one sync packet
//! ```
//!
//! # Sync packet (Cataclysm)
//!
//! ```text
//! +0x00 type u16 (0x5555)    +0x02 from u16           +0x04 frame u32 (packet number)
//! +0x08 checksums: packetnum u16, randcheck u16, univcheck f32, statcheck u32, cdet u32
//! +0x18 numberOfCommands u16 +0x1a pad u16
//! +0x1c commands: type u16 (low byte command, high byte issuing player), then a body whose
//!       length depends on the type (see [`command_len`])
//! ```
//!
//! The checksums are the ones the captain computed when it processed packet `packetnum`
//! (normally the latest it had processed when it built this one); `randcheck == 0` means
//! "none yet". They are checked against the receiver's own values for that packet number, so
//! a recording carries the original's sync checksums for free: see [`crate::checksum`].

use crate::bytes::{le_u16, le_u32, put_u32, FormatError, Reader};

/// Record magic: settings header.
pub const MAGIC_HEADER: u32 = 0x4441_4548;
/// Record magic: sync packet.
pub const MAGIC_PACKET: u32 = 0x4b43_4150;
/// Size of Cataclysm's game settings block.
pub const GAME_INFO_SIZE: usize = 0x2b0;
/// Sync packet type.
pub const PACKETTYPE_SYNC: u16 = 0x5555;
/// Sync packet header size, commands follow.
pub const SYNC_HEADER_SIZE: usize = 0x1c;
/// Universe updates run per sync packet while a game is running.
pub const UNIV_UPDATES_PER_PACKET: u32 = 2;

/// One record of a `.pkts` file.
#[derive(Debug, Clone, PartialEq)]
pub enum Record {
    /// Game settings (`tpGameCreated`).
    Header(Vec<u8>),
    /// A sync packet as received.
    Packet(Vec<u8>),
}

/// A parsed packet recording.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Recording {
    pub records: Vec<Record>,
}

impl Recording {
    pub fn parse(data: &[u8]) -> Result<Recording, FormatError> {
        let mut r = Reader::new(data);
        let mut records = Vec::new();
        while !r.is_empty() {
            let at = r.pos();
            let magic = r.u32("record magic")?;
            let size = r.u32("record size")? as usize;
            let body = r.bytes(size, "record body")?.to_vec();
            records.push(match magic {
                MAGIC_HEADER => Record::Header(body),
                MAGIC_PACKET => Record::Packet(body),
                m => return Err(FormatError::new(at, format!("bad packet recording magic {m:#010x}"))),
            });
        }
        Ok(Recording { records })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for rec in &self.records {
            let (magic, body) = match rec {
                Record::Header(b) => (MAGIC_HEADER, b),
                Record::Packet(b) => (MAGIC_PACKET, b),
            };
            put_u32(&mut out, magic);
            put_u32(&mut out, body.len() as u32);
            out.extend_from_slice(body);
        }
        out
    }

    /// The settings block, if the file has one (playback reads only a leading header).
    pub fn game_info(&self) -> Option<GameInfo<'_>> {
        match self.records.first() {
            Some(Record::Header(b)) if b.len() == GAME_INFO_SIZE => Some(GameInfo(b)),
            _ => None,
        }
    }

    /// Raw sync packets in order (playback skips any header records between them).
    pub fn packets(&self) -> impl Iterator<Item = &[u8]> {
        self.records.iter().filter_map(|r| match r {
            Record::Packet(p) => Some(p.as_slice()),
            Record::Header(_) => None,
        })
    }

    /// All sync packets decoded.
    pub fn sync_packets(&self) -> Result<Vec<SyncPacket>, FormatError> {
        self.packets().map(SyncPacket::parse).collect()
    }
}

/// Number of players and recording player from a `/packetRecord` file name
/// (`...<numPlayers>_<curPlayer>.pkts`), the way playback derives them.
pub fn players_from_file_name(name: &str) -> Option<(u8, u8)> {
    let lower = name.to_ascii_lowercase();
    let at = lower.find(".pkts")?;
    let b = name.as_bytes();
    if at < 3 || b[at - 2] != b'_' {
        return None;
    }
    let digit = |c: u8| c.is_ascii_digit().then(|| c - b'0');
    Some((digit(b[at - 3])?, digit(b[at - 1])?))
}

/// View of the game settings block (`CaptainGameInfo`).
#[derive(Debug, Clone, Copy)]
pub struct GameInfo<'a>(pub &'a [u8]);

impl GameInfo<'_> {
    fn ascii(&self, at: usize, len: usize) -> String {
        let s = &self.0[at..at + len];
        let end = s.iter().position(|&c| c == 0).unwrap_or(len);
        String::from_utf8_lossy(&s[..end]).into_owned()
    }

    fn wide(&self, at: usize, chars: usize) -> String {
        let units: Vec<u16> = (0..chars).map(|i| le_u16(self.0, at + 2 * i)).take_while(|&u| u != 0).collect();
        String::from_utf16_lossy(&units)
    }

    pub fn name(&self) -> String {
        self.wide(0x00, 32)
    }
    /// Map file name; used as a `sprintf` format with the player count.
    pub fn map_name(&self) -> String {
        self.ascii(0x68, 32)
    }
    pub fn display_map_name(&self) -> String {
        self.ascii(0x88, 32)
    }
    pub fn num_computers(&self) -> u8 {
        self.0[0xac]
    }
    pub fn starting_fleet(&self) -> u8 {
        self.0[0xad]
    }
    pub fn bounty_size(&self) -> u8 {
        self.0[0xae]
    }
    pub fn starting_resources(&self) -> u8 {
        self.0[0xaf]
    }
    pub fn resource_injection_interval(&self) -> u32 {
        le_u32(self.0, 0xb0)
    }
    pub fn resource_injection_amount(&self) -> u32 {
        le_u32(self.0, 0xb4)
    }
    pub fn resource_lump_sum_time(&self) -> u32 {
        le_u32(self.0, 0xb8)
    }
    pub fn resource_lump_sum_amount(&self) -> u32 {
        le_u32(self.0, 0xbc)
    }
    pub fn ai_difficulty(&self) -> u8 {
        self.0[0xc0]
    }
    /// `MG_*` option bits.
    pub fn flags(&self) -> u16 {
        le_u16(self.0, 0xc4)
    }
    pub fn num_players(&self) -> u16 {
        le_u16(self.0, 0xc6)
    }
}

/// Sync checksums carried in a packet header.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyncChecksums {
    /// Packet number these checksums belong to (16 bits).
    pub packet_num: u16,
    /// Low 16 bits of the `RAN_Game` value drawn when that packet was processed; 0 = none.
    pub rand_check: u16,
    /// Universe checksum (sum of live ship positions), see [`crate::checksum::univ_checksum`].
    pub univ_check: f32,
    /// Player statistics checksum (`statsGetStatChecksum`).
    pub stat_check: u32,
    /// A value fixed at game start (`CDET` in the sync log).
    pub cdet: u32,
}

impl SyncChecksums {
    pub fn is_valid(&self) -> bool {
        self.rand_check != 0
    }
}

/// A decoded sync packet.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncPacket {
    pub packet_type: u16,
    pub from: u16,
    pub frame: u32,
    pub checksums: SyncChecksums,
    /// The two bytes after the command count (structure padding).
    pub pad: u16,
    pub commands: Vec<Command>,
    /// Bytes after the last command (normally none).
    pub trailing: Vec<u8>,
}

/// One command of a sync packet.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    /// Command type (low byte of the type word), see [`command_name`].
    pub kind: u8,
    /// Issuing player (high byte of the type word).
    pub player: u8,
    /// The body after the type word.
    pub body: Vec<u8>,
}

impl SyncPacket {
    pub fn parse(p: &[u8]) -> Result<SyncPacket, FormatError> {
        let mut r = Reader::new(p);
        let packet_type = r.u16("packet type")?;
        let from = r.u16("from")?;
        let frame = r.u32("frame")?;
        let checksums = SyncChecksums {
            packet_num: r.u16("checksum packet number")?,
            rand_check: r.u16("rand check")?,
            univ_check: r.f32("univ check")?,
            stat_check: r.u32("stat check")?,
            cdet: r.u32("cdet")?,
        };
        let count = r.u16("command count")?;
        let pad = r.u16("pad")?;
        let mut commands = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let at = r.pos();
            let len = command_len(&p[at..]).map_err(|m| FormatError::new(at, m))?;
            let raw = r.bytes(len, "command")?;
            commands.push(Command {
                kind: raw[0],
                player: raw[1],
                body: raw[2..].to_vec(),
            });
        }
        let trailing = p[r.pos()..].to_vec();
        Ok(SyncPacket {
            packet_type,
            from,
            frame,
            checksums,
            pad,
            commands,
            trailing,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.packet_type.to_le_bytes());
        out.extend_from_slice(&self.from.to_le_bytes());
        put_u32(&mut out, self.frame);
        let c = &self.checksums;
        out.extend_from_slice(&c.packet_num.to_le_bytes());
        out.extend_from_slice(&c.rand_check.to_le_bytes());
        out.extend_from_slice(&c.univ_check.to_le_bytes());
        put_u32(&mut out, c.stat_check);
        put_u32(&mut out, c.cdet);
        out.extend_from_slice(&(self.commands.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.pad.to_le_bytes());
        for cmd in &self.commands {
            out.push(cmd.kind);
            out.push(cmd.player);
            out.extend_from_slice(&cmd.body);
        }
        out.extend_from_slice(&self.trailing);
        out
    }
}

/// Command type names (HW1 names where the type exists there).
pub fn command_name(kind: u8) -> &'static str {
    match kind {
        1 => "Move",
        2 => "Attack",
        3 => "Formation",
        4 => "Dock",
        5 => "CreateShip",
        6 => "CollectResource",
        7 => "Protect",
        8 => "BuildShip",
        9 => "Special",
        10 => "Misc",
        11 => "AutoLaunch",
        12 => "LaunchMultiple",
        13 => "AllianceInfo",
        14 => "RUTransfer",
        15 => "MpHyperspace",
        16 => "GodSync",
        17 => "PlayerDropped",
        18 => "DeterministicBuild",
        19 => "ResearchInfo",
        0x14..=0x1b => "Cataclysm",
        _ => "Unknown",
    }
}

/// Byte length (type word included) of the command at the start of `c`, as the original's
/// packet walker (`clProcessSyncPacket`) advances over it.
///
/// Types the original does not handle (0, `GodSync`, anything above 0x1b) advance by the type
/// word alone. For types 0x15, 0x16 and 0x18 the original advances by the selection size
/// *after* removing ships that no longer exist, which mis-walks the rest of the packet when a
/// selected ship has died; this function returns the length as stored (what was written).
pub fn command_len(c: &[u8]) -> Result<usize, String> {
    let need = |n: usize| -> Result<(), String> {
        if c.len() < n {
            Err(format!("truncated command: need {n} bytes, have {}", c.len()))
        } else {
            Ok(())
        }
    };
    need(2)?;
    let u16_at = |at: usize| -> Result<usize, String> {
        need(at + 2)?;
        Ok(le_u16(c, at) as usize)
    };
    let len = match c[0] {
        1 | 15 => 0x108 + 2 * u16_at(0x106)?,
        2 | 9 | 0x19 | 0x1a => {
            let targets = u16_at(2)?;
            6 + 4 * targets + 2 * u16_at(4 + 4 * targets)?
        }
        3 | 4 | 0x18 => 8 + 2 * u16_at(6)?,
        5 | 8 | 13 | 14 | 17 | 0x1b => 10,
        6 | 12 => 6 + 2 * u16_at(4)?,
        7 => {
            let protect = u16_at(2)?;
            6 + 2 * (protect + u16_at(4 + 2 * protect)?)
        }
        10 => 12 + 2 * u16_at(8)?,
        11 | 19 => 6,
        18 => 14,
        0x14 => 0x16,
        0x15 => 0x10 + 2 * u16_at(0xe)?,
        0x16 => 0x14 + 2 * u16_at(0x12)?,
        0x17 => 0x12,
        _ => 2,
    };
    need(len)?;
    Ok(len)
}

impl Command {
    fn word(&self, at: usize) -> u16 {
        le_u16(&self.body, at - 2)
    }

    fn id_list(&self, count_at: usize, width: usize) -> Vec<u32> {
        let n = self.word(count_at) as usize;
        (0..n)
            .map(|i| {
                let at = count_at + 2 + width * i - 2;
                if width == 4 {
                    le_u32(&self.body, at)
                } else {
                    le_u16(&self.body, at) as u32
                }
            })
            .collect()
    }

    /// Ship numbers of the ships the command applies to, where the layout is known.
    pub fn ships(&self) -> Option<Vec<u16>> {
        let at = match self.kind {
            1 | 15 => 0x106,
            2 | 9 | 0x19 | 0x1a => 4 + 4 * self.word(2) as usize,
            3 | 4 | 0x18 => 6,
            6 | 12 => 4,
            7 => 4 + 2 * self.word(2) as usize,
            10 => 8,
            0x15 => 0xe,
            0x16 => 0x12,
            _ => return None,
        };
        Some(self.id_list(at, 2).into_iter().map(|v| v as u16).collect())
    }

    /// Target ids (high 16 bits: object kind, low 16 bits: number) of attack-like commands,
    /// or the protected ships of a `Protect`.
    pub fn targets(&self) -> Option<Vec<u32>> {
        match self.kind {
            2 | 9 | 0x19 | 0x1a => Some(self.id_list(2, 4)),
            7 => Some(self.id_list(2, 2)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(kind: u8, player: u8, body: Vec<u8>) -> Command {
        Command { kind, player, body }
    }

    fn sel(ids: &[u16]) -> Vec<u8> {
        let mut v = (ids.len() as u16).to_le_bytes().to_vec();
        for id in ids {
            v.extend_from_slice(&id.to_le_bytes());
        }
        v
    }

    fn sample_packet() -> SyncPacket {
        let mut mv = vec![0u8; 0x104];
        mv.extend(sel(&[3, 4, 5]));
        let mut attack = vec![2, 0];
        attack.extend_from_slice(&0x0000_0007u32.to_le_bytes());
        attack.extend_from_slice(&0x0002_0001u32.to_le_bytes());
        attack.extend(sel(&[9]));
        let mut misc = vec![0x10, 0, 0, 0, 2, 0];
        misc.extend(sel(&[1, 2]));
        misc.extend([0, 0]);
        SyncPacket {
            packet_type: PACKETTYPE_SYNC,
            from: 0,
            frame: 41,
            checksums: SyncChecksums {
                packet_num: 40,
                rand_check: 0x1234,
                univ_check: 1234.5,
                stat_check: 99,
                cdet: 7,
            },
            pad: 0,
            commands: vec![
                cmd(1, 0, mv),
                cmd(2, 1, attack),
                cmd(10, 0, misc),
                cmd(5, 2, vec![1, 0, 2, 0, 3, 0, 4, 0]),
                cmd(16, 0, vec![]),
            ],
            trailing: vec![],
        }
    }

    #[test]
    fn sync_packet_round_trip() {
        let p = sample_packet();
        let b = p.to_bytes();
        let back = SyncPacket::parse(&b).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.commands[0].ships(), Some(vec![3, 4, 5]));
        assert_eq!(back.commands[1].targets(), Some(vec![7, 0x0002_0001]));
        assert_eq!(back.commands[1].ships(), Some(vec![9]));
        assert_eq!(back.commands[2].ships(), Some(vec![1, 2]));
        assert_eq!(command_name(back.commands[3].kind), "CreateShip");
    }

    #[test]
    fn recording_round_trip() {
        let mut info = vec![0u8; GAME_INFO_SIZE];
        info[0x68..0x68 + 9].copy_from_slice(b"Garden%d\0");
        info[0xac] = 2;
        info[0xc6] = 3;
        let rec = Recording {
            records: vec![
                Record::Header(info),
                Record::Packet(sample_packet().to_bytes()),
                Record::Packet(sample_packet().to_bytes()),
            ],
        };
        let b = rec.to_bytes();
        let back = Recording::parse(&b).unwrap();
        assert_eq!(back, rec);
        let gi = back.game_info().unwrap();
        assert_eq!(gi.map_name(), "Garden%d");
        assert_eq!(gi.num_computers(), 2);
        assert_eq!(gi.num_players(), 3);
        assert_eq!(back.sync_packets().unwrap().len(), 2);
        let mut bad = b.clone();
        bad[0] = 0;
        assert!(Recording::parse(&bad).is_err());
        assert!(Recording::parse(&b[..b.len() - 1]).is_err());
    }

    #[test]
    fn file_name_players() {
        assert_eq!(players_from_file_name("Garden3_1.pkts"), Some((3, 1)));
        assert_eq!(players_from_file_name("x.pkts"), None);
    }

    #[test]
    fn truncated_command_is_an_error() {
        let mut b = sample_packet().to_bytes();
        b.truncate(SYNC_HEADER_SIZE + 0x100);
        assert!(SyncPacket::parse(&b).is_err());
    }
}
