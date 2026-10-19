# Run-time simulation (`hwc_sim` universe: movement and combat)

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

## Not yet modelled / known differences

- Combat: missiles and mines (launchers hold fire), kamikaze, ramming, salvage, leech, DFG,
  Sentinel fields, crystals, the burst attack special, cloaking and Mimic disguise, the
  mothership big gun; the gun-opening animations (`mad`); fog of war and the detection-time
  gates of `getEnemiesWithinProximity` (everything counts as visible); alliances beyond "same
  player". The ray/triangle refinement of hit positions is skipped (the box hit is used).
- `NewGimble` turrets start from an identity default orientation (the mesh-bound orientation
  is not computed yet).
- Fighters that run out of targets stop instead of flying the holding pattern
  (`clHoldingPattern`); formations attack ship by ship (`processAttackToDoInFormation` and
  `holdFormationDuringBattle` are not modelled), guard/protect (`clPostProcess`) and the
  attack-move ("move while attacking") are not modelled.
- Evasive ships do not retreat to a dock when badly damaged (no docking yet).
- The tactics retaliation list (`tacticsShipDied`) and battle chatter are not modelled.
- Dead ships leave the ship list at the end of the update in which they die (the original
  keeps the hulk until its explosion effect ends).
- Only the parts listed in the table above are verified bit for bit against the binary; the
  AI routines follow the decompiled code with the original operation order.

- Missiles, resources, derelicts and effects have lists but no objects. No docking,
  building, research, sensors, KAS.
- Not modelled in the fly-to code: salvager, clamped, cloaked and slaved-ship exceptions; the
  Sentinel force field; the docking dock-point lists; military parades.
- Formations: sphere and the Sentinel force-field walls are not implemented; per-type formation
  filters (static +0x190) are not consulted. `processFormationToDo`, `clFormation`,
  `formationArrageCrazyOptimum` follow the binary; only `FormationCalculateOffsets` is verified.
- `univCheckShipState`: no world-limits recall or death timer. Housekeeping skips damage
  effects, trails and animations.
- `FPTAN` (pitch while turning for frigates and up, two tweak setters) uses the `f64` tangent;
  results can differ in the last bit in rare cases. The emulator cannot run `FSIN`/`FPTAN`, so
  rotation and the capital-ship pitch path are verified only through the `hwc-math` trig tests.
- The Beast mothership damage modifier and the Ramming Frigate ram bonus/push state are 0.
- Blob itemising for the sensors manager and the attack flags of blobs are not computed.
- The render list is a simple distance filter (`Universe::update_render_list`).

## Determinism harness

`harness::Scenario` (spawns plus orders by tick), `harness::Run` and `harness::world_hash`
(FNV-1a over the exact bits of every ship's state including health, attack state, guns and
manoeuvres, every bullet, the game random stream and the command list). Orders: move,
formation, halt, attack, tactics. Scenarios: `fleet`, `idle`, `patrol`, `dogfight` (six
Acolytes against six Taiidan heavy interceptors, both aggressive) and `frigates` (three
multi-beam frigates against a Taiidan destroyer). Tests (`tests/runtime.rs`,
`tests/combat.rs`, need `HWC_EXTRACTED_DIR` or `HWC_GAME_DIR`): two runs of each built-in
scenario hash identically for 600 ticks (1600 for the combat scenarios, which also check that
guns fire, hit and kill), a cloned universe continues identically, and golden bit patterns of
ship positions after 400/800 ticks (regression values of this implementation, not taken from
the game; the fleet values changed with the combat slice because the Acolytes of the delta
formation now bump each other).
