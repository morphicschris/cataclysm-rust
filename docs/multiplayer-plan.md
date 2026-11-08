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

Done when every shipped map (the 10 `multiplayermissions.script` lists) loads for each player count it
allows, with any mix of Somtaaw and Beast, and a headless game runs to a winner under scripted orders.

Status: done in `hwc_sim::skirmish` (setup), `hwc_sim::multiplayer` (rules) and `hwc_sim::crates`,
tested by `crates/hwc-sim/tests/skirmish.rs`:

- The map list (`spTitleListLoad`: the ten maps the script lists; LiveFireExercise2 is commented
  out) and the four game types of `gametypes.script`.
- `gameStart` / `levelInit` for a multiplayer game: players and races (a skirmish's computer
  players alternate races), the start-position shuffle, unit caps for the player count
  (`UnitLimitCaps<n>.script`), `scriptSetMissionSphereCB`'s multiplayer path (positions, starting
  RUs, the level's own objects owned by the player after the last), `scriptSetShipsCB`'s race swap
  and start fleets in the parade, the Hive Frigate of "capture capital ship", backup motherships,
  starting resources (computer players get five times as much with harvesting on) and research
  (everything known when research is off).
- While it runs: resource injections and the lump sum, players losing (`univKillOtherPlayersIfDead`,
  "last mothership", `DisableAllPlayersShips`: the loser's ships careen and blow up), the winner and
  the end of the game (`CheckPlayerWin`, `DeclarePlayerWin`, allied victory in network games), and
  crates (placement, expiry, ships, technologies or RUs).
- Every map loads for every player count, Somtaaw first and Beast first; a game with a scripted
  strike force runs to a winner.

- Bounties during the game (`calculatePlayerBounties` every 128 updates, `bountyShipWasKilled`;
  save version 13).
- Hyperspace jumps (`MG_Hyperspace`, `hwc_sim::mp_hyperspace`): the `hyperspace` cost lines of
  `Tweak.script` (`hyperspaceCost`, `makeSelectionHyperspaceCapable`), `clMpHyperspace` /
  `ChangeOrderToHyperSpace` (command order 10) and `processMpHyperspaceingToDo` (stop, out through
  the window, at least `TW_MULTIPLAYER_HYPERSPACE_WAIT_IN_ETHER_TIME` in the ether, in at the
  destination), run on the campaign's mid-level jump code; `Order::MpHyperspace` over the network;
  in the game the Hyperspace menu item / key opens the plate (hyperspace colour) in the sensors
  manager. Save version 14. Not modelled: the sensors pings of the ships going and coming and the
  arrival chatter; the cost is posted as a message, not drawn on the plate.

- Fuel burn: nothing to port. Cataclysm keeps the option (`mgFuelBurnEnabled` sets bit 0x400) but no
  code tests the bit, nothing burns fuel and `OUT_OF_FUEL_FRICTION` is never read; ships' fuel
  (+0x334) is only filled in (`InitStatShipInfo`, hyperspace, `refuelRepairShips`) and read by KAS.

Left for later stages: alliances being formed and broken, and drawing crates (the
simulation has them; the view does not yet).

## 2. Skirmish front end

The original's screens behind *Skirmish* (`mgSkirmish`, the game-options screens and their basic,
advanced and resource tabs, player colours, the map list with each map's description), in the
original look and the modern one, and a game started from them that plays like a mission: the same
managers, sensors and orders, the multiplayer hyperspace, the end-of-game screen.

Status: the original-look screens work (`hwc_ui::skirmish`, test `hwc-ui/tests/skirmish_screens.rs`):
Connection Method's Skirmish, the Basic, Advanced and Resource tabs, CPU players and sliders, the
player name, the scenario list with each map's description, and Player Setup's race buttons. Start
Skirmish loads the map and plays it as a script-less mission (`MissionRun::from_skirmish`); losing,
winning and the end of the game show and return to the menu (`campaign_shots::skirmish_plays`).
Still to do: the modern look of these screens, drawing crates, the end-of-game statistics screen,
and saving a skirmish (refused for now). The computer players do nothing until stage 3.

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

Status (first part): `hwc_sim::orders` (every order the interface gives as data, `apply` the
one place it changes the universe, and `sync_checksum`) and the `hwc-net` crate: lockstep turns
of two updates with a delay of three turns and per-packet checksums (`lockstep`), framed
messages (`wire`), a host and clients over TCP with the host relaying (`tcp`, `session`), the
lobby with slots, races and computer players (`lobby`), the LAN game browser by UDP broadcast
(`discovery`) and players dropping out (`multiplayer::player_dropped` at the turn after their last
packet). Tests `hwc-net/tests/net.rs`: two machines with different local players stay identical
for 1500 turns of random orders and lag, a desync is reported on the turn it happens, the browser
finds a host, and a host and a client play over TCP until the client leaves. The checksum leaves
out the battle chatter's and the effects' random streams, which only the machine showing them
draws from. Next: the game giving its orders through `orders` (and the local player not fixed at
0), the LAN screens, recording games.

Second part, in the game: the game screen gives every order through `Game::give`
(`hwc_sim::orders`), applied at once offline and sent in the next packet online; the local player
is `host::player()` (set when a network game starts; the `PLAYER` constant stays 0 for the tests);
a network game's updates are paced by the turns (`net_game::NetLink`: a turn starts only when every
player's orders for it have come, "Waiting for ..." after two seconds), with no pause or time
compression, and chat, drop-outs and desyncs shown as messages. The LAN screens of
`multiplayer_game.fib` are wired (`hwc_ui::lan`, `apps/cataclysm/src/lan.rs`): Connection
Method's LAN, LAN Login, Available Games (the LAN browser's list, Ping All, Join, Create),
Create Game on the option tabs, Connecting, and the waiting rooms (Captain Wait to start or
dissolve, Player Wait to leave) with the game's players and chat; Player Setup sets the race. The
original's lobby-server rooms (Channel Chat) have no LAN equivalent; the modern look leaves them,
the ad banner and the dead WON links out. Tests: `hwc-ui/tests/lan_screens.rs` (both looks,
screenshots `screenshots/front_end/lan_*`) and `lan::tests::two_front_ends_host_find_join_and_start`
(two front ends on one machine: host, browse, join, chat, start the same game).

## 5. Internet

Direct connection by address first, then a lightweight lobby (game list and relay for players behind
NAT) in place of WON's servers. Chat in game and in the lobby.

Status: the lobby server `hwc-lobby` (`hwc_net::internet::LobbyServer`) replaces WON: log in
with a free name (no accounts), rooms with their chat and players (Lobby, and rooms players
make), the games hosted in each room, and a relay. A host that is not behind a firewall (or
lets the server find out) is checked by the server connecting back to its game port and joined
directly; otherwise joining players reach it through a pipe the server opens between them. The
original's internet screens are wired to it (Internet Login with the firewall choice, Channel
Chat, Available Rooms, Create Room, the room's games, the waiting rooms with the room's chat);
the server's address is `hwcLobbyServer` in the options or `HWC_LOBBY`, else this machine.
Tests: `hwc-net/tests/net.rs::an_internet_game_through_the_lobby_and_its_relay` (200 turns in
lockstep through the relay), `lan::tests::two_front_ends_meet_in_the_lobby_and_play_through_the_relay`,
`hwc-ui/tests/lan_screens.rs` (internet screens, both looks). Not yet: a public server, accounts,
passworded games, the recorded games.

## Not planned

Talking to the original game over the network: its protocol runs through WON, which is gone, and
every machine must run the same build for lockstep anyway.
