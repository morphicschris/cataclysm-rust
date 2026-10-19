//! The original's sync log (`netlog.txt`, or the file named by `/logFilePath`).
//!
//! With `/logOn` the original appends one line per processed sync packet:
//!
//! ```text
//! Packet:<frame> Rand:<randcheck> Checksum:<univcheck %f> <live ships> CDET:<statcheck>
//! ```
//!
//! With `/logOnVerbose` it also writes, while computing the checksum, one line per live ship
//!
//! ```text
//! S:<player> <race> <type> <ship number> (<x> <y> <z>) <fuel> <tactics> <isDodging> <dodgeDir> <tacticsOrder> (<order> <attributes>)
//! ```
//!
//! plus statistics lines (`  CDET:...`, `BOUNT...`) and docking/resource detail that this
//! parser skips. The floats are printed with six decimals, so a log is a coarse oracle: it
//! catches a divergence within about 1e-6 of the coordinate, not a bit-level one. Its value is
//! that the original writes it with no modification at all.

/// A ship line of a verbose log.
#[derive(Debug, Clone, PartialEq)]
pub struct LogShip {
    pub player: i32,
    pub race: i32,
    pub ship_type: i32,
    pub ship_number: i32,
    pub position: [f64; 3],
    pub fuel: f64,
    pub tactics: i32,
    pub is_dodging: i32,
    pub dodge_dir: i32,
    pub tactics_order: i32,
    /// Current order and its attributes, -1 without a command.
    pub order: i32,
    pub attributes: i32,
}

/// One packet's entry. In the file the ship lines come *before* their packet line (they are
/// printed while the checksum is computed); this parser attaches them to the packet line that
/// follows.
#[derive(Debug, Clone, PartialEq)]
pub struct LogPacket {
    pub frame: u32,
    pub rand_check: u32,
    pub univ_check: f64,
    pub live_ships: u32,
    pub stat_check: i64,
    pub ships: Vec<LogShip>,
}

/// Parses a sync log; lines it does not know are skipped.
pub fn parse(text: &str) -> Vec<LogPacket> {
    let mut out = Vec::new();
    let mut pending = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if let Some(rest) = line.strip_prefix("S:") {
            if let Some(s) = parse_ship(rest) {
                pending.push(s);
            }
        } else if let Some(rest) = line.strip_prefix("Packet:") {
            if let Some(mut p) = parse_packet(rest) {
                p.ships = std::mem::take(&mut pending);
                out.push(p);
            }
        }
    }
    out
}

fn tokens(s: &str) -> Vec<&str> {
    s.split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .filter(|t| !t.is_empty())
        .collect()
}

fn parse_ship(rest: &str) -> Option<LogShip> {
    let t = tokens(rest);
    if t.len() < 14 {
        return None;
    }
    let i = |k: usize| t[k].parse::<i32>().ok();
    let f = |k: usize| t[k].parse::<f64>().ok();
    Some(LogShip {
        player: i(0)?,
        race: i(1)?,
        ship_type: i(2)?,
        ship_number: i(3)?,
        position: [f(4)?, f(5)?, f(6)?],
        fuel: f(7)?,
        tactics: i(8)?,
        is_dodging: i(9)?,
        dodge_dir: i(10)?,
        tactics_order: i(11)?,
        order: i(12)?,
        attributes: i(13)?,
    })
}

fn parse_packet(rest: &str) -> Option<LogPacket> {
    // "<frame> Rand:<r> Checksum:<f> <n> CDET:<c>"
    let mut parts = rest.split_whitespace();
    let frame = parts.next()?.parse().ok()?;
    let (mut rand, mut univ, mut live, mut stat) = (None, None, None, None);
    while let Some(p) = parts.next() {
        if let Some(v) = p.strip_prefix("Rand:") {
            rand = v.parse().ok();
        } else if let Some(v) = p.strip_prefix("Checksum:") {
            univ = v.parse().ok();
            live = parts.next().and_then(|v| v.parse().ok());
        } else if let Some(v) = p.strip_prefix("CDET:") {
            stat = v.parse().ok();
        }
    }
    Some(LogPacket {
        frame,
        rand_check: rand?,
        univ_check: univ?,
        live_ships: live?,
        stat_check: stat?,
        ships: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_verbose_log() {
        let text = "S:0 1 5 12 (100.500000 -2.000000 3.250000) 0.000000 1 0 0 0 (1 0)\n\
                    S:1 2 7 13 (-5.000000 0.000000 1.000000) 500.000000 0 1 2 3 (-1 -1)\n\
                    R:4 \n\
                    \x20 CDET:1 2 3 4 5 6\n\
                    Packet:41 Rand:4660 Checksum:98.750000 2 CDET:-12345\n\
                    Packet:42 Rand:1 Checksum:0.000000 0 CDET:0\n";
        let p = parse(text);
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].frame, p[0].rand_check, p[0].live_ships, p[0].stat_check), (41, 4660, 2, -12345));
        assert_eq!(p[0].univ_check, 98.75);
        assert_eq!(p[0].ships.len(), 2);
        assert_eq!(p[0].ships[1].ship_number, 13);
        assert_eq!(p[0].ships[1].position, [-5.0, 0.0, 1.0]);
        assert_eq!(p[0].ships[1].order, -1);
        assert!(p[1].ships.is_empty());
    }
}
