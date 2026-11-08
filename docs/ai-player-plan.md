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

## Status

Done (9b8a845, `hwc_sim::ai`): the player (`aiplayerInit` / `ChooseEnemies` / `GameStart` /
`UpdateAll`), fleet command, the build requests and their weighing, ships on order
(`clBuildShip`, `COMMAND_BUILDING_SHIP`), the research helpers, and the resource manager. Teams are
created and given their ships but have no moves yet (`AiTeam::order` names the `aioCreate*` order
they were given); resourcers in teams are sent harvesting directly until the moves exist.

The AI builds externally built ships (the Somtaaw mothership's modules) with `clWrapCreateShip`:
the hull launches into place and, its owner having an AI player, materialises over its build time
(`ShipDocksAtShip` → `clWrapMaterialise`, 701eaea).

## Notes on teams and moves

`aitCreate` (0x416820, `AITeam` 0x280 bytes): type (manager), `teamFlags` +4, `teamFeatures` +0xc
(level 1: 8; level 2: 0x1f), `teamDelay` +0x10 (0x740a18 by team level = player level * 2),
`newships` +0x14, owner +0x18, ships +0x1c, strength +0x24 and value +0x28 (`aiuRateShip`), team level
+0x2c, moves list +0x30 (head, tail, count), current move +0x3c, `kasTactics` +0x74 (2 in
multiplayer), `kasFormation` +0x78 (7).

`aitExecute` (0x417c00): for each team, `aieExecute` (its event handlers), then a team flagged
0x10 (to be destroyed) or whose move is type 0x2d (delete team) is destroyed; else the current move
(the first when none) runs its process function (+0x154) after `teamDelay` passes; when it returns
done, the next move becomes current, with its tactics (team feature 8) and formation (8 = same, 7 =
none), the cloak / special handling for teams flagged 0x1000000 / 0x2000000, `teamDelay` reset, and
the old move deleted when it said so (+0x166, with its close function +0x15c).

Move structure (0x170 bytes): type +0x10, process +0x154, ship-died +0x158, close +0x15c,
resource-died +0x160, processing +0x164, wait +0x165, remove +0x166, formation +0x168, tactics
+0x16c, event handlers +0xcc (0x22 words, copied into moves a move inserts before itself).

The resource manager's teams use: `aimCreateActiveResource` (0x29) → inserts a ResourceVolume move
(0x23, `aimProcessResourceVolume`) at the nearest resource blob (`aiuFindNearestResourceBlob`);
`aimCreateControlResources` (0x22: a processor goes to `aiuFindResourceControllerDestination` or
the given ships' centre, by an inserted Move (6) when further than 10 km); `aimCreateSupport`
(0x20); `aimCreateGetShips` / `FancyGetShips` (0x16: requests ships through
`aifTeamRequestsShipsCB` and waits on an AI variable); `aimCreateMoveDone` (0).

## Attack manager and teams (step 3 and 4, first part)

Done (`hwc_sim::ai::attack`, `hwc_sim::ai::team`, save version 11): teams run their moves every
turn of their player (`aitExecute`: the team delay by team level, tactics and formation handed
from move to move, moves removed when done), AI variables (`AIVar.c`), and the moves the attack
orders use: `FancyGetShips` (ships taken from the new ships or requested through
`aifTeamRequestsShipsCB`, a random alternative first, the unit caps respected; the variable set
by `aifAssignNewShips` when the last requested ship comes), `VarDec`, `TempGuard` (the team moves
out two thirds of the way across the sphere of influence towards the middle of the map, then
waits for the attack variable to go negative), `Armada`, `Attack`, `FlankAttack`, `Move` and
`MoveTo`.

The attack manager: `aiaInit` (the attack variable at 2..4, the first timed attack at 200..1000
s, the chance of each attack type by level, the features by level and race), the 24 attack slots
filled with drawn attack types while fewer than 4 teams (10 with over 12000 RUs) wait for ships
(`aiaProcessAttackTeams`), finished teams sent after nearby enemies or the enemy mothership,
`aiaDivideNewShips`, `aiaTimeAttack` (beginners attack at once; otherwise every 800..1000 s, or at
once when no RUs are left to harvest) and `aiaArmada` with `aiuFindArmadaTarget`.

The orders ported: mothership takeouts fast, guard, big and huge, and the fighter, corvette and
frigate strikes, with the exact ship lists and worth ratios of each race (the Beast's depending on
the types it has acquired). Not yet: the escort teams of the guard and huge takeouts, the target
takeouts (types 4 and 5) and hyperspace jumps. Capture, mines and the special defences
need HW1 races' ships (salvage corvettes, minelayers, cloak and gravity well generators), so a
Somtaaw or Beast player never gets them.

Reconnaissance (`hwc_sim::ai::recon`): `aiaProcessSpecialTeams` on the first turn makes a scout
team at the standard level (`aioCreateReconaissance` 0: towards a point 15000 from the enemy
mothership, then wandering the enemy blobs) and another at the advanced level (kind 2), and
`aiaAttackManager` adds kind 2 scouts while there are more than five enemy blobs per scout (up to
five). `aimProcessActiveRecon` flies five seconds towards the enemy mothership, then from blob to
blob (`aiuFindStandoffOfNextNearest(Enemy)BlobIn/Out`, `aiuGenerateRandomStandoffPoint`),
turning back and switching between enemy and all blobs by the number of enemy blobs. A scout team
that loses its ships starts over (`aihReconaissanceTeamDiedHandler`, the only event handler
ported so far, run by `aieExecute`). `aiaArmada` sends the nearest scout to look at targets the
player's ships do not see (`aiaSendRecon`, `aimProcessShipRecon`, `aimProcessIntercept`). Save
version 15. Test: `skirmish::computer_players_send_scouts`.

Harassment (`hwc_sim::ai::harass`): a harass team on the first turn by the harass chance
(`aiaProcessSpecialTeams`) and attack type 10 at the advanced level (`aioCreateHarass`): fifteen
fighters (Acolytes and stand-ins, Beast Interceptors and stand-ins) fly to a point 25000 short of
the enemy mothership, then `aimProcessHarassAttack` picks the nearest undefended, unarmed or
fighter-prey capital ship (`aiuFindHarassTarget`, the strength being Cataclysm's ship-count ratio
`statsGetRelativeFleetStrengths`), flies to 16000 short of it and attacks its defenders, or it
(`aimProcessAdvancedAttack` with `aiuAttack` for teams with feature 1). The team events of
`aieExecute` now run: getting rocked (`aihGenericGettingRockedHandler`: fight back with 1.5 times
the ships), firing and disengage (`aihHarassFiring/DisengageSingleShipHandler`: formation 5 on a
single target), health low (`aihKamikazeHealthLowHandler` for kamikaze players), numbers low
(`aihHarassNumbersLowHandler`: below 40% the team goes home and is topped up) and team died
(`aihHarassTeamDiedHandler`: made again three times in four). `aiuFindNearbyEnemyShips` and the
armada now leave out slipgates and crystals (`MakeTargetsOnlyEnemyShips`). Save version 16.
Test: `skirmish::computer_players_harass`.

Finding: the defence manager's `aiuEnemyShipsInMothershipBlob` keeps the level's slipgates (its
only filters are alliance and the crystal types) and `clAttack` does not filter targets, so in
the port a team defending a mothership that starts next to a slipgate keeps attacking it (seen on
Coldwar2); not yet checked against the original game.

Finding: `statsGetOverallKillRating` is 0 for every ship in Cataclysm. The game ships HW1's
`statfile.bin` (192,892 bytes, 83 types); `statsLoadFightStats` wants 0x41a70 bytes (98 types),
warns and clears the table. So the blob ratings have no strength, a visible enemy mothership's
blob always scores infinite and the armada always goes for the enemy mothership when it sees it.

Test: `skirmish::computer_players_attack` (research off, beginner and hard): both computer players
form attack teams that get their ships and attack the other.

## Defence manager (step 5, first part)

Done (`hwc_sim::ai::defense`): `aidInit` (features by level: none, 0x131, 0x377),
`aidClearDistressSignal` every turn, `aidDefenseManager` (finished defence teams give their ships
back), `aidCheckSphereOfInfluence` (the sphere's radius, and at the advanced level the enemy ships
leading into it become distress calls and threats, which a temporary guard attacks when they are
capital ships), `aidSetupResourceDefenseTeams` with `aioCreateGuardShips` (an escort of six,
twelve for a capital ship, of Acolytes or Interceptors or their stand-ins, in a sphere, guarding
each resourcer and processor) and `aidMothershipDefense` (the enemies in the mothership's blob or
near whoever is hitting it; dangerous new ships form the mothership's defence team, idle teams
with dangerous ships defend; with too few defenders the guards, then every team in range, join).
The moves `GuardShips`, `DefendMothership` and `Formation`, and `aimInsertMove`.

Not yet: the advanced player's roving defence (`aioCreateFast/SlowRovingDefense`, patrol moves),
recalling guard teams to the sphere, resourcers' distress calls, the HW1 races' cloak defence.

Test: `skirmish::a_computer_player_defends_its_mothership` (escorts for the resourcers, and
frigates sent at the mothership are attacked). `a_scripted_game_runs_to_a_winner` now has two
human players, since a computer player defends its mothership.
