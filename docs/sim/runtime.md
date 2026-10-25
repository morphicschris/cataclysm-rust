# Run-time simulation (`hwc_sim` universe: movement, combat and base building)

Ships exist in a universe, take move, formation, attack and halt orders, fly them with the
original's AI and physics at 16 Hz, bump into each other, fire guns, hit, damage and destroy
each other, and retaliate according to their tactics. Code: `crates/hwc-sim/src/` (`object`,
`universe`, `physics`, `blobs`, `aitrack`, `aiship`, `command`, `formation`, `tactics`,
`tunables`, `harness`; combat: `collision`, `gun`, `bullet`, `damage`, `attack`,
`flightman`, `combat_statics`, `combat_tweaks`). Everything is ported from Cataclysm's build of the
functions (`cataclysm.exe`), with the x87 evaluation order read from the disassembly: which
products are rounded to `float` and which stay in a register, the order of sums, and the NaN
behaviour of each comparison. All math goes through `hwc-math` (vectors, matrices, the table
square root, x87 `FSIN`/`FCOS`, the CRT `qsort`).

## Verified against the binary

Ghidra p-code emulation of the original functions on constructed inputs, compared bit for bit
(unit tests in each module):

| Function | Address | Cases |
| --- | --- | --- |
| `physUpdateObjPosVelShip` | 0x4dfdf0 | 12 (position, velocity, rotation speed; collision position where no `FSIN` runs, the emulator lacks it) |
| `aitrackVelocityVectorGuidance` | 0x419470 | 10 (force) |
| `aitrackHeadingFunc` | 0x418df0 | 10 (torque, rotation speed, result) |
| `aitrackZeroVelocity` | 0x4197c0 | 10 |
| `CalculateMoveToPoints` | 0x4416c0 | 6 (1..7 ships) |
| `aishipFlyToPointAvoidingObjsFunc` | 0x414620 | 10 (fly state 2 with a neighbour: avoidance, obscured destination, flag variants) |
| `FormationCalculateOffsets` | 0x47ec90 | 8 (delta, broad, delta 3D, claw, wall, with and without flip) |
| `collUpdateCollRectangle` | 0x4357a0 | 12 (all eight box corners, random orientations) |
| `collCheckRectLine` | 0x4359e0 | 48 rays against those boxes (hit distance and face; 30 hits) |
| `gunOrientGimbleGun` | 0x485c10 | 8 gimbal guns (result and barrel heading; uses `aishipGetTrajectoryWithVelPrediction`) |
| fixed-gun hit check | 0x488360 | 8 (result) |
| `dockGetDockPointType` | 0x45f310 | 672 (every class and purpose, sample types, hosts with and without small-in points and the slipgate flag, single-player mission 16) |
| `dockReserveDockPoint` | 0x45fc00 | 10 (nearest free point among 2..7 with reservations, rotated hosts; point and cone distance) |
| `cmBuildTaskFunction` | 0x450160 | 6 jobs over 5..25 frames each (multiplayer and single-player cost shift, cost divisors 1 and 2, external hulls: fraction, cost so far, RUs, hull health and `1 - left/total` progress) |
| `aishipGuideMissile` (+ `aishipFlyMissileToTarget`, `vecCapMinMaxVector`, `aitrackVelocityVectorGuidance`) | 0x415e70 | 12 (force, orientation, rotation, life counter, result; leading and non-leading paths) |
| `ShipApplyEMP` then `ShipDisableUpdate` | 0x5ae410, 0x5ae850 | 10 (flags, shock, charge count, disable stopwatch, velocity damping, re-enable; random spin and leech/ram/salvage helpers stubbed) |
| `univBulletCollidedWithTarget` + `ApplyDamageToTarget` | 0x541ac0, 0x5412f0 | 12 hits (knock-back force, spin, health, chatter bits; weapon/armour table, side modifiers, friendly fire, heavy-defender armour piercing). Speech, alliance, crystal and dock helpers were stubbed with `RET` in the emulator and the rock ranges collapsed so the random numbers do not matter. |

