# Static game data (`hwc_sim::statics`)

Everything the original builds in memory before a mission runs: ship, weapon, dock and light
statics, missiles and mines, resources, derelicts, research tables, unit caps and team colours.
Code: `crates/hwc-sim/src/statics/`. Entry point: `StaticWorld::load(&BigArchive)` (or
`StaticWorld::load_from(&dyn DataSource, LoadOptions)`, which also accepts an extracted directory
through `DirSource`). Test over the real data: `crates/hwc-sim/tests/static_world.rs`
(`HWC_GAME_DIR` or `HWC_EXTRACTED_DIR`, `--nocapture` prints a summary).

Sources: decompiles of `universeStaticInit` (0x538350), `InitStatShipInfo` (0x536b60),
`InitStatShipInfoPost`, `InitStatMissileInfo`, `InitStatMineInfo`, `InitStatDustCloudInfo`,
`InitStatNebulaInfo`, `InitStatHyperspaceGateInfo`, `precalcStaticHeader`,
`precalcStaticInfoGuidance`, `aishipPrecalcInfo`, `mexSetCollisionInfo`, `mexGet*StaticInfo`,
`scriptSetGunStatics`/`DockStatics`/`NAVLightStatics`, `lodTableReadScript`, `rmResetStaticInfo`,
`unitCapInitStatics`, `teReset`, `scriptSetTweakableGlobals`; the keyword tables were dumped from
the executable's data section (`{name, setter, offset, base}` records, e.g. the ship table at
0x8a80e8), which gives every keyword's exact spelling, conversion and field offset.

## Load order and what is global

1. **Tweaks** (`Tweaks::load`). `scriptSetTweakableGlobals` and `tacticsStartUp` read the
   startup scripts before any object file. Only the globals the static loaders use are modelled:

   | Field | Keyword (file) | Compiled default | Shipped value |
   | --- | --- | --- | --- |
   | coll damage min/max fraction | `COLLDAMAGE_DEFAULTMIN/MAX_FRACTION` (Tweak) | 0.2 / 1.0 | 0.1 / 1.0 |
   | blast radius default | `BLAST_RADIUS_MULTIPLE_DEFAULT` (Tweak) | 1.5 | 3.0 |
   | retaliate zone | `RETALIATE_ZONE` (Tweak) | 1000 | 7000 |
   | mining base hack | `MINING_BASE_VERTICAL_DIST_HACK` (Tweak) | 3400 | 0 |
   | capital side modifiers | `capitalSideModifiers[TRANS_*]` (Tweak) | 1 x6 | 1.3 1.3 1 1 0.9 1.7 |
   | ship speed modifier | `GLOBAL_SHIP_SPEED_MODIFIER` (Tactics) | 1 | 1 |
   | ship health modifier | `GLOBAL_SHIP_HEALTH_MODIFIER` (CommandLayer) | 1 | 1 |
   | strike craft padding | `STRIKECRAFT_PADDING_MODIFIER` (Formation) | 1.2 | 1.3 |
   | avoid padding (big) | `AVOID_OBJ_PADDING_SCALE(_BIG)` (AIShip) | 1.6 / 2.0 | 2.55 / 2.0 |
   | bank sines | `CORVETTE/FRIGATE/DESTROYER/MASSIVESHIP/NOSHIP_BANK` (AIShip, degrees, sine stored) | 0.707 0.5 0.35 0.17 0.01 | sin 70, 40, 30, (unchanged), 0.01 deg |
   | fighter bank | none (no table entry; `FIGHTER_BANK 85` in the file is ignored) | 0.99 | 0.99 |
   | turn / descend pitches | `*_TURNPITCH`, `*_DESCENDPITCH` (AIShip, degrees) | 30/25/20/0.01, 20/15/10/0.01 deg | 15/10/-/0.01, 20/15/-/0.01 |
   | asteroid harvest range | `ASTEROID_HARVEST_RANGE` (Resource) | 400 | 600 |
   | dust cloud sizes | `DUSTCLOUD_RADIAL_SCALE`, `DUSTCLOUD_RADII[DustCloudN]` (Cloud) | 9; 350 500 800 1300 | 1; same |
   | nebula radius | `NEB_RADIUS` (Nebulae) | 100 | 400 |
   | hyperspace gate | `HYPERSPACEGATE_HEALTH/WIDTH/HEIGHT` (SinglePlayer) | 20000/1000/1000 | 44000/1000/1000 |

   Keywords are matched exactly: `aiship.script`'s `BIGSHIP_*` lines and `FIGHTER_BANK` have no
   effect because the binary's table names them `MASSIVESHIP_*` and has no fighter entry.

