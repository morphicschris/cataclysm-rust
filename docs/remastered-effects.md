# Optional Homeworld Remastered explosions and engine trails

A user-requested addition, off by default. Players who own Homeworld Remastered can have ship
deaths and engine trails drawn with Remastered's effects instead of Cataclysm's. Nothing from
Remastered ships with this game: the effects are imported from the player's own install, and
without them (or with the option off) the stock effects are used.

## Importing

```
hwc-import --hwr "<Steam>\steamapps\common\Homeworld" <data folder>
```

`--hwr` can also be given with a normal import. The install is only read. It writes
`<data folder>\hwr\` (about 11 MB): the `.events` and `.ship` files of the Kushan and Taiidan
ships from `HW1Ships.big`, every effect their death sequences start (followed through combos,
spray particles and spawns), the textures and animations those effects use and the engine-trail
textures from `Homeworld2.big`. The game looks for the folder in `$HWC_HWR_DIR`, then `hwr\` in
the data folder, then `hwr\` next to the executable.

## Turning it on

ENHANCEMENTS > DISPLAY > REMASTERED EXPLOSIONS AND TRAILS, or `hwcRemasteredFx = 1` in the
options file. It applies at once.

## What changes

Only the drawing. The simulation still starts and runs the original death effects (they time
the hulk's removal and are saved with the game); their primitives are left out of the render
list (`FxWorld::draw_except`; spawned effects are traced to their death effect through
`Effect::family`) and the Remastered death sequence plays where the ship died instead.

* **Deaths.** Each ship's `Death` animation events (`ship/<ship>/<ship>.events`) start effects
  at `animtime × length` seconds, at hull markers (`top_00`, `front_01`, ...: placed along the
  ship's axes) with `fx_scale` (a number, or `entity` for the ship's size). A Cataclysm ship
  takes the sequence of the closest Remastered Homeworld 1 ship by class (Kushan, or Taiidan for
  the Taiidan, Turanic and Beast). Remastered units are scaled by 4.5 to Cataclysm's.
* **Engine trails.** Ribbon trails keep the original's recorded nozzle path and gating and are
  drawn with Remastered's ribbon texture, the stand-in ship's `setEngineTrail` width and the
  race's `*_trail_clr` brightness ramp, in the team's trail colour with a hot core. Capital-ship
  glows are unchanged.

## The effect player (`crates/hwc-hwr`)

The Homeworld 2 effect runtime is not available, so the player follows the data: curves run over
each effect's normalised life; `STYLE_RING` is a textured quad (billboard or in the effect's
plane) sized by `Radius`, tinted by `Colour`, spun by `SpinZ` and moved by `Offset`, with
`BirthSpawn_Fx`/`Spawn_Fx`; `STYLE_SPRAY` emits `Emitter_Rate` particles a second for
`Emitter_Duration` seconds into a cone of `Emitter_Deviation` degrees, each particle another
effect moving at `Particle_Speed` and slowed by `Emitter_Drag`; `STYLE_COMBO` starts its effects
together; `STYLE_TRAIL` is a ribbon through its parent's recent positions. Animated textures
(`.anim`) are image lists or sprite-sheet cells. `Blending = 2` adds; the others blend by alpha
(24-bit art takes its coverage from brightness). Lights, beams, lens flares and mesh shapes
(`UseMesh`) are not drawn.

## Known gaps

* A capital ship's hull disappears when the original death effect removes it, which can be
  before the Remastered sequence's final blast.
* Death sounds stay the original's.
* Damage smoke and fire (the `.events` damage animations) are not imported yet.