The emulator needs the `fmathSqrtDouble` table (built by the game at start-up) written to
0x8d3ff4; the scripts that drive the emulation are not part of the repository.

## Universe update

`Universe::update` is one `univUpdate(1/16)`: time and counter, collision blobs (rebuilt on
frame 1 and when `counter & REFRESH_COLLBLOB_RATE == REFRESH_COLLBLOB_FRAME`, refreshed
otherwise; bullets live in blobs too and are put back into their target's blob after a
rebuild), bullet/beam collisions (`collCheckAllBulletMissileCollisions`), `clProcess`, bump
collisions (`collCheckAllBumpCollisions`), bullet movement (`univUpdateAllPosVelBullets`),
then `univUpdateAllPosVelShips` (per ship in ship-list order: `univCheckShipState` with the
right-of-way manoeuvre, auto-stabilisation and the tactics reaction, `physUpdateObjPosVelShip`,
post-physics housekeeping; then the per-ship upkeep that ends bursts), and finally the dead
ships are removed (`univDeleteDeadShips`). `Universe::events` lists what happened in the
update (gun fired, bullet hit, ship died, veterancy gained) for presentation. The viewer drives it through the task-scheduler model (240 Hz ticks, universe
task every 15 ticks, at most 8 runs per frame).

Cataclysm-specific behaviour reproduced: the world-bounds velocity clamp (skipped in
single-player), the speed multiplier from engine upgrades and damage (0x50fb10), per-ship speed
caps (+0x5c4) in fly-to and formation travel speed, small ships not avoiding small targets,
collision distances measured from the collision centre, move orders with waypoint lists and
modes (once, loop, patrol), groups split into one move command per ship with shifted
waypoints, the formation origin being the leader's collision centre.

## Combat

- **Guns** (`gun.rs`): per-ship gun state from the statics, fire rates with Cataclysm's damage
  slow-down and bursts (`gunCanShoot`), aiming of fixed guns (hit check along the barrel),
  gimbal guns (cone check) and turrets (`GUN_NewGimble`, tracking rates and limits),
  `gunShootGunsAtTarget` / `...AtMultipleTargets`, line of sight for capital ships
  (`collCheckLineOfSight`), damage = weapon table (by bullet type 0..19 and armour level) x
  `DamageMult` x veterancy step (x2 in easy single-player).
- **Bullets** (`bullet.rs`): creation at the muzzle, flight, lifetime, energy-bolt homing; ion
  beams follow their gun every frame and are cut at the first box they meet (they hit every
  frame).
- **Collisions** (`collision.rs`): the oriented collision box (`collUpdateCollRectangle`, every
  fourth frame while rotating), ray/box (`collCheckRectLine`), the per-blob bullet walk with the
  binary search on `collOptimizeDist` (a hit deletes the bullet without advancing the index,
  as the original), beams across blobs, and ship bumps (big/big, small/small, small/big;
  repulsion or the velocity exchange after `COLLISION_CLEAR_TIME`, glancing blows).
- **Damage and death** (`damage.rs`): knock-back and spin from bullet mass, side modifiers,
  friendly fire, the "under attack" state, `ApplyDamageToTarget`, veterancy (kills by class ->
  level 0..4, which also raises speed), the death list with careening, the shock wave
  (`BLAST_*`: push, spin and damage falling off with distance; chains), and the removal of dead
  ships (bullets let go of them, commands and target lists drop them).
- **Orders** (`command.rs`): `clAttack` (commands that were moving resume their waypoints
  afterwards), `clPassiveAttack`, `clSetTactics`, `InitExtraAttackInfo` (attacking from above,
  fighter pairing, first targets), `pickMultipleAttackTargets`, `processAttackToDo` with the
  fighter logic (threat checks, evasive manoeuvres, sandwich and split with wingmen, wingmen
  tracking their leader), `processPassiveAttackToDo`, attack clean-up.
