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

## Notes from the first reading

`aiplayerUpdateAll` (0x411250): every update while the game runs, each computer player whose
`aiplayerUpdateRate` mask matches its player number (and who is not dead, and no NIS plays) runs
`aifFleetCommand` with `aiCurrentAIPlayer` set, then `aiplayerFrameCount += 1`.

`aifFleetCommand` (0x405ba0): allies once (`aifFindAllies`), the blob arrays and the knowledge of
enemy ships (`aiuCreateBlobArrays`, `aiuUpdateKnowledgeOfEnemyShips`), distress cleared, new ships
handed out (`aifAssignNewShips`: to the team waiting for that type, else to the resource manager for
resourcers, carriers, motherships, non-combat and support ships). On the first turn only the special
teams are processed; after it the hyperspace savings are skimmed off the RUs (feature `0x1000000`),
then the attack, defence and resource managers run. Every turn: `aitExecute`,
`aifProcessShipBuildRequests`, the blob arrays freed.

Build requests: the resource manager has one request (`aifResourceManRequestsShipsCB`: type,
number, priority, creator); the attack, defence and script managers have queues of requests and of
teams waiting for ships (`aifTeamRequestsShipsCB`, by the team's manager 0/1/3). Each turn
`aifProcessShipBuildRequests` takes the first buildable request of each queue (researching what a
type needs when a lab is free, every `aifResearchDelay[difficulty]` turns), asks for a support
module when support units run short (Cataclysm: `aifRequestSupportModule`, sMothershipSupport /
sCarrierSupport / bMothershipSupport), keeps a free Somtaaw lab researching the mothership's and the
carrier's upgrades and a Beast lab on technology 0x20, and `aifBuildRequestedShips` spends the RUs
over the four requests by each manager's share of what was spent (`BuildShip`).

The resource manager (`airResourceManager` 0x413250): the RUs left in the world (summed every
`0x743310` frames); with research on, the Somtaaw mothership's modules in turn (Docking Bay, Micro,
then Special / Weapons / Armour at random, a cooldown of 100 turns) and now and then technologies
0x1c and 6; workers (sWorker, bWorker, or type 0x58 for race 3) up to the dock points and the RUs
left, or a "dumb" random count by difficulty; a processor (sProcessor / bProcessor) when the
features allow; a carrier when support units run short; the support teams divided; distress for
resourcers under attack. Feature bits (`ResourceFeatures` at +8): 1 support carriers, 2 resourcer
teams, 4 processor teams, 8 advance-support teams, 0x10 distress, 0x200 smart resourcer count,
0x400 processors, 0x1000 evasive resourcers.
