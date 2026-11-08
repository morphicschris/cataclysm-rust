# The computer player (multiplayer stage 3)

The skirmish's computer players are the original's AI player (`AIPlayer.c` and its parts in HW1,
reworked by Cataclysm for the Somtaaw and the Beast). It is the largest piece of the multiplayer
plan: about 420 functions in `cataclysm.exe` (0x401000..0x420230), against about 23,000 lines of
HW1 source.

## What is there

| Part | Cataclysm | HW1 file | In the port |
| --- | --- | --- | --- |
| Player (`aiplayerInit`, `aiplayerGameStart`, `aiplayerUpdateAll`, enemies, deaths) | 14 | `AIPlayer.c` | no |
| Fleet command (`aifFleetCommand`, hyperspace savings, the managers' turns) | 11 | `AIFleetMan.c` | no |
| Resource manager (`air*`: resourcers, RU accounting, research) | 10 | `AIResourceMan.c` | no |
| Attack manager (`aia*`: attack types, armada, harass) | 13 | `AIAttackMan.c` | no |
| Defence manager (`aid*`: guard, roving defence, distress) | 13 | `AIDefenseMan.c` | no |
| Orders (`aio*`: the team recipes) | 22 | `AIOrders.c` | no |
| Teams (`ait*`) | 44 | `AITeam.c` | the KAS teams' part (`kas_host`) |
| Moves (`aim*`) | 168 | `AIMoves.c` | the moves KAS teams use (`kas_host/moves.rs`, `aimove.rs`) |
| Events and handlers (`aie*`, `aih*`) | 35 | `AIEvents.c`, `AIHandler.c` | no |
| Utilities (`aiu*`) | 87 | `AIUtilities.c` | a few (`aiuWrap*`, distances, filters) |
| Variables (`aiv*`) | 11 | `AIVar.c` | in `hwc-kas` |

The single-player computer player is only its KAS teams (`aitExecute` from `kas_host`); the
managers above only run with fleet control, which a skirmish gives.

## Order

1. The player and fleet command: `AiPlayer` state per computer player (saved, added at the next
   save version), `aiplayerInit` / `aiplayerGameStart` / `aiplayerChooseEnemies`, and
   `aiplayerUpdateAll` calling `aifFleetCommand` every `aiplayerUpdateRate` updates in the
   universe's computer-players hook. The build requests (`RequestShips`, the queues per manager)
   go through the existing construction manager.
2. The resource manager: workers harvesting, more resourcers when RUs are low, research topics
   (Somtaaw labs, Beast acquisition), unit caps and support units.
3. Teams, orders and the moves they use (the rest of `AITeam.c`, `AIOrders.c`, `AIMoves.c`), with
   the events and handlers.
4. The attack manager: the attack types and their probabilities by difficulty and bigotry, the
   armada, harass, takeout-mothership.
5. The defence manager: guard teams, roving defence, distress.
6. Cataclysm's own: Somtaaw modules and Beast infection, Acolyte linking, Mimics, Leeches,
   Siege Cannon, crystals.

Each step is tested headlessly with CPU-against-CPU skirmishes (`hwc-sim/tests/skirmish.rs`): the
CPU harvests and builds (step 2), its teams move and fight (3, 4), and a game between computer
players runs to a winner, the same on every run (the determinism checks of `docs/parity.md`).