- **Attack routines** (`attack.rs`, chosen per type from the `CustShipHeader` table, see
  `combat_statics.rs`): `GenericInterceptorAttack` (attack runs, break-off points, turnaround
  manoeuvres, whip strafe against capital ships; Acolytes share it), `attackPassive` for
  frigates and capital ships (lie flat against fast targets), `attackSimple` /
  `attackPassiveRotate`, `attackSideStep` (corvettes), the multi-beam frigate.
- **Flight manoeuvres** (`flightman.rs`): all 23 `FLIGHTMAN_*` manoeuvres with the
  `FlightMan.script` tunables.
- **Tactics** (`tactics.rs`): `getEnemiesWithinProximity`, retaliation when hit or when enemies
  come within `passiveRetaliateZone` (evasive: move away by the sensor radius and fire back;
  neutral: fire back; aggressive: attack), capital ships firing back while moving.
- **Right of way**: ships with right of way tell idle or halted friendly ships in their way to
  move aside (`rowSignalShipToGetOutOfWayOfMe`), which fly out of the way and back
  (`rowFlyShipOutOfWay`).

## Base building

Code: `base` (shared state, `univCreateShip`, clean-up when a ship dies), `clamp`, `dock`,
`construction`, `modules`, `research`, `upgrade`, `resource`, `salvage`, `link`, `think`,
`base_tweaks`. Per-ship state is `Ship::base` (`ShipBase`), per-player and game state
`Universe::base` (`BaseState`); field docs give the original offsets.

- **Ship creation** (`base::create_ship` = `univCreateShip`): upgrades applied, dock-point
  occupancy table, ships-inside list, factory registration (motherships and carriers),
  external-build hulls starting at 0 health with `specialFlags2 & 8`, support units, the
  universe lists (skipped for ships built inside a factory) and the mothership's modules.
  `Universe::add_ship` is `base::spawn_ship` (level placement: also the type's default upgrade
  bits, the unit-cap counters, the lab of a research module and the player's mothership).
- **Clamping** (`Clamp.c`): a host keeps a list of slaves; `univUpdateAllPosVelShips` gives the
  first slave of an unclamped host its physics through `updateClampedObject` (every slave runs
  its physics step, then takes pose and velocity from the host; later slaves also run the normal
  step, as in the original); hidden ships follow without physics. Clamped pairs neither bump nor
  avoid each other.
- **Mothership modules** (`univCreateMothershipModules`): the starting modules of the game mode
  (multiplayer: upper engines, command, engineering; tutorial and missions 1..3 their own lists;
  with research off in multiplayer the seven research modules too) are ships clamped to the
  mothership. Modules built later are external-build hulls clamped where they were built. A
  module whose host is gone waits its type's delay and explodes (`ShipModuleHouseKeep`); losing
  the docking bay cancels the mothership's external jobs and refunds them (`cmFloatRefundRatio`).
- **Docking** (`Dock.c`): `dockGetDockPointType` maps class, type, external-build flag,
  capability flags, purpose and host to one of 35 point types; a mothership without a docking
  bay (or with its normal points full) uses the engineering points. `ShipDocksAtShip` states:
  0 reserve the nearest free point (or wait in range), 2 fly into the cone, 5 slide down to the
  point (following a turning host), 6/7 stand off and latch (own `center` point onto the host's
  point, `dockCalcLatchCoordSys`), 8 inside (timer +4 s with cargo, +6 s when damaged; then launch
  or finish), 9 latched (unload, then release), 10 launch from the first free launch point,
  11 external construction (clamped, disabled), 12 fly out of the cone. Resourcers unload into
  `resourceUnits` and the statistics. Retiring (purpose 7) refunds
  `buildCost * TW_RETIRE_MONEYBACK_RATIO` plus cargo and removes the ship. Launch commands go to
  the front of the command list; the per-host auto-launch bits (all on at game start,
  `clAutoLaunch`) decide whether built and docked ships launch.
- **Construction** (`ConsMgr.c`): factories with one job per type; each update the local
  player's jobs are charged in 16.16 fixed point (`buildCost << 16 / buildTime`, single player
  `<< (17 - divisor)` with the skill divisors 4/2/1) and counted down; finished ships are created
  inside the factory (`clCreateShip`) and launched. External-build types first get a hull at a
  build point (`dockHasFreeBuildDockPoint`; needs a docking bay unless it is a component or the
  factory is the Beast mothership); the hull's health follows `1 - left/total` and it is released
  (components stay clamped) when done. Support units (`supportUnits` used, `providesSU` capacity
  once finished) and unit caps (total, type, class, counting queued jobs) gate `queue_build`,
  which also checks the build-manager tables (`cmCanBuildShipType`) and research.
