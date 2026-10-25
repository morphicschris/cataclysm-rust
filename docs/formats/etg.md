# ETG effects (`ETG\*.ebg`, `ETG\*.script`) and the particle system

ETG ("Effects Tool of the Gods") is Homeworld's effect scripting language. Cataclysm ships
255 compiled-from-source effect files (`ETG\*.ebg`) and eight binding scripts. The Rust port
is `crates/hwc-fx`. This document records the format and the runtime behaviour as
`cataclysm.exe` implements them (verified against the binary; HW1's `ETG.c`/`Particle.c` are
the ancestor).

## Files

* `ETG\*.ebg` — C-preprocessed effect sources (CRLF text). They still carry `#line` markers
  and long runs of blank lines from `etg.h`/`texanims.etg`. `[expr]` brackets hold arithmetic
  evaluated at load time (`Eval.c`: `+ - * /`, parentheses, comparisons, `PI`, `SIN`, ...).
* `ETG\General.script`, `RACE_SECT|BEAST|KUSHAN|TAIDAN|P1|TRADERS|DERELICT.script` (in that
  order; `derelict.script` is not read) — `StatScript` key/value files:

| Key | Meaning |
|---|---|
| `LoadEffect file` | compile an effect |
| `SoftwareVersion hw, sw` | software-renderer substitute |
| `setDeathEvent race, class, kind, file` (or `derelictType, kind, file`) | kind = damage/projectile/beam/collision/special |
| `setFireEvent` / `setHitEvent` / `setBulletEffect` / `setBulletDestroyEvent race, gunSound, file` | gun events (`[7][20][5]`); `setHitEvent ..., NULL` clears the *fire* slot (HW1 bug kept) |
| `setBulletColor race, gunSound, r, g, b` | bullet colour |
| `bigBoomFactor race, class, n` / `derelictType, n` | default -10 |
| `setAsteroidEffect` / `setGaseousEffect race, file` | resourcing |
| `setTractorBeamEffect race, file` | |
| `setSpecialPurposeEffect EGT_xxx, file` | 43 slots (`StrToEffectNum` table) |
| `setDamageEvent class, light|heavy|dying, file` | |
| `setHyperspaceEvent file` | |
| `NumberEffects`, `etgHistoryScalarMin`, `etgSoftwareScalar{Damage,Hit,Fire}` | tuning |

Every assignment to the same slot appends an LOD level. Names are matched ignoring case
(Cataclysm uses `_stricmp` throughout ETG).

## Source language

Lines are read with `fileLineRead` (255 chars, CR ends a line), `//` comments stripped,
leading blanks and (except on one-character lines) trailing blanks removed, then matched in
order against: the parse table, the function table, `var <- value` assignment.

* Structure: `eventStart(type name [= init], ...)` declares the parameters (the first
  variables), `variable { type name [= init] ... }`, `startup { }`, `eachFrame { }`,
  `timeIndex { }`, `particleBlocks(n)` (default 8), `maxFrequency(n)` (spawn history),
  `worldRender`. Types: `int`, `float`, `RGB`, `RGBA` (colour initialisers are `r,g,b[,a]` in
  0..1). Outside any block, `int`/`float`, `subTextureDWORD(tex, uvBytes, flags)`,
  `morphAnimDWORD(mesh, flags)` and `label(name)` build constant data (texture and mesh
  animations, offset arrays).
* Code: `if (a op b) { } [else { }]` (`= == != <> > >= < <=`, int or float, operands may be
  swapped), `at(t)`, `before(t)`, `after(t)`, `every(t)`, `between(t0, t1)` blocks (time or
  float variable), `timeBlock(var) { time(t, value) ... }` linear effectors, `alternate {
  alt(n) ... }` weighted random choice, `label`/`goto`/`call`/`return`, `end`, `delete`,
  `spawn(file, args...)`, `createEffects(file, n, spread, args...)`, `setTexture(tex, u0, v0,
  u1, v1)`, `setMesh(file)`, and calls of the 168 functions in `functions.rs` (`set*` write
  the particle attribute block, `modify*` change a created system, `create*` make systems and
  may be followed by a `{ }` block run once per particle, maths, randoms, colour helpers,
  `setEff*` spawn offsets, `eff*` this-effect changes, `playSound`, `deleteParentShip`, ...).

Quirks reproduced from the original:
* `x <- -5` stores 5: the assignment is split with `strtok(" \t<-")`.
* `%d` constants wrap silently (`4294967295` is `-1`; colours > 2^32 wrap).
* Re-opening a block of the same kind removes the previous trailing `End`, so repeated
  `eachFrame` blocks concatenate.
* `at(t)` times are moved to the middle of their 1/32 s slot.
* `setFramerate` is bound to `partSetLoopFlag` (it sets the loop flag to the low byte of the
  float), `modifyTrueBillboard` to `partModifyPseudoBillboard`, `print` and
  `modifyBackgroundColor` to an empty stub; `modifyLighting(sys, 1)` takes a float and so
  clears `lit` (low byte of 1.0f is 0).
