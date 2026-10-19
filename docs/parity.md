# Parity harness

How we check, with numbers, how close the recreation is to the original `cataclysm.exe`:
record a game in the original, run the same inputs through our engine, and compare world state
tick by tick. The code lives in `crates/hwc-replay`.

## 1. The two recorders in the original

Cataclysm ships two independent recorders, both inherited from Homeworld.

### 1.1 Demo recorder (`Demo.c`, `.dem`, header `BaNng`): UI-level input

Records **raw user-interface input**, not game commands. Reader and writer: `hwc_replay::demo`.

| Part | Contents |
|---|---|
| header | `"BaNng\0"`, version `u16` = 7, `startTime f32`, `initialStateSize i32`, then the full state of every random stream `(x, y, z, c, n)` x N. N = **27 in Cataclysm** (header 0x22c bytes), 25 in Homeworld (0x204) |
| option block | `(type u32, value)` entries: 0 dword, 1 bool, 2 real32, 3 byte, 4 bool8, `0x100+n` string, `0x10000+n` struct. Cataclysm's table (0x8b76a0): smInstantTransition, taskTimeElapsed, determCompPlayer, noDefaultComputerPlayer, CAMERA_MOUSE_SENS, a real32 at 0x74674c |
| event stream | one input snapshot at record start, then repeated `ticks i32` (written by the task scheduler before each pass, `demNumberTicksSave`) and an input snapshot (written by the once-per-frame region task, `demStateSave`) |
| snapshot | `time f32, mouseX i16, mouseY i16, frameTicks u32, mouseButtons u8, taskNumber u8, pad u8 (uninitialised), nKeys u8`, then `nKeys` x `(scan index u8, keyScanType u8)` for every key whose state byte is non-zero |

Retail builds compile out the per-snapshot checksums (`DEM_CHECKSUM`), so a demo has no
built-in sync check. Playback replaces the live mouse/keys, the tick counts, `taskNumberCalls`
and `utyNFrameTicks` with the recorded ones and restores the random streams from the header;
determinism then depends on *every* piece of front-end and UI code behaving identically.

Findings:

* The shipped `demos\abc640x480.dem` (with `autodemos.list`, `homeworld.bat` naming
  `Homeworld.exe` and `E3DEMO.dem`) is a **Homeworld** recording: 25 streams, Homeworld's
  five-entry option table. Cataclysm's player would mis-read it. Parsed in the test: 2233
  snapshots, 2232 tick words, 44957 ticks (187.3 s), byte-exact round trip.
* **Retail Cataclysm cannot record or play demos**: the `/demoRecord`, `/demoPlay` and auto-demo
  switches are gone from the option table, and nothing sets the recording/playing flags (they are
  only ever cleared). The code (`dem*` at 0x45e100-0x45e700) is dead.
* Conclusion: demos are the wrong layer for parity anyway (they would need a bit-identical UI).

### 1.2 Packet recorder (`NetCheck.c`, `.pkts`): command-layer lockstep packets

This is what deterministic replay is built on, and it is **live** in retail Cataclysm. Reader
and writer: `hwc_replay::pkts`.

* Recording paths: the in-game Esc menu "Record game" (saves a savegame to
  `SavedGames\RecordedGames\<name>` and the packets to `<name>.pkts`; in single player it
  switches the game into lockstep with a local fake captain), and `/packetRecord` in a
  multiplayer game (file `<map name formatted with player count>_<player>.pkts`, starts with a
  settings header).
* Playback paths: front-end "Load recorded game" (savegame + packets), and `/packetPlay <file>`
  at game start (player count and recording player read back from the file name).
* File: records of `magic u32, size u32, bytes`; `"HEAD"` (0x44414548) = `CaptainGameInfo`
  (0x2b0 bytes: game name, password, map name at 0x68, numComputers 0xac, resources, AI level
  0xc0, `MG_*` flags 0xc4, numPlayers 0xc6, player info...), `"PACK"` (0x4b434150) = one sync
  packet exactly as received. Any other magic is fatal in the original.
* Sync packet: `type u16 (0x5555), from u16, frame u32`, checksums
  `packetnum u16, randcheck u16, univcheck f32, statcheck u32, cdet u32`, `numberOfCommands u16`,
  pad, then commands. Each command: `u16` whose low byte is the type and high byte the issuing
  player, then a type-specific body (`hwc_replay::pkts::command_len` has the walker's length for
  all 27 types, including Cataclysm's new 0x14-0x1b and the waypoint-carrying Move /
  MpHyperspace with the selection at +0x106).

## 2. How the original replays deterministically, and what our engine must consume

In lockstep mode `universeUpdateTask` does, for each sync packet:

1. `clProcessSyncPacket`: append the packet to the recording (when recording);
   `netcheckShow`, which computes the sync checksums **before** the commands:
   `univcheck = univGetChecksum()` (sum of live ship positions: three f32 running sums, each add
   at 53-bit precision rounded to f32, result `(z + y) + x`), `statcheck = statsGetStatChecksum()`,
   and `randcheck = ranRandom(RAN_Game) & 0xffff`, which **consumes one value of stream 10 per
   packet**; then compares the checksums the packet carries against the local ones for that
   packet number; then executes every command through the command layer (`clMove`, `clAttack`,
   ...), converting ship ids to ships and dropping ships that no longer exist.