- **Research** (`ResearchAPI.c`): 41-entry technology sets with the ALL/disabled mode word, five
  labs (one per research module: docking bay, armour, micro, special, weapons) and the
  per-technology lab table; topics count down one second per `REFRESH_RESEARCH_RATE` frames
  (eight in the tutorial); `rmCanBuildShip` with the docking-bay rule for external hulls; the
  Beast's single lab and `rmAcquireShipTechOnDock`.
- **Upgrades** (`ShipUpgrade.c`): tech level (0..2 from Armour2/Armour3), upgrade bits from the
  25-entry technology table masked per type, the energy-gun upgrade; applied at creation, on
  launching after a dock (`UpgradeByDocking`, workers) and when an upgrade finishes.
- **Resources** (`ResCollect.c`): asteroids and dust clouds with their resource value, random
  orientation and basic physics; `clCollectResource` (one command per ship, a zone around the
  resource), `processCollectResource` (harvest until `maxresources`, then dock with purpose 3 and
  come back; `ResourceAlreadyBeingHarvested` with `harvestableByMultipleShips`, the
  not-accessible timer, out-of-world checks), `CollectResources` (per `harvestRate` mask frames,
  x5 in the tutorial; asteroid health follows its value, empty resources are deleted), the worker
  beam (`WorkerUpdateBeam`), the resource collector routine (`R1ResourcerHarvestsAsteroid`) and
  the multiplayer trickle (`regenerateRUs` per `regenerateRUrate` frames below
  `regenerateRUsIfUnder`).
- **Salvage** (`Salvage.c`): salvagers latch onto the free salvage-latch (else salvage) points of
  a target damaged enough (`twSalvageDamageLimit` by class); once every point is taken the target
  flies itself to the player's nearest depot, which keeps it for its length / `twSalTransSpeed`
  seconds (whole seconds, as stored) and converts it (`univConvertShipToPlayer`: a new ship keeps
  pose, health, cargo and upgrades). `gameStatsAddShipSalvaged` keeps its indexing quirk (it
  writes `[type + player*8]`, the getter reads `[player*145 + type]`).
- **Linking**: Acolyte + Acolyte -> ACV, Mimic + Mimic -> MCV, Beast Acolyte pair -> Beast ACV
  (`processLinkToDo`: approach below the partner, align upside down, merge; health is the mean of
  the two health fractions, kills add up); unlinking splits the ship.
- **univShipThink**: health regeneration (`repairTime`/`repairDamage`, combat values while
  attacked or attacking; repair upgrade or Beast) with the Somtaaw mothership sharing seven
  times a module's repair among the damaged modules of the highest priority group; finished
  upgrades.

