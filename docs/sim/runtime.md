# Run-time simulation (`hwc_sim` universe, movement slice)

The first running part of the simulation: ships exist in a universe, take move and formation
orders, and fly them with the original's AI and physics at 16 Hz. Code: `crates/hwc-sim/src/`
(`object`, `universe`, `physics`, `blobs`, `aitrack`, `aiship`, `command`, `formation`,
`tactics`, `tunables`, `harness`). Everything is ported from Cataclysm's build of the
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

The emulator needs the `fmathSqrtDouble` table (built by the game at start-up) written to
0x8d3ff4; the scripts that drive the emulation are not part of the repository.

## Universe update

`Universe::update` is one `univUpdate(1/16)`: time and counter, collision blobs (rebuilt on
frame 1 and when `counter & REFRESH_COLLBLOB_RATE == REFRESH_COLLBLOB_FRAME`, refreshed
otherwise), `clProcess`, then `univUpdateAllPosVelShips` (per ship in ship-list order: the
auto-stabilisation part of `univCheckShipState`, `physUpdateObjPosVelShip`, post-physics
housekeeping). The viewer drives it through the task-scheduler model (240 Hz ticks, universe
task every 15 ticks, at most 8 runs per frame).

Cataclysm-specific behaviour reproduced: the world-bounds velocity clamp (skipped in
single-player), the speed multiplier from engine upgrades and damage (0x50fb10), per-ship speed
caps (+0x5c4) in fly-to and formation travel speed, small ships not avoiding small targets,
collision distances measured from the collision centre, move orders with waypoint lists and
modes (once, loop, patrol), groups split into one move command per ship with shifted
waypoints, the formation origin being the leader's collision centre.

## Not yet modelled / known differences

- Only ships exist; missiles, resources, derelicts, bullets and effects have lists but no
  objects. No collisions (bump/damage), combat, docking, building, research, tactics
  (passive attack, retaliation), fog of war, sensors, KAS.
- Right of way: a ship with right of way skips allied ships that are not in its command, as
  the original does, but `rowSignalShipToGetOutOfWayOfMe` (telling them to move aside) is not
  implemented, so they stay put.
- Not modelled in the fly-to code: salvager, clamped, cloaked and slaved-ship exceptions; the
  Sentinel force field; the docking dock-point lists; military parades.
- Formations: sphere and the Sentinel force-field walls are not implemented; per-type formation
  filters (static +0x190) are not consulted. `processFormationToDo`, `clFormation`,
  `formationArrageCrazyOptimum` follow the binary; only `FormationCalculateOffsets` is verified.
- `univCheckShipState`: only auto-stabilisation (not the world-limits recall, the death timer,
  or proximity retaliation). Housekeeping skips damage effects, trails and animations.
- `FPTAN` (pitch while turning for frigates and up, two tweak setters) uses the `f64` tangent;
  results can differ in the last bit in rare cases. The emulator cannot run `FSIN`/`FPTAN`, so
  rotation and the capital-ship pitch path are verified only through the `hwc-math` trig tests.
- The Beast mothership damage modifier and the Ramming Frigate ram bonus/push state are 0.
- Blob itemising for the sensors manager and the attack flags of blobs are not computed.
- `collUpdateCollRectangle` (world-space selection boxes) is not run.
- Objects created between blob rebuilds are appended to the nearest blob's lists (the
  original inserts them in order).
- The render list is a simple distance filter (`Universe::update_render_list`).

## Determinism harness

`harness::Scenario` (spawns plus orders by tick), `harness::Run` and `harness::world_hash`
(FNV-1a over the exact bits of every ship's state and the command list). Tests
(`tests/runtime.rs`, need `HWC_EXTRACTED_DIR` or `HWC_GAME_DIR`): two runs of each built-in
scenario hash identically for 600 ticks, a cloned universe continues identically, and golden
bit patterns of ship positions after 400/800 ticks (regression values of this implementation,
not taken from the game).