* `modifyTexture` loses its system argument (the parameter count is reset after it is
  written); `call label` to an already-defined label jumps to the label's variable offset.
* `carandom` declares 7 parameter types but receives 8 values.
* `BulletBigGunGlow.ebg` references an undefined symbol and cannot be compiled (the game never
  loads it); `SWHitProjectile.ebg` never closes its `startup` block.

## Byte-code

Three blocks (startup, each-frame, time-index) of little-endian dwords with the original
structure sizes; all branch and skip distances are byte offsets.

| Op | # | Layout (dwords) |
|---|---|---|
| Nop | 0 | op |
| Before/BeforeVar/After/AfterVar/At/AtVar/Every/EveryVar | 1-6, 8, 9 | op, skip, time or var offset |
| Between | 7 | op, start, end, startVar, endVar, skip |
| Effector VI/VF/VCo/VCa, CI/CF/CCo/CCa | 26-33 | op, end (const or var), var, rate offset, id byte |
| Delete / End / Yield | 34 / 35 / 36 | op |
| Goto / Call | 37 / 38 | op, block, target |
| Return | 39 | op |
| Compare V-C-I, V-V-I, V-C-F, V-V-F (= != > >= < <=) | 42-65 | op, var, value or var, skip |
| VarCopy / VarAssign | 70 / 71 | op, source, dest |
| Function | 72 | op, function, n, return var, passThis, (type, value) × n |
| Alternate | 73 | op, decision, table length, skip, offset table |

Parameter types: 1 int, 2 float, 3 RGB, 4 RGBA (variable offsets), 6 constant, 8 constant-data
pointer, 9 variable-block pointer. Pointers the original stored (functions, textures, meshes,
effect statics, offset tables) are indices in the port. A per-particle `{ }` after `create*`
compiles to: `CreateCallbackSetup(callbackOffset)`, the create call, `Goto` past the callback
code, the callback code, `End`.

`tests/emu_compiler.rs` compares the port's output for 13 effects (including the 615-line
`bigdeath.ebg`) with the original `etgEffectCodeLoad` run in Ghidra's emulator: identical.

## Runtime

* `etgEffectUpdate` runs once per universe update (`univUpdateAllPosVelEffects`, dt = 1/16 s,
  after the universe clock advances): startup when `total <= dt + 0.005`, then the time-index
  block, then each-frame unless deleted; then attachment (position/velocity/coordsys/N-Lips
  from the owner), drag, integration (skipped for some owner states), spin about Z (with
  `matMakeRotAboutZ(sin, cos)` — the arguments are swapped, as in HW1). Deletion waits until the
  effect has been drawn once or four updates.
* `FxWorld::assume_drawn` (off by default) treats every effect as drawn for this rule and
  `FxWorld::draw_randy` gives drawing its own generator; a simulation that must not depend on
  rendering sets both (`hwc_sim::effects`). `FxWorld::run_update` runs the effect pass without
  advancing the clock, for callers that keep `universe_time` equal to their universe clock.
* Particles advance in `etgEffectDraw` by `universe.totaltimeelapsed - lastUpdated`, so at the
  universe rate regardless of frame rate. New effects appended during the update loop are
  updated in the same pass.
* Random streams: 26 for ETG (`frandom`, `irandom`, `crandom`, alternates, frequency limits,
  `createEffects` spread), 25 for particle creation (`partRealDist`: `n ± (rand%1e6)*1e-6·d`),
  0 for `setRandomOffsetArray` and the electric-bolt jitter (drawn at render time).
* `etgFRandom` clamps its result to `[lo, hi]`; `crandom` mixes channels (green uses the blue
  upper bound); `EffectorCCo` advances green and blue from the red value (HW1 bug kept);
  integer effectors jump to `start + (end-start)*16/(int)(duration*16)`.

## Particles (Cataclysm)

* Particle record 0x100 bytes: as HW1 plus 2-D scale (`scaleX` 0x50, `scaleY` 0x54), a length
  clamp flag (0x70, set by `modifyDeltaLength`'s third argument, cleared by
  `setDeltaLength`), `turbulance` (0x74), `segments` (0x78), lens-flare pointer (0x7c), a copy
  of the effect velocity (0x98) between `wVel` and `wAccel`.
* Types: 0 points, 1 cubes, 2 lines, 3 meshes, 4 billboards (sprites and circles), 5 electric
  bolts (polyline of `segments` steps of `length/segments` along the velocity, each inner
  vertex jittered by ±½·turbulance·step), 6 lens flares (each particle drives a flare object;
  only the first flare's flag is cleared when drawn and the system never dies by itself).
* Every `create*` creates a cylindrical distribution (`partCreateSystemWithDelta`); the angle
  is `matMakeRotAboutZ(sin, cos)` swapped like the HW1 source.
* `partUpdateSystem` and particle creation are bit-identical to the binary on random inputs
  (`tests/emu_particles.rs`).