Tunables: `Resource.script` and the base entries of `Tweak.script` (`base_tweaks.rs`; the
binary's `twSalvageDamageLimit[CLASS_MonsterShip]` entry writes the corvette slot and the
shipped file's `[CLASS_Composite]` line matches nothing, both reproduced).

Scenario `base` (harness and viewer sandbox): a mothership with its modules, four workers
harvesting ten asteroids, a docking bay and two recon ships queued. Tests (`tests/base.rs`):
modules and ship numbering, build and launch (time, cost, support units, dock point release),
harvest and delivery, research (no lab before the docking bay; time; then Acolytes buildable),
two Acolytes linking into an ACV, two workers salvaging a damaged derelict ion array frigate
(ownership, statistics), with determinism checks.

## Not yet modelled / known differences

- Combat: the burst attack special (heavy corvettes), the carriers' and advance support
  frigates' specials, the RKO, slipgates and the salvage corvettes' own module routines; the
  gun-opening animations (`mad`, they finish at once); alliances beyond "same player". The
  ray/triangle refinement of hit positions is skipped (the box hit is used) for bullets,
  missiles and ramming snap points.
- `NewGimble` turrets start from an identity default orientation (the mesh-bound orientation
  is not computed yet).
- Fighters that run out of targets stop instead of flying the holding pattern
  (`clHoldingPattern`); formations attack ship by ship (`processAttackToDoInFormation` and
  `holdFormationDuringBattle` are not modelled), guard/protect (`clPostProcess`) and the
  attack-move ("move while attacking") are not modelled.
- Evasive ships do not retreat to a dock when badly damaged.
- Base layer: mesh animations (MAD) complete immediately (worker grinders, latch animations;
  the link merge happens when the original's 2 s limit runs out); slipgate transit (states
  13..17) and the slipgate queue, carrier doors, the Beast and big-gun depot processes, message
  pods, the military parade launched ships join (they stay where they leave the dock), the network command
  stream (`multiPlayerGame`, deterministic build lists) and computer players. The build task runs
  for `BaseState::cur_player` only, as the original's local build manager does. Resources are not
  in the collision blobs (ships neither avoid nor bump them, bullets pass through), dust clouds
  do not change size, resources do not regrow. Unlinking is an immediate split.
- The tactics retaliation list (`tacticsShipDied`) and battle chatter are not modelled.
- Dead ships leave the ship list at the end of the update in which they die (the original
  keeps the hulk until its explosion effect ends).
- Only the parts listed in the table above are verified bit for bit against the binary; the
  AI routines follow the decompiled code with the original operation order.

- Derelicts and effects have lists but no objects. Missiles do not collide with resources or
  derelicts (they are not blob objects here); the mesh ray refinement of missile hits is skipped.
- Fog of war: the discovery message interval uses game time instead of real time; the red/blue
  message distinction (`FUN_00525100`) is not made.
- Auto behaviour: the periodic "go and get upgraded" decision is not modelled.
- Self-destruct timers (`clScuttle`, used by KAS `ShipsBeastify` for ships that cannot be
  infected) are not simulated; repair droids are presentation only (as in the original) and
  need a hull mesh from the presentation layer to land.
- Not modelled in the fly-to code: salvager, clamped and slaved-ship exceptions; the docking
  dock-point lists; military parades.
- Formations: the sphere formation is not implemented (a sentinel field sphere falls back to the
  wall, as the original does when it protects nothing); per-type formation
  filters (static +0x190) are not consulted. `processFormationToDo`, `clFormation`,
  `formationArrageCrazyOptimum` follow the binary; only `FormationCalculateOffsets` is verified.
- `univCheckShipState`: no world-limits recall or death timer. Housekeeping skips damage
  effects, trails and animations.
- `FPTAN` (pitch while turning for frigates and up, two tweak setters) uses the `f64` tangent;
  results can differ in the last bit in rare cases. The emulator cannot run `FSIN`/`FPTAN`, so
  rotation and the capital-ship pitch path are verified only through the `hwc-math` trig tests.
- Blob itemising for the sensors manager and the attack flags of blobs are not computed.
- The render list is a simple distance filter (`Universe::update_render_list`).

## Missiles and mines

Code: `missile.rs`. A missile and a mine are one object kind (`ObjRef::Missile`, `Universe::missiles`,
`missile_list`), kept in `SpaceObjList`, `ImpactableList` and the blobs (flying missiles join their
target's blob after each rebuild like bullets; mines get blobs of their own with mass 0.1; each blob
lists its missiles and mines).

* Launching (`missileShoot` 0x4860f0) from missile and mine launchers in `DefaultShipFire`,
  `GenericCapitalFire` and `gunShootGunsAtTarget` (launchers within the heading cone; Acolytes excluded):
  line-of-sight check for missiles, targets held in a salvage depot are skipped, one missile used,
  muzzle + barrel position, gun speed added to the ship's velocity, damage = weapon table (by the
  target's armour at launch) x veterancy x `DamageMult` (x KAS modifier), lifetime = `BulletLifeTime`;
  mines leave sideways (two launchers point opposite ways); wall mines take the minelayer's forced
  ranges, damage and lifetime (`ships::minelayer::forced_drop`).
* Guidance per frame (outside the universe pause, after bullets): `aishipGuideMissile` (fly at the
  target's collision centre at full speed, leading it inside 250 m above 200 m/s; drop a cloaked target;
  near small ships a chance per tactics, plus dodging/manoeuvring bonuses, to lose the target
  (`tweak*` entries of Tweak.script); point along the velocity), `aishipGuideMine` (states: leave the
  layer, drift to a stop with `MINE_STOP_FRICTION`, acquire the heaviest visible enemy in
  `MINE_RANGESQR` every 16 frames, chase it (moving into its blob) while it stays within the forced
  range, is not cloaked or disabled and not faster while running away, fly to the wall point),
  `physUpdateObjPosVelMissile`.
* Collisions per blob after its bullets: missiles then mines against the blob's small and big ships
  (binary search on blob distance, owner skipped, sentinel fields, sphere then box), missiles against
  mines (`mine_max_diameter`). A hit pushes and spins the target with the missile's mass, applies side,
  friendly-fire and "under attack" rules, then `ApplyDamageToTarget` (missiles and mines take damage
  too). Missiles that lose their target (target died) live at most 5 more seconds.
* Level mines (`Mine` keyword, `scriptSetMineCB`): derelict-race mine statics, player 1 in single
  player (7 otherwise), 5000 damage, endless life, at the raw coordinates (the original computes a
  sphere-relative position but does not use it).
* Removal at the end of the update (`DeleteDeadMissile`): blobs, formations, lists; events
  `SpecialEvent::MissileFired/MissileHit/MissileDied` for presentation (`Missile::trail_position` is the
  trail head).

## Ship modules (`CustShipHeader`)

Code: `ships/` (dispatch in `ships/mod.rs`, one file per module). `module_of(type)` gives the header;
`Ship::spec` (`ShipSpec`) holds the per-ship state, `TypeStatics` (built with the combat statics from the
ship files' custom keywords) the per-type statics. Slots called: init (`univCreateShip`), close and died
(`univDeleteDeadShip`), fire / passive attack / attack (types marked `Unsupported` in
`combat_statics::routines`), special activate/target (`clSpecial` = `ships::cl_special`,
`processSpecialToDo`), order changed (`FreeLastOrder`, `RemoveShipsFromDoingStuff`), housekeeping
(after every ship moved), remove ship references (every live ship when one is deleted, also for
docking ships via the `univShipThink` dock notification), target/effect and cascade references,
cancel, formation changed, plus `emp_shocked`.

Generic Cataclysm machinery the modules share:

* `disable.rs`: `ShipApplyEMP` (charges x `TimeShockedByEMP`, extends a running disable, Beast-infected
  ships immune, KAS-flagged ships flicker), `ShipDisableForTime`, `ShipApplyFieldSlowdown`,
  `ShipReEnable`, `ShipDisableUpdate` (EMP-shocked ships damped x0.9 per frame), stopwatches, the
  `*CRAZY*` careen spin.
* `cloak.rs`: `cloakInit/Toggle/IsCloaked/Update/GetFadeLevel/GetPowerFraction` (power 0..1000,
  recharge/drain per frame, 20-frame fades after a random delay from the effects stream; cloaking ends
  attack targets of players who cannot see the ship (`shipHasJustCloaked`).
* `visibility.rs`: per-player visibility of Mimics/MCV/P1 standard corvette, the Leech and the
  cloakers (`univShipVisibleToPlayer`, `SetVisibleToPlayer`, `InitVisibility`, `UpdateVisibility` every
  16 frames: allies see, detectors with the sensors upgrade see within their vision range;
  `suGetShipVisionRange` with the x1.3 sensors technology).
* `fow.rs`: the fog-of-war grid (10 km cells over the world, viewer counts and explored flags), only
  for the local player as in the original (`levelFogOfWarCreate`, `harness::Order::FogOfWar`); ships
  are viewers (components and swarmers excepted), moved by the physics step; discovering an enemy in
  unexplored space gives `SpecialEvent::EnemyDiscovered` (at most every 10 s); `fowShipVisibleToPlayer`.
* `autobehaviour.rs`: `univShipAutoBehaviourUpdate` (materialising ships; computer players' ships use
  specials against enemies in vision range by difficulty, and in multiplayer pick fights with the
  primary enemy / attackers / anyone by bigotry).
* `think.rs`: `univShipThink` in the original order (dock notification, Beast infection check, queued
  attack, sentinel field timer, EMP flicker, regeneration, bursts, upgrade end, `ShipDisableUpdate`,
  visibility, auto behaviour).
* `present.rs`: render flags (cloak, infection, EMP, field) for the viewer.

## Special abilities (per module)

Each file under `ships/` documents its module in detail; tests are in `tests/ships_*.rs`.

* **Acolytes** (`acolyte.rs`): interceptor attack plus gun pods that open and close after random
  delays, and two link missiles; with the upgrade a special target makes the group launch both
  missiles within 4 km. **ACV** (`acv.rs`): side-step attacker whose EMP (activate or target,
  formation members combine their charges, 2 s firing + 75 s recharge) disables ships through
  `ShipApplyEMP`; it keeps its Acolytes' missile state for unlinking.
* **Minelayer** (`minelayer.rs`): attack passes dropping mines within drop range; the special
  lays a square wall of forced mines (`NumMinesInSide` squared) that fly to their wall points;
  mine formations; mine regeneration. **Missile ships** (`missile_ships.rs`): missile destroyer
  volleys and regeneration after `MissileLagVolleyTime`, missile corvettes, the cruise missile
  (kamikaze within 5 km; with the infection upgrade it infects Beast-infectable ships).
* **Mimic, MCV, P1 standard corvette** (`mimic.rs`): disguise state machine (delay, morph out,
  hold, morph back), fooling players as a ship of another player, an asteroid (a real asteroid
  record while still) or a derelict; disguised ships hold fire and attack by ramming; an MCV
  splits into its Mimics for fighter disguises. **Leech** (`leech.rs`): latches onto `DPT_Leech`
  points and drains RUs and/or health per second, hidden unless detected, lets go on EMP.
* **Cloakers** (`cloaker.rs`, `cloak.rs`): the cloaked fighter and ion array frigates toggle
  their cloak (cloak generators have no behaviour in Cataclysm). **Proximity sensors**
  (`proximity.rs`): sense non-allied ships every 32 frames; revealing cloaked ships comes from
  the detector capability (`visibility.rs`). Targeting skips ships the player cannot see.
* **Ramming frigate** (`ramming.rs`): turn, close in, charge above top speed, snap onto the hull,
  ram damage (fighters and micro ships die), clamp and push with the cutting beam for
  `RamPushTime`, wait; Beast ships with a cascade are immune, and a rammer charging a Beast ship
  catches the infection. **Hive frigate** (`hive.rs`): six swarmers created inside, launched at
  the target, recalled, refuelled and replaced every 15 s; swarmer kills count for the hive.
  **Sentinels** (`sentinel.rs`): field triangles between the sentinels of a wall formation stop
  enemy bullets, beams and missiles and block bumps; field strength by upgrade, regeneration,
  going down when spent, suppressed for 2 s by an enemy DFG; sentinels in a field formation take
  no attack orders.
* **Dreadnought** (`dreadnought.rs`): missile regeneration, the cannon module (upgrade 0x400),
  one-second charge and a shock wave. **Motherships** (`mothership.rs`): the Somtaaw big gun
  (target point, 11 s charge, a shell with distance falloff, scatter in multiplayer), the drawn-out
  Somtaaw death, the Beast damage modes (speed, damage, research, build, fire rate; burning 20
  health a frame), death shock waves; the Beast mothership's cascade. **DFG** (`dfg.rs`): reflects
  enemy bullets in its field (percentage through, angle variance, life extension), detonates
  big-gun shells, suppresses enemy sentinel fields. **Gravity well** (`gravwell.rs`): catches
  strike craft, brakes, tumbles and repels them, collapses after its operation time. **Nomad
  moon** (`nomad.rs`): 6 s charge, push-and-fling shock wave with a field slow-down, 11 s rest,
  needs its emitters. Shock waves: `shockwave.rs`.
* **Beast** (`beast.rs`): infection (hits until `beastAttacksRequired`, stun, conversion to
  `beastAttackConvertTo` of the infecting player via `univConvertShipToPlayer`, beastified
  statistics) and the cascade of Beast heavy cruisers, carriers and motherships (charge, shoot
  the nearest special target, the beam jumps on within the cascade distance while points last).
  **Crystals** (`crystal.rs`), **processors** (`processor.rs`, up to four auto-repair beams),
  **worker repair** (`worker_repair.rs`, repair orders), **repair droids** (`repair_droid.rs`,
  presentation only).
* **Kamikaze** (`collision.rs`, the bump code's `FUN_00436470`): a kamikaze ship that bumps an
  enemy (or its target's structure) destroys itself and does its `kamikazeDamage`
  (Tactics.script) to the other ship (scaled for an upgraded disguised Mimic); ships with the
  killer-collision attribute damage each other by the smaller health
  (`ApplyDamageToCollidingObjects`).
* **KAS** (`kas_host.rs`): `SpecialToggle`, `ShipsSpecialToggle`, `SpecialOn/Off`,
  `ShipsSpecialOn/Off`, `Mimic`, `Unmimic`, `SetStayMimicked`, `ShipsBeastify`,
  `ShipsSetBeastInfectionUnlimited`, `ShipsSelectBeastified`, `GetShipCountBeastified`,
  `ShipInfectionTerminal`, `EnableBeastWeaponAutoAttack`, `IsBeastReady`, `TimesHitByEMP`,
  `IsForceFieldActive`, `IsEMPReady`, `Salvage`; the mission variable `G_NaggarokEMP` feeds
  `ShipApplyEMP`.

## Determinism harness

`harness::Scenario` (spawns plus orders by tick), `harness::Run` and `harness::world_hash`
(FNV-1a over the exact bits of every ship's state including health, attack state, guns and
manoeuvres, every bullet, the game random stream and the command list). Orders: move,
formation, halt, attack, tactics, build, collect, research, link, salvage, dock (scenarios also
place resources and set starting resource units). Scenarios: `fleet`, `idle`, `patrol`, `dogfight` (six
Acolytes against six Taiidan heavy interceptors, both aggressive) and `frigates` (three
multi-beam frigates against a Taiidan destroyer), `base` (see Base building). Tests (`tests/runtime.rs`,
`tests/combat.rs`, `tests/base.rs`, need `HWC_EXTRACTED_DIR` or `HWC_GAME_DIR`): two runs of each built-in
scenario hash identically for 600 ticks (1600 for the combat scenarios, which also check that
guns fire, hit and kill), a cloned universe continues identically, and golden bit patterns of
ship positions after 400/800 ticks (regression values of this implementation, not taken from
the game; the fleet values changed with the combat slice because the Acolytes of the delta
formation now bump each other; the base layer changed them again: the mothership's three module
ships are clamped to it and take part in blobs and avoidance, and without them the earlier values
and hash are reproduced exactly). The world hash also covers the base state (docking, clamping,
cargo, players' resources and research, factory jobs, resources), the missiles, each ship's
module state (`Ship::spec`) and the fog grids. With the ship modules the fleet hash changed again
with unchanged positions (the hive's six swarmers are new ship-table entries and the module state
is hashed); `harness::world_hash_with` leaving out the swarmers and the special state reproduces
the previous hash exactly, which the golden test checks. Extra orders: `Special`, `Mine`,
`ComputerPlayer`, `FogOfWar`.
