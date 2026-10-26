# Single-player missions on the simulation (`hwc_sim` level, mission, kas_host)

A campaign mission runs headless on the simulation: the level is loaded as the original
loads it, the player's fleet arrives from hyperspace, and the mission's KAS script (extracted
from the player's `cataclysm.exe` or the imported `.kasb`) drives the computer player's teams
through `hwc_kas::Interpreter` with the runtime functions implemented on the universe.
Code: `crates/hwc-sim/src/level.rs`, `mission.rs`, `kas_host.rs`; runner
`crates/hwc-sim/examples/mission.rs`; tests `crates/hwc-sim/tests/mission.rs`.

```text
cargo run --release -p hwc-sim --example mission -- <data> 1 --ticks 4000 [--no-calls]
    [--speech SECONDS] [--nislet SECONDS] [--skill N] [--every N]
```

`<data>` is an imported data directory, an install (the script is then extracted from
`cataclysm.exe`) or a `.big`. The log lists level loading, single-player state changes, NISlet
skips, the hyperspace arrival, every FSM creation and state change, objectives and (unless
`--no-calls`) every KAS runtime call with its arguments and result.

## Level loading (`level.rs`, `LevelLoad.c`)

`load_level` is `levelStartNext` (0x49f2e0), the single-player level path:

1. MissionMan detection (`; MissionMan` on the first line): positions in the sphere files are
   absolute and the sphere's position and rotation are ignored (all shipped levels).
2. `SetSMMissionTweaks`: the sensors-manager keywords anywhere in the file;
   `smUniverseSizeX/Y/Z` + 5000 become the world bounds (0x888c58).
3. The `.mission` lines in file order: `MissionSphere` (`scriptSetMissionSphereCB`); `Lighting`,
   `Background`, `LensFlare`, `Fog` are recorded only. In single player every sphere's
   contents belong to the computer player (player 1); a sphere for a real player sets its RUs.
4. Each sphere file line by line with Cataclysm's table (0x879908): `Ships`, `AIPoint`,
   `AIPath`, `AIBox`, `AISphere`, `Resources`, `MeteorStorm`, `Mine`, `Slipgate`, `Derelict`.

`Ships` (`scriptSetShipsCB` 0x49ca50): an optional leading label names a KAS team; the team is
created at the end of the computer player's team list the first time the label appears
(`aitCreate(ScriptTeam)`), so the team list order - which is the order `kasExecute` runs teams
and `MsgSendAll` sends - is the order of first appearance in the files. `kasOrigShipsCount` adds
up the line counts and `kasOrigShipsType` is the last line's type. `numShipsUpper` raises the
count by `(upper - n) * spFleetModifier + 0.5` when the fleet modifier is not 0 (it is 0 in
mission 1, so lines like `FleetIntel ... sRecon,0,...,1` create no ship). Ships of a group are
created at the same point, each `2 * collspheresize` higher in z, yawed by the line's angle
(`univRotateObjYaw`, about the ship's own X axis), coloured by `colorScheme(n)` (clamped to 7),
and given `clFormation` when the line names a formation (`StrToTypeOfFormation`; unknown names
such as `NULL_FORMATION` mean none). sMothership, bMothership, kCarrier, tCarrier and
trFloatingCity become their player's mothership. Ship numbers therefore follow the file order.
An sMothership created in a single-player game gets its mission modules
(`univMissionModuleTypesGet`: six in missions 1-2, five in 3, three later) created right after
it and clamped to it.

`AIPoint`/`AIPath`/`AIBox`/`AISphere` define the hashed KAS labels (`kasNameHash`): points keep
their fourth value (an angle), paths their open/closed flag and points by index, boxes are
stored as extents (`x - w/2 .. w/2 + x`), spheres as centre and half the diameter.

`Resources` (`scriptSetResourcesCB` 0x49be10) builds the resource volume, reads its `.dist`
file (`GetResourceDistribution`: `Asteroid0-4`, `DustCloud0-3`, `GasCloud0-1`, two `uword`
columns, running sums) and creates its objects with the game random stream (stream 10) exactly
as the original draws it: three numbers per position (sphere, cylinder, rectangle, or the
shell variant), one for the type, and for asteroids of type 3 and up one for the spin chance
(`ASTEROID_ROTATE_PROB`, 210/255 in `resource.script`) plus three for the spin. Volumes with
the regrow attribute are kept. The objects themselves are records in `LevelState` (the
simulation has no asteroid objects yet), but the random stream is left where the original
leaves it. `Derelict` lines are records too (type, position, pitch and yaw orientation,
attributes, colour scheme).

After loading, `clPresetShipsToPosition` puts every formation's ships at their slots
(`setFormationToDo`: the leader's collision centre plus the slot offset in the leader's or the
locked formation's axes; sphere formations without rotation).

## Single-player flow (`mission.rs`)

The original path into mission 1 is `singlePlayerStartGame` -> the opening NIS (`nis\n01`,
level `Mission01_NIS01`) -> `singlePlayerNisStoppedCB` (the first player ship of class
Mothership becomes the player's mothership; the state is set to 6) -> `singlePlayerLoadNewLevel`
(non-capital player ships are put inside the mothership, capital ships and modules go into the
hyperspace list, every other ship is deleted, the universe is reset keeping the game clock, the
fleet modifier is computed from `BaseFleetStrengthForLevel`, the level is loaded with
`levelStartNext`, then `clPresetShipsToPosition`).

`MissionRun::start` does the same without playing the NIS: the fleet is what `nis\n01.script`
keeps at the end (`remainAtEnd` on Somtaaw ship paths: the mothership, three recons, two
workers) plus the mothership's mission modules and, in mission 1, the docking bay the NIS adds;
recons and workers travel docked in the mothership.

`MissionRun::step` is one `univUpdate` via `Universe::update_with`; the mission's work runs in
the original's slots:

* `singlePlayerGameUpdate` (after the collision blobs): `G_PlayNis` / `G_PlayNislet` requests
  (the variable is destroyed when seen; the NIS is not played and `G_Nis<n>Complete` /
  `G_Nislet<n>Complete` is set, at once or after `--nislet` seconds, and logged); state 6
  starts the KAS mission (`kasMissionStart`) and moves to 7; state 9 runs the arrival.
* `aiplayerUpdateAll` (after bullet collisions): `kasExecute` when
  `univUpdateCounter & 15 == 0` (`AIPLAYER_UPDATE_RATE[AI_ADV]`, `SP_KAS_UPDATE_FRAME`), skipped
  while the state is neither 0 nor 7 (i.e. during the arrival).
* `PauseUniverse` stops the command layer and ship movement (`universe_paused`; time runs on).

Arrival (`StartLevelByHyperspacingShips` sets state 9; `UpdateArrivingShip` 0x513340 per ship
each frame, `singleplayer.script` timings): the ship is put at `StartPointPlayer` plus its
offset from the mothership (y negated, turned by the point's angle) with the default
orientation yawed by that angle and hidden; it waits `2 * HYPERSPACE_BASE_WAITTIME` (mothership
and its modules) or `3 * BASE_WAITTIME + HYPERSPACE_RANDOM_WAITTIME * random(stream 10)`;
then it is shown, stopped, and the hyperspace effect runs (`hsStart`, `hsUpdate`: pop-up, slice
and collapse with `HYPERSPACE_SLICE_RATE` and the `HS_CLIPT_*` scalars; ships in the slice
phase wait while another is still popping out, the mothership collapses last) for at least
`HYPERSPACE_DECELERATE_TIME`. When every ship is in, `G_ShipsHaveArrived` is set, the state
returns to 0 and the docked ships are launched (`clAutoLaunch`). In mission 1 this happens at
13.75 s.

Mission 1 timeline (no speech timing): KAS mission starts at update 1, the NISlet request is
skipped at update 2, `StartLevelByHyperspacingShips` and the team FSMs at update 16 (the first
KAS tick), arrival complete at update 220, the intel sequence then advances one message per
`IntelEventEnded` (each waits out `SUBMESSAGE_SAFETY_TIMEOUT`, 25 s, because no subtitle
reports its end), and the first objective (`DestroyBombers`, "Destroy Taiidani Bombers") is
created at update 3040. With `--speech 3` it is created at update 832.

## KAS runtime on the simulation (`kas_host.rs`)

`SimHost` implements `hwc_kas::KasRuntime`/`KasHost` over the universe and the mission state.
Teams are the level's teams, registered with the interpreter in team-list order before the
mission starts. Selections are the original grow-selections: named selections are created on
first use, team lists are written in place (`THISTEAMSHIPS`), removals swap the last ship into
the hole (`growSelectRemoveShip`, `clRemoveTargetFromSelection`), `ShipsAdd` skips duplicates.
Ships that die leave every selection and team. Positions of selections and teams are the
centre of their bounding box (`selCentrePointComputeGeneral`); distances use
`aiuFindDistanceSquared`'s order (z first, kept in the FPU).

Implemented from the `kasf*` functions (Mission01 uses all of these): `Attack`, `MoveTo`,
`PatrolPath` (closed paths loop), `Stop`, `FormationDelta/Broad/Delta3D/Claw/Wall`,
`DockInstant`, `Launch`, `TeamDockedReadyForLaunch`, `TeamFinishedLaunching`,
`ShipsCreateLaunch`, `ReinforceTeamWithShips`, `TeamSwitchPlayerOwner`, `ShipsClear/Count/Add/
Remove`, `FirstShip`, `FindShipsInside`, `FindShipsNearPoint`, `FindEnemiesNearby`,
`ShipsSelectEnemy/Friendly/Player/Race/Type/Nearby`, `ShipsSort` (the original's exchange
sort), `GetPlayerMothership`, `IsIdle`, `HasShipBeenDiscovered`, `TeamCount`,
`TeamCountOriginal`, `TeamHealthAverage` (rounded with the 1.5*2^52 trick), `TeamHealthSet`,
`ShipsDamageModifierSet/Clear`, `ShipsRepairModifierSet`, `ShipsSetInvincible`,
`ShipsSetColor`, `HideShips`/`UnhideShips` (with the "hidden twice" bit), `RUsSet`,
`GetWorldResources`, `MissionSkillGet`, `MissionFailed`, `StartLevelByHyperspacingShips`,
`ObjectiveCreate/Destroy/DestroyAll`, `SpeechEvent`, `IntelEventEnded/NotEnded`,
`LocationCard`, `SensorsIsOpen`, `OpenSensors`, `CloseSensors`, `SetSensorsLookAtPoint`,
`PauseUniverse`/`UnpauseUniverse`; recorded only (state kept for a UI): `MusicPlay`,
`TechSet`, `TechSetResearch`, `EnableShipConstruction`, `SaveLevel`, wide screen, camera,
pointers, pings, tutorial flags, `DeleteShipsWhenMissionEnds`. Variables, timers, messages,
`Random` (stream 6), `ThisTeamIs` and the KAS pause are the interpreter's. Anything else falls
through to `unhandled`, which logs the call (`Unimplemented`) and returns 0.


## Campaign: from one mission to the next (`campaign.rs`)

The original keeps the ships that jump as ship objects in `ShipsInHyperspace` while
`singlePlayerLoadNewLevel` (0x515da0) deletes everything else, resets the universe (keeping the
game clock and the player structure: RUs, technology, research topics and labs; `suPlayerInit`
clears the upgrade state that `suPlayerUpdateFromResearch` rebuilds) and loads the next level.
Here the end of the jump gives a `Carry` (`MissionRun::carry`): the hyperspace list in order with
what is docked inside each ship (type, offset from the mothership along its heading/right/up
axes, module flag, colour scheme, tactics, attributes), the player, the clock and the random
streams. `MissionRun::start_campaign` starts mission N+1 from it: `spWipeUnavailableModulesInHyperspace`
drops the mission modules the new mission does not have (built modules stay; mission 4 loses the
docking bay), the fleet modifier is computed from the carried ships and RUs, the level is loaded,
and the carried ships are created in hyperspace (out of the universe lists) so that the mission's
start finds the player's mothership. `Carry::to_text`/`from_text` is the file form
(`hwc-viewer --carry`), `Carry::warp` the `warp.script` fleet for starting at any mission.

The jump (`singlePlayerGameUpdate` states 2-5, `HyperspaceRollCallBegin`): allowed once the mission
creates its "Hyperspace" objective (`objectiveAndFleetIntelligenceCreate` sets 0xaaf94c) or with the
debug override (the original's C key, `MissionRun::force_hyperspace`), not while a ship is under
construction. Missions 1, 2, 5, 7, 15, 17 (table 0x88a35c) have no roll call: after 5 s every ship
that can dock docks instantly (`clDock` purpose 9) and the capital ships and modules go straight
into hyperspace. The others order the small ships to dock and wait for the roll call
(`CalculateRollCall`, `OrientHyperspacingShips`: capital ships fly to within `HYPERSPACE_RANGE` and
turn to the mothership's heading; `hyperspace_go_now`, `emergency_hyperspace` are the two buttons),
then 2.5 s, then each ship goes through its window (`UpdateHyperspacingShip`). State 5 is the
whiteout and the between-missions movie (not played), then ships marked by
`DeleteShipsWhenMissionEnds` are wiped. Arrival: missions with table 0x88a3a8 set (4, 5, 6, ...)
fade the whole fleet in at the start point and put docked ships outside at once
(`spLaunchDockedShipsOnArrival`); the others use `UpdateArrivingShip` per ship and then
`clAutoLaunch` with the auto-launch the player had.

Mid-level hyperspace (`TeamHyperspaceOut/In/InNear`, `ShipsHyperspaceOut/In/InDefault`,
`ShipsCreateHyperspace`, `HyperspaceDelay`): `spHyperspaceSelectionOut/In` and
`UpdateMidLevelHyperspacingShips`; ships marked to start in hyperspace (attribute 0x80) go at once.

Headless: `cargo run --release -p hwc-sim --example campaign -- <install> [--from N] [--to M]
[--budget S] [--carry-dir DIR] [--log]`; the scripted test orders are `hwc_sim::autoplay`.
Viewer: `hwc-viewer <install> --mission N [--carry FILE] [--save-carry FILE] [--autoplay
[--budget S]]` (J is the hyperspace button / roll call "go now").

Known differences: carried ships get new ship numbers when the level loads; the movies are not
played.

## Guard, team moves, slipgates, parade, scuttle (`command.rs`, `aimove.rs`, `parade.rs`)

* **Protect orders** (`clProtect` 0x4469c0, `ChangeOrderToProtect`, `clWrapProtect`): a null order
  with the protecting attribute and a protect list (`CommandToDo::protect`). `clProcess` runs
  `protectShip` / `protectShipsAvg` (Cataclysm's versions: guard distance
  `PROTECT_GET_THIS_CLOSE_CAP/STRIKE` + both sizes, keep pace above `PROTECT_STILL_VELOCITY`,
  resource controllers follow behind) and the formation variant; `clPostProcess` (right after
  `clProcess`) sends the attack-capable guards, less the non-combat types, at whatever hits a
  protected ship, keeping the guard order. Circular guard chains set special flag 0x40000000.
  Dead ships leave protect lists; an empty list ends the order. KAS `Guard`
  (`kasfGuardShips`: deletes the team's AI moves, `makeShipsControllable` on the team list in
  place), `ShipsGuardShips`, `IsGuarding`, `ShipsSelectGuarding` read and write them. The player's
  protect order is `command::cl_wrap_protect`.
* **AI team moves** (`aimove.rs`, `kas_host/moves.rs`): `MoveTo` and `Intercept` create moves
  (`aimCreateMoveTeam`, `aimCreateIntercept`) that the computer player's pass (`aitExecute`, at
  `univUpdateCounter & 15 == 1`, not while an NIS plays) carries out: `aimProcessMove`
  (`aiuWrapMove`, finished within 1300 of the destination or when the first ship has nowhere to
  go), `aimProcessIntercept` (a move to the target or halfway to where it is going, re-aimed every
  2 s when it moved off, done within 4 km of the team's first ship: `AIM_INTERCEPT_FINISH_DISTSQ`
  16e6, `RECALC` 2e6), next-move tactics and formation, removal. Direct orders (`Attack`, `Guard`,
  `Stop`, docking, launching, patrols, kamikaze) delete the team's moves first. `ThisTeamIdle` /
  `IsTeamIdle` are `aitTeamIsIdle` (no current move, or a done / get-ships move).
* **Slipgates** (`scriptSetSlipgateCB` 0x49c680, `ships/slipgate.rs`): `Slipgate x,y,z,n/m,angle,type`
  lines create the gate ships (sphere's player, yawed, not targetable or bumpable, disable reason
  6), paired in file order. A ship docking with purpose slipgate opens both gates
  (`SlipgateOpen`: opening 2.25 s, open; closing 0.625 s, 4 s after the last use), waits 1.5 s,
  goes inside the partner gate and launches from it (`ShipDocksAtShip` states 0x10, 0xd, 10,
  0x11, 0xe). Guards and parade followers of a ship that enters are sent after it
  (`SlipgateDockShip`). `FindSlipgate`, `SlipgateDock`, `EnteringSlipgate` work on them.
* **Mines and meteors in selections**: `FindMinesInside`, `FindMinesNearTeam` and `FindMeteors`
  fill their selections (mine missiles and meteor-storm asteroids under ids of the ranges
  `ShipId::MINE_RANGE` / `METEOR_RANGE`); selection positions, counts and
  `HasShipBeenDiscovered` see them; gone ones leave the selections before each KAS pass.
* **Military parade** (`parade.rs`, `Parade_*.script` tables, `GetShipSlot` 0x481230,
  `CreateMilitaryGroupAroundShip`, `processMilitaryParadeToDo` 0x480980, `setMilitaryParade`):
  `order::MILITARY_PARADE` commands; the jump (both kinds), the go-now / emergency buttons and the
  fade-in arrival group the capital ships around the mothership and `clPresetShipsToPosition`
  snaps them to their slots, so carried offsets are parade slots. KAS `ForceParadeFormation`
  and `ShipsSetFormation(.., 8)` use it.
* **Scuttle** (`clScuttle` 0x445bf0): the self-destruct timer (+0x2fc, now + `SCUTTLE_SHIPDEATHTIMEMIN`
  .. `MAX`, stream 10); `univCheckShipState` blows the ship up when it runs out (spent gravity
  wells too).
* **NIS levels** (`nis_level.rs`): `NisLevelRun::start` loads `MissionNN_NISMM` for a mission that
  has one (table 0x88a3f4, number 0x88a440: before missions 1, 4, 6, 7, 10, 12, 14-17) with the
  carried fleet in hyperspace and the `singlePlayerStartNis` placement (`StartPointPlayer` and
  angle, `G_NisCentreDerelict` / `G_NisCentreInstance`); `finish` loads the mission (clock carried)
  and sets `G_Nis<NN>Complete`.
* Ships are created neutral (`univCreateShip` sets `tacticstype` +0x2b8 to 1); they were evasive.
* `MissionCompleted` sets `Game::mission_completed`; the roll call queues its speech events
  (`campaign::SPEECH_ROLLCALL_DOCK/COMPLETE`, `SPEECH_HYPERSPACE_ABORTED`) in `game.presentation`;
  `campaign::abort_hyperspace` is the cancel button, `campaign::rollcall_go_now_disabled` says
  which jump button the original greys out (+0x35a / +0x35b: recently attacked / fired upon).

## Scripted player (`autoplay.rs`)

Per-mission plans read the objectives and give a player's orders: mission 1 (build workers,
ramming frigates and Acolytes, strike in groups of 8 at the bombers, resourcers, proximity
attackers and the cruiser, recons to the unexplored region) and mission 2 (recons search the
quadrants, Engineering module, research repair and salvage, the fleet guards the Bushan-Re while
workers repair it, upgraded workers salvage the pod, the fleet clears the raiders) complete for
real and jump without the override (`tests/campaign.rs`, `missions_1_and_2_complete_for_real`);
mission 4 (hangar module, ten Acolytes guarding the Clee-San, the infection research) completes
for real from the `warp.script` fleet (`mission4_from_warp_completes_for_real`), and so does
mission 3 (`mission3_from_warp_completes_for_real`: recons find the Clee-San, the Mimic goes to it,
the mothership comes within 6 km so that it docks with `ShipsDockStay`; the mission waited for
`TeamDockedReadyForLaunch`, which only knew the instant-dock helpers until it was made the
original's test: every team ship hidden inside a host and not waiting for a soft launch, or staying
docked at a carrier's refuelling point). The pirate missile corvettes' missiles that hit the
Clee-San come from passive retaliation (`univCheckShipState` -> `getEnemiesWithinProximity` ->
`FUN_00524d50` -> `clPassiveAttack`): moving ships are idle in the original's sense, so a neutral
corvette flying back to its patrol point fires at any visible enemy within its
`passiveRetaliateZone` (5 km), and the KAS `Attack` lists do not restrict that; the original does
the same. Mines may home on it once `ShipsEffectedByMines(.., 0)` clears flag 0x20 (bit set =
ignored by mines, as in the binary).

KAS docking and launching go through the dock code (`dock.rs`): `ShipsCreateLaunch` creates
the ships, puts them inside the host (`univRemoveShipFromOutside`, `dockPutShipInside`) and orders
them to launch (`aiuWrapDock` purpose launch), so they come out through the host's launch points
one after another; `ShipsLaunch` likewise; `Launch` and the team docks (`Dock`, `DockSupport[With]`,
`DockStay`, `DockStayMothership`, `DockInstant`) are AI moves (`aimCreateLaunch`, `aimCreateDock`;
`aimProcessLaunch`, `aimProcessDock`, `TEAM_DOCKING` for `TeamDocking`) carried out by the next
computer-player pass; `ShipsDockInstant`, `SlipgateDockInstant` put ships inside at once;
`TeamFinishedLaunching`/`ShipsFinishedLaunching` test `specialFlags & 0x1000000` and the universe
list. Ships wiped inside a dying host leave KAS teams and selections.

The computer player has an AI player in single player (`Player.aiPlayer`, +0x1c): its harvesters
find resources anywhere (`rescollectFindNearestResource`; mission 1's resource collectors now
harvest), and the auto-behaviours of computer ships run. The effects are attached when a mission
loads (`MissionConfig::effects`, off with `HWC_NO_EFFECTS`): a dead ship stays a hulk until its
death effect deletes it, so a run without effects diverges at the first death; with them, headless
and drawn runs (with a camera) give the same world (`tests/render_lockstep.rs`).

Single-player research gating (`rmUpdateTechList`): `research::available_topics` lists what the
research manager offers: not known, not being researched, its lab present, and allowed by the
mission (`TechSetResearch` = `AllowPlayerToResearch`; the flags carry over between missions and
in the carry-over file). Formations ordered to attack close on the target in formation until it is
within 10 km and the leader's gun range (`attack_in_range`, +0x27a). A leader whose tactics hold the
formation during battle (`holdFormationDuringBattle`: neutral and aggressive, not evasive) then
attacks in formation, as `processAttackToDoInFormation` (0x43ea80) does: every ship gets a target
(the wingmen are put on the leader's when it is not itself under a command, or, with aggressive
tactics, take it when they have none; sphere wingmen re-pick the closest target every 32 updates
when theirs is out of gun range), and `formGroups` splits the formation into fighters, heavy
defenders, corvettes and the rest, each attacking through `delegateCommand` (0x43e270) on a copy
of the command whose selection is the group. The group leader makes its own attack runs
(`CustShipAttack`); for fighters, its flip turns and barrel rolls (`tacticsTalk`) lock the
formation's axes (`lockFormation`, the heading reversed for a turn) and are flown by the wingmen
too, and when the turn ends the formation unlocks with its slots mirrored left to right (and
`flipState` toggled); the wingmen fire passively only at targets ahead of the group leader,
holding their heading on them, and fly their slots (tight; loose while a wingman is still in the
leader's manoeuvre). Heavy defenders and capital ships hold loose slots and fire passively;
corvettes are tight while their leader turns or comes back in, and a minelayer never leads its
group and breaks formation to lay its mines. Spheres hold their own shape (`enders`). When the
targets are gone, strike craft fly the holding pattern (`processHoldingPattern`: legs of
`TW_HOLDING_PATTERN_PATHTIME` seconds, turning right, the wingmen in their slots) and other ships
stop; the formation is kept. Evasive leaders attack as loose ships (`processAttackToDo`) and the
formation re-forms when the attack ends. Not modelled: kamikaze attacks, the guard checks during
the attack, and the attack routines' range argument; sphere formations are not flown by this
slice.

Status with the scripted player (headless, `--speech 3`): missions 1 (fresh start), 4 and 6 (from
the `warp.script` fleet) complete for real. Mission 3 from the warp fleet now usually fails: the
Clee-San, open to mines for ten seconds after it is found (`ShipsEffectedByMines(.., 0)`, then 1),
is killed by mines that arrive at that moment. Mission 5 fails: the Bentusi's explosion kills its
attackers before the scuttle message reaches their teams, so the teams re-create six Beast
Acolytes each from the carrier (36, launched one by one), and with the bombers and corvettes that
follow they beat the scripted fleet at the Caal-Shto. Mission 2 from mission 1's carry-over fails
when mission 1 ends with one worker and no RUs. The full chain (`real_campaign_chain_mission1_to_mission6`)
is `#[ignore]`d.

The viewer and the headless runs differ in speech timing only: headless runs end intel events
after `--speech` seconds (3 by default), the viewer after the real speech/subtitle length. Mission
2 from a weak mission-1 carry-over is short of RUs (1005 RUs, three workers), and with some
timings the Engineering module build ran out of RUs; the viewer's RU readout showed the mission's
`RUsSet` mirror (`game.resource_units`) instead of the player's live RUs. The readout now shows
`base.players[0].resource_units`, and the mission 2 plan builds more workers and one Acolyte at a
time.

## Known differences from the original

* NIS and NISlets are not played (the completion variables are set; `--nislet` delays them).
* The player's fleet is recreated when it comes out of hyperspace, so it gets new ship numbers
  after the level's ships; in the original the ships keep the numbers they got in the NIS and
  the level's numbering restarts at 0 (duplicates in the original's ID table).
* The fleet's recons and workers that travelled docked in mission 1's opening are put beside the
  mothership at once when the fleet arrives (`launch_new_ship`), not launched through its doors.
* Player attack orders on mines are not supported (`cl_attack` keeps ships only); mines are set
  off by flying a ship into them.
* `UpgradeShips` does nothing (no research upgrades yet); `PatrolPath` is issued at once instead
  of through the AI move layer.
* `HasShipBeenDiscovered` uses the human player's ships' `fowRadius` instead of the fog-of-war
  grid; enemy selection treats every ship as visible (no cloaking, no Mimic disguise).
* `IntelEventEnded` only ends by the safety timeout unless `--speech` simulates the subtitle;
  real speech/subtitle lengths are not known headless. In the game the front end reports the
  end (`subMessageEnded`) when any subtitle region it draws runs out of cards, a location card
  included (Cataclysm's `subTitlesDraw`; HW1 checked the letterbox region only), and the
  safety timeout also waits for no card to be on screen (`subAnyCardsOnScreen`,
  `Game::subtitles_on_screen`). Each end and its reason is in `Game::intel_ends`; the game
  traces the presentation with `HWC_INTEL_TRACE=1`.
* Derelicts and meteor storms are records, not simulation objects; `GetWorldResources` sums the
  static resource values of the recorded asteroids and dust clouds.
* `HasShipBeenDiscovered` uses the player's ships' `fowRadius`; the original asks the local
  player's fog-of-war grid (`levelFogOfWarCreate` runs in `singlePlayerLoadNewLevel`), which
  the mission run does not create.
* Mothership modules are kept at the mothership's position and orientation and out of physics
  and bumps (not through the clamp code); the computer player's AI (`aifFleetCommand`), research,
  construction and unit caps do not run.
* Single-player and KAS hooks run inside `Universe::update_with`; the universe update itself
  still lacks the build manager, research, missiles and effects (see `runtime.md`).

## Next steps

* Carry the player's fleet over between missions (the hyperspace list with offsets,
  `CalculateHyperspaceTimeForShip`, roll call, `singlePlayerGameUpdate` states 2-5 and 10) and
  move ship creation, docking, launching and clamping onto the base layer (`base.rs`,
  `dock.rs`, `clamp.rs`) as it lands.
* Implement the AI move layer (`aimCreate*`) for `MoveTo`, `PatrolPath`, `Guard`, `Launch`,
  `DockInstant`, and the remaining Tutorial1 functions (tutorial UI predicates, `ShipsCreate`,
  `TeamHyperspaceIn/Out`, `SelectContainsShipTypes`, `ObjectiveSet/Get`).
* Viewer: `MissionRun` exposes `universe` and `step()`; the sandbox (`apps/hwc-viewer`,
  `sandbox.rs`) holds a `harness::Run`, so a `--mission N` mode needs the sandbox to step either
  a `Run` or a `MissionRun` (e.g. an enum over both); not done here because the viewer is being
  changed by another worker.