2. `netCheck()`, then `univUpdate(1/16 s)`, a yield (render), and a second `univUpdate(1/16 s)`
   while the game is running. So **one packet = two universe ticks**.

Random streams 1..26 are reset to fixed defaults at game start; stream 0 is clock-seeded and
only used outside the simulation. No UI input, render, timing or camera state enters the
simulation in this mode (that is what made multiplayer possible), so the inputs our engine must
consume are exactly:

* the start state: a Cataclysm **savegame** (in-game recordings) or the map +
  `CaptainGameInfo` (`/packetRecord`);
* the sync packets, in order, applied at packet start, each followed by two universe ticks,
  with the `RAN_Game` draw for `randcheck` before the commands.

Original quirk to reproduce: for command types 0x15, 0x16 and 0x18 the walker advances by the
selection size *after* dead ships were removed, so a packet with such a command naming a dead
ship is mis-walked from there on. `command_len` returns the stored length; a faithful replay
must advance the way the original does.

## 3. State from the original

Available without modifying anything:

* **The recording itself.** Every packet carries the captain's checksums for an earlier packet
  (`packetnum`): `univcheck` (bit-exact f32), `randcheck`, `statcheck`. Our engine recomputes the
  first two (`hwc_replay::checksum::univ_checksum`, `next_rand_check`) and compares per packet.
  This is a free, bit-exact (if coarse) oracle on every recording.
* **The sync log.** `/logOn` writes `netlog.txt` (or `/logFilePath`) with one
  `Packet:%d Rand:%d Checksum:%f %d CDET:%d` line per packet; `/logOnVerbose` adds one `S:` line
  per live ship (player, race, type, ship number, position, fuel, tactics, dodge state, order),
  plus stats lines. Parser: `hwc_replay::netlog`. Positions are printed with `%f`, so it locates
  a divergence to the ship and packet but is not bit-exact.

A full per-tick bit-exact dump of the original (positions, velocities, orientation, health, all
random streams each `univUpdate`) would need instrumentation of the running game; that was not
prototyped (see section 5). The state-record format below is ready for such a capture or for a
debugger script; the original's layout facts are: ship list head `universe.ShipList`, node
`next` at +0, ship at +0xc; ship `flags` +0x14 (`SOF_Dead` 0x800), position +0x58, velocity +0x64,
coordsys +0x98, rotspeed +0xbc, health +0x170, type +0x2b0, race +0x2b4, owner +0x32c (player
index at +0x24 of the player), fuel +0x334, ship number +0x43c; 27 random streams of 20 bytes.

## 4. State records and the comparer

`hwc_replay::state`: `"HWCSTATE"`, version 1, a header (source, stream count, ship record
length, label) and length-prefixed frames: update counter, universe time, all random streams,
and the ship list **in list order** (order is simulation state) with ship number, player, race,
type, flags, position, velocity, coordsys, rotspeed, health, fuel (92 bytes; longer records keep
the extra bytes). Readers tolerate a truncated last frame.

`hwc_replay::compare`: pairs frames by index, ships by number; integers by value, floats by bit
pattern (`-0.0 != 0.0`), no tolerance by default (`float_ulps` for exploratory runs), stream 0
ignored by default. The report gives the number of identical leading frames and the first
divergent frame with every differing field (ulp distance for floats, the previous frame's value
for context), ship-list order and membership differences.

## 5. Does the original run here?

No. Launched (minimised, from a scratch copy of the install, now deleted) it loads its imports
and then spins one core indefinitely inside the copy-protection wrapper's code without creating
a window; the game code is never reached. The protection checks for the original CD, which this
machine does not have. Producing recordings therefore needs a machine with the original CD.
I did not attempt to work around the protection, and the in-process capture idea was not
pursued.

## 6. Recommended parity workflow

1. On a machine where the original runs (CD present): start a skirmish or mission, immediately
   use Esc > Record game, play (or let CPU players fight), stop recording. Optionally run with
   `/logOnVerbose` to get `netlog.txt` as well. Keep the savegame, `.pkts` and `netlog.txt`.
2. Our engine: load the savegame (needs a Cataclysm savegame loader in hwc-sim), then for each
   sync packet: compute `univ_checksum` / draw `randcheck` from `RAN_Game`, apply the commands,
   run two universe ticks, write a `StateFrame`.
3. Compare: per packet, our checksums against the ones carried by the following packets
   (matched by `packetnum`) and against `netlog.txt` lines; the first mismatching packet
   brackets the divergence to two ticks, and the verbose log names the ship.
4. Engine against itself (and, once available, against a per-tick capture of the original):
   `compare` on two state streams for the exact field and tick.

Start with `/packetRecord`-free single-player recordings of short fights; a CPU-vs-CPU skirmish
gives long deterministic runs with no human commands at all.
