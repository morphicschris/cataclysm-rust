# Multiplayer plan

The order agreed with CrasH: skirmish against the computer first, then LAN, then internet. Each stage
ends playable and tested before the next starts.

The original is a lockstep game (see `docs/parity.md`): every machine runs the same simulation, and
only the players' orders travel, as sync packets of command-layer commands, two universe ticks per
packet. The port's simulation is already deterministic per build (`hwc-sim/tests/determinism.rs`),
so networking carries orders, not state.

## 1. Skirmish core (simulation)

What a multiplayer game sets up that a mission does not, from `cataclysm.exe`:

- The game options (`tpGameCreated`, `gametypes.script`, the `mg*` option callbacks): the `MG_*`
  flags (last mothership, research, harvesting, unit caps, fuel, allied victory, crates,
  hyperspace), starting fleet and resources, resource injections and the lump sum.
- Loading a multiplayer level (`multiplayer/<map>/<map>.level`): `scriptSetMissionSphereCB`'s
  multiplayer path, which gives each player a start sphere, swaps the race of its ships to the
  player's race and adds the starting fleet; `.dist` resources; crates.
- Per-player setup: race, colours, research table (the Somtaaw multiplayer table or the Beast's),
  unit caps, resources.
- The rules while it runs: injections and lump sums, crates, victory and defeat (`MG_LastMotherShip`,
  capture, allied victory), a player leaving.

Done when every shipped map (the 11 in `multiplayermissions.script`) loads for each player count it
allows, with any mix of Somtaaw and Beast, and a headless game runs to a winner under scripted orders.

## 2. Skirmish front end

The original's screens behind *Skirmish* (`mgSkirmish`, the game-options screens and their basic,
advanced and resource tabs, player colours, the map list with each map's description), in the
original look and the modern one, and a game started from them that plays like a mission: the same
managers, sensors and orders, the multiplayer hyperspace, the end-of-game screen.

## 3. Computer players

The original's computer player (`AIPlayer.c` and its parts, `aip* aia* aif* air* aid* ait* aiu*`:
resource management, fleet building, attack and defence teams, the `aiplayer.script` tunables, the
three difficulty levels). This is the largest stage. Headless CPU-against-CPU games give long
deterministic runs and the parity checks of `docs/parity.md`.

## 4. LAN

A new `hwc-net` crate: the lockstep transport replacing WON Titan. Orders go out as the original's
sync packets (`hwc-replay::pkts` already reads and writes them), two ticks per packet, with the
checksums each packet carries checked so a desync is found at once and reported. A host and clients
on the local network, a game browser by broadcast, joining before the game starts, and a player
dropping out. Recording a game works as the original's `.pkts` recordings do.

## 5. Internet

Direct connection by address first, then a lightweight lobby (game list and relay for players behind
NAT) in place of WON's servers. Chat in game and in the lobby.

## Not planned

Talking to the original game over the network: its protocol runs through WON, which is gone, and
every machine must run the same build for lockstep anyway.