2. **Default ship**: `DefaultShip\DefaultShip.lod` and `.mex`, used for `LODFile DefaultShip` /
   `pMexData DefaultShip` and as the fallback when a named mex file is missing.
3. **Ships** by race and type (`InitStatShipInfo`), then the health pass (below).
4. Asteroids (5), dust clouds (4), nebulae (1), derelicts (8, the last being the built-in
   hyperspace gate), missiles (per race), mines (per race; the derelict race's mine is always
   loaded). Then `mine_max_diameter` = 2 x the largest mine collision radius.
5. `ResearchDepend.script`, `UnitLimitCaps.script` (+ `UnitLimitCaps<n>.script` presets layered
   on it, applied by `gameStart` with the game's caps setting), `TeamColors.script`.

The original loads ship statics only for types a level needs (`IF_InfoNeeded`); each type's load
is independent, so loading all of them gives identical per-type values.

## Value conversions (`cscan`)

All setters go through `sscanf`; `cscan::scan` reproduces `%d`, `%f`, `%s`, literals and
whitespace, stopping at the first mismatch (the target keeps its old value). `%f` converts via
double then narrows. Booleans (`scriptSetBool`/`Bool8`): skip blanks and commas; true if the next
character is 1-9 or the rest is `TRUE`/`YES` (any case). Unlike HW1's documented bug, Cataclysm
compares against the characters `'1'..'9'`. `scriptSetFramesCB` stores seconds x 16;
`scriptSetReal32SqrCB` squares; `scriptSetAngCB`/`CosAngCB`/`SinAngCB` convert degrees with the
single-precision factor 0.017453292. Enum lookups (`StrToShipType`, `StrToGunType`, ...) ignore
case and return -1 (here `None`) for unknown names; keyword matching itself is case sensitive.

## StaticHeader (0xa4 bytes, all object kinds)

| Off | Field | Keyword | Notes |
| --- | --- | --- | --- |
| 0x00 | lod | `LODFile` | `.lod` relative to the object's directory |
| 0x04 | mex | `pMexData` | default ship's if missing |
| 0x08 | mass | `mass` | |
| 0x0c | moment of inertia | `momentOfInertiaX/Y/Z` | |
| 0x18 | 1/mass, 1/MOI | derived | infinite when zero, like the original |
| 0x28 | max velocity | `maxvelocity` | x `GLOBAL_SHIP_SPEED_MODIFIER` for guided objects |
| 0x2c | max rot | `maxrot` | |
| 0x30 | info flags | runtime | not modelled |
| 0x31 | immobile | `immobile` | not in the missile/mine tables |
| 0x32 | right of way | `rightOfWay` | ships default from a 145-entry table (0x8a7a80); derelicts 1; missiles/mines -1 |
| 0x34.. | collision info | see below | |
| 0x88 | precise selection | `preciseSelection` | ships default 0xff |
| 0x8c..0xa0 | box scale F/U/R, box offset X/Y/Z | `collBoxScale*`, `collBoxOffset*` | the mine table's box-scale entries point outside the mine record and are ignored |

Collision (`mexSetCollisionInfo` + `precalcStaticHeader`): box scales of 0 count as 1; `Col0`
gives the sphere: radius x the largest scale goes to approx, original and collision radius,
avoidance radius = that + 3, centre + `collBoxOffset` = sphere offset. `Rct0` gives the box
(corner, lengths; each scaled axis re-centred: x = up, y = right, z = forward); if
1.1 x half-diagonal exceeds the sphere, every sphere radius (and the padding) is set to it.
Without `Rct0` the box is a cube of the sphere's size around its centre. Diagonal = box length;
Cataclysm's new `coll_rect_centre` = (length + corner) / 2 per axis, exactly as computed. Then
the approx/avoid modifiers multiply (when nonzero) and padding = avoid x (2.0 if avoid > 1000 else
`AVOID_OBJ_PADDING_SCALE`). The `collspheresize` keyword is only in the ship table and is
overwritten whenever the mex has `Col0`.

Health (0xa4..0xb0): `maxhealth`, `minCollDamage`, `maxCollDamage`; when both damages are 0 they
default to `maxhealth` x the coll-damage fractions; `1/maxhealth` when nonzero.

Guidance (0xb4..0xec): `thruststrength[...]` x mass x speed modifier (force), `rotstrength[...]`
x the matching MOI (torque), `turnspeed[...]` (ships and missiles default 1, derelicts 0).

## ShipStaticInfo (0x458 bytes)

File: `RACE_<race>\<shiptype>.shp`. A type without a file is disabled (traders:
`trResearchStationBridge`, `trResearchStationTower`). Every field's offset, keyword and default is
in the doc comment of `ShipStaticInfo` (`ship.rs`). Summary of defaults set before the file:
turn speed 1; `formationPaddingModifier` = strike-craft modifier for the original's 27-type
strike-craft list (`ShipType::is_strike_craft`), else 1; trails style 1 and -1 sizes; precise
selection 0xff; dock light far 0.1, colour (120,194,255); hyperspace colour black, box scale 1;
`Overlay_Selection_Scalar` 1; `leechRUsPerSecond` 1, `leechHPsPerSecond` 20;
`beastAttackCascadeCost` 1, `beastAttacksRequired` -1, `beastAttackConvertTo` bWorker;
`bUseRayTriangleIntersectionForHitLocation` true; `forceFieldProtectionDistanceScale` -1;
`upgradeTimeModifier` 1; byte flags at 0x212/0x213 = 1/0 (no keyword; purpose unknown).

Derived after the file, in the original's order:
- `repairCombatDamage` = 0.2 x `repairDamage` and `repairCombatTime` = `repairTime` when unset
  (HW1 copied the damage unchanged).
- Capability bits at 0x1ec from a switch on ship type (values reproduced; bit meanings only
  partly known: 0x10000 sensors/recon/probes, 0x1000 mothership modules, 0x8012 carriers,
  0x800e motherships, 0x1 harvesters, 0xc000 slipgates, 0x20 acolytes/mimic/cloak, 0x40 ACV ...).
- `collSideModifiers` default: `capitalSideModifiers` if `isCapitalShip`, else 1.
- Single-player build costs: bHeavyCruiser 3000, bIonArrayFrigate 600, sMothershipBigGun 5000.
- `blastRadiusShockWave` and `passiveRetaliateZone` defaults (negative zone -> 0).
- `canReceiveSomething`; `multipleEngines` (`Eng1` chunk).
- Header precalc; trMiningBase sphere/box x += `MINING_BASE_VERTICAL_DIST_HACK`; health and
  guidance precalc; AI bank/pitch by class (heavy cruiser/carrier and the class-less types
  p1Mothership, trFloatingCity, trResearchStation, trGhostship, dRKO, dBeastMothership take the
  "massive ship" values).
- Engine nozzles `Eng0..3` (position; Cataclysm's attached object name kept).
- Docks, then resource nozzle (`Gun0`, harvesters = class Resource with `harvestAmount` > 0),
  repair nozzle (`Rep0`, `repairBeamCapable`), nav lights, guns.
- Bullet ranges: with guns, all three tactics get max and min gun range (seeded with gun 0);
  without guns, class Resource ships get `ASTEROID_HARVEST_RANGE`; squares follow.
- `forceFieldProtectionDistanceScale` <= 1 -> 2 (warning; most trader ships hit this).
- After all ships (`universeStaticInit` tail + `InitStatShipInfoPost`): `maxhealth` x
  `GLOBAL_SHIP_HEALTH_MODIFIER`, 0 -> 1, saved as `base_max_health` (0x44c); in single player
  Somtaaw ships get `base x single_player_health_factor` (a value the campaign sets; 1 here by
  default, `LoadOptions`).

### Guns (`GUN n { }` blocks, 0x11c bytes each)

Reader: wait for `NUMBER_OF_GUNS` (< 1 ends the scan), then `GUN n`, a line starting `{`, keyword
lines, a line starting `}`. The gun goes to slot `n`, not file order; slave driver defaults to -1.
Keywords (`gun.rs`): `Type` (aliases `GUN_HiddenGimble` = Gimble, `GUN_VisibleGimble` =
NewGimble), `GunSize`, `StartUpgradeLevel`/`EndUpgradeLevel` (`WEAPON_*`), `DamageMult`,
`MaxMissiles`, bullet length/lifetime/speed/range/mass, fire/burst times, the second fire mode
(`BulletLifeTime2`, `BulletRange2`, `BulletSpeed2`, `FireTime2`), `MinAngle`/`MaxAngle`/
`TriggerHappy` (cosines), turret angles (radians), tracking, barrel/recoil, `OffsetX/Y/Z`,
`SlaveDriver`. HW1's `SoundType`, `BulletType`, `DamageLo`, `DamageHi` only produce a warning in
Cataclysm (none of the shipped files use them), so per-tactic damage stays 0 in the statics and
the damage comes from the weapon technology at run time. Derived: tracking 0 -> 1; lifetimes =
range / speed when unset (second mode falls back to the first mode's range/speed). From the mex
(`Gun`/`GunN`): position, normalised direction ((1,0,0) if zero), and when neither MinAngle nor
MaxAngle was given, cos(edge) / cos(cone + edge) of the chunk.

### Docks and nav lights

`NUMBER_OF_DOCK_POINTS` then `DockPoint name,type,flyaway,min,max,heading,up` (commas -> blanks,
`?` -> 0). Equal heading and up become 0 and 2. Per-type index (first point, count) is built when
at least one line was read. Mex: `Dok` chunk named `Dock` or `Sal` chunk named `SALVAGE` with the
point's name: position, normalised normal, cos(cone + edge); missing -> origin, (1,0,0),
cos 90 degrees. `center` is looked up by name.

`NUMBER_OF_NAV_LIGHTS` then `NavLight name,type,on,off,delay,size,minLOD,texture` (`NULL` = no
texture). Mex `Nav` chunk named `Nav` with the light's name: position, colour, attached object.

## Other objects

- **Asteroids** (`Resources\Asteroids\AsteroidN.shp`): header (no `collspheresize`), health,
  `resourcevalue`, `harvestableByMultipleShips`; Cataclysm's `NUMBER_OF_ASTEROID_PIECES` /
  `AsteroidBreakInfo type,pos x3,rotX,rotY,rotZ,speed,spin x3`. The break orientation is
  RotX(rotX) x RotY(rotY) x RotY(rotY): the original applies the y rotation twice and never uses
  rotZ; reproduced. Asteroid 0's box is zeroed.
- **Dust clouds / nebula**: as resources plus `*CanShrink`/`*MinShrink`; box zeroed and every
  sphere radius = `DUSTCLOUD_RADIAL_SCALE x DUSTCLOUD_RADII[n]` / `NEB_RADIUS`.
- **Derelicts** (`Derelicts\<name>.shp`): header, health, guidance (turn speed not defaulted),
  `sinbank`/`pitchturn`/`pitchdescend`, `salvageable`, `scaleFactor`, `worldRender`,
  zoom/render-list fields, nav lights. The hyperspace gate is built in code (height x width x 50
  box centred on the origin, render limit 5e8). Most Cataclysm derelicts are ship types of
  `RACE_DERELICT` instead.
- **Missiles / mines** (`RACE_<race>\Missile.shp`, `Mine.shp`): header (no immobile, right of way,
  coll modifiers), health, guidance (turn speed 1, right of way -1). Mines add
  `MINE_STOP_FRICTION`, `MINE_RANGESQR(_FORCED)` (squared on read; `mine_range` = sqrt),
  `maxvelocity_FORCED`, `N-LIPS`. Missiles get trail width/height -1.

## Research (`ResearchDepend.script`)

Three tables: `SPRS*` single-player Somtaaw, `MPRS*` multiplayer Somtaaw, `MPRB*` Beast (also used
by single-player Beast players per `rmInitializeResearchStatics`). `<p>ShipDepend ship techs...`
and `<p>TechDepend tech techs...` replace the whole 42-flag entry (41 technologies); `<p>TECH_x n`
sets research times (the Beast table has no `TECH_RESEARCH_CLEESAN_ATTACK` entry). Shipped data:
13 / 14 / 12 ship dependencies, all technology graphs acyclic.

## Unit caps and team colours

`UnitLimitCaps.script`: `ShipTotalCaps`, `Cap<shiptype>` (spelled `CaptrGhostShip` for
trGhostship; derelict-race types except dAcolyte have none), `CapCLASS_<class>`; -1 disables a
cap. `TeamColors.script`: per player base, stripe, four trail colours; specular/diffuse/ambient
adjust; trail HLS factors; standard dot colours. Cataclysm dropped HW1's `TOColor` and
`BaseToTOFactors`. In multiplayer, when the local player is Beast, schemes 0 and 1 swap
(`TeamColors::swap_for_beast_player`).

## Not reproduced yet

- **Per-type `CustShipStaticInit`/`StaticInitPost`** (about 40 ship modules, see
  `recovered/naming/s0-modules.md`): the keywords they read are collected in
  `ShipStaticInfo::custom_entries` (124 distinct keywords over the shipped files, e.g. the
  per-tactic `flyPastDist`/`breakRange`/`flightman*` tables, missile regeneration, cloaking,
  gravwell, explosion types, `regenerateRUs`, `salvageFlyToDistance`, `MultiBeamRangeRatio`).
- **Mesh-dependent data**: `BindGun`/`BindAnimations`/`Mad*` bindings, gun default orientations
  and the `GF_MultiLevelMatrix` flag (`meshFindHierarchyMatrixByUserData`), per-LOD attachment
  indices of engine nozzles and nav lights (names are kept), the mesh hierarchy size.
- **Trail statics** (`trailSegments`, `trailGranularity`, `trailColor` gradients,
  `trailStaticInfoParse`).
- **Dock overrides** (`DockOveride`; unused by the shipped files).
- Runtime-only state: info flags, team colour flags at 0xf0, custom ship header function
  pointers, texture handles.
- Fight statistics / firepower ratings (`statsLoadFightStats`, `statfile.bin`) and armour levels.

## Known deviations

- x87 arithmetic: multi-term expressions are evaluated in `f64` and rounded once to `f32`;
  `cos`/`sin` use the platform's double routines rather than `fcos`/`fsin`. Results can differ
  from the original in the last bit.
- Undefined behaviour of the original is not emulated: out-of-range `GUN`/player/technology
  indices (the original writes outside its arrays), a setter whose `sscanf` fails storing pointer
  bits (`scriptSetSwordCB`, `scriptSetRGBCB`), and stale LOD fields carried over from the
  previously read `.lod` (the reader only clears some fields; every shipped `.lod` sets all of
  them, so this has no effect on shipped data).
- A missing `.lod` gives `lod = None` with a warning (the original would fail).

## Data quirks found (tolerated by the original)

- `DockPoint` types not in the table: `DPT_Drill` (sWorker/bWorker `Grinder`, `gun0`),
  `LatchPoint` (trJunkYardHQ `dawg`, dCrystal0/1 `center`).
- sRepairDroid's LOD 1-4 meshes do not exist (default mesh is substituted).
- nisLightSource1/2 and nisLensflareSource1 use the default ship's LOD and mex by design.
- sRammingFrigate/bRammingFrigate set only `minCollDamage` (5000), so `maxCollDamage` stays 0.
- Several dock points and nav lights have no mex chunk (bCloakedFighter/bAttackBomber `Link`,
  bSectMoship lights `new*`, p1Outpost, trFloatingCity, tCarrier `salproc`), several have equal
  heading and up, and support modules and trader ships lack `fowRadius`.
