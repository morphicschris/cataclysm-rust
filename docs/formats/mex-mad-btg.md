# MEX, MAD and BTG formats

Readers: `crates/hwc-formats/src/mex.rs`, `mad.rs`, `btg.rs`. Real-data tests:
`crates/hwc-formats/tests/mex_mad_btg_real.rs` (set `HWC_EXTRACTED_DIR` to an extracted archive;
the tests skip when it is unset).

All three formats are little-endian. Every count, offset and index is range-checked by the
readers; a malformed file produces an error, never a panic.

Verified against the full Cataclysm archive: **967 `.mex`, 29 `.mad`, 46 `.btg`, 0 failures.**

---

## `.mex` — mesh extensions

One per mesh LOD (`<ship>/rl0/lodN/<ship>.mex`, plus `defaultship`, derelicts and effect meshes).
Holds collision volumes and named points (guns, engines, docking, salvage, damage effects, nav
lights). No absolute offsets: a header then chunks laid end to end.

### Header (12 bytes)

| offset | type    | field                     |
|--------|---------|---------------------------|
| 0      | [u8; 8] | `"mannngo\0"`             |
| 8      | u16     | version — `0x106` in all 967 files |
| 10     | u16     | chunk count               |

### Chunk (16-byte header + body)

| offset | type    | field                                   |
|--------|---------|-----------------------------------------|
| 0      | [u8; 4] | tag, NUL-terminated                     |
| 4      | [u8; 8] | chunk name, NUL-terminated              |
| 12     | u32     | body size (bytes after this header)     |

Text fields are NUL-terminated inside fixed-size fields; the bytes after the NUL are uninitialised
tool memory (`0xAB` in chunk headers, `0xCD` in bodies) and must be ignored. Angle fields the
exporter never filled hold `0xCDCDCDCD` (reads as about −4.3e8); the reader reports them as
`None`.

A *point* is 32 bytes: position `f32×3`, normal `f32×3` (not always unit length; can be zero),
cone angle `f32` and edge angle `f32` (degrees).

| tag   | names seen                          | body size | body                                                       | count |
|-------|-------------------------------------|-----------|------------------------------------------------------------|-------|
| `Col` | `Col0`                              | 20        | level `u32` (always 0), centre `f32×3`, radius `f32`       | 967 (one per file) |
| `Rct` | `Rct0`                              | 28        | level `u32` (always 1), min corner `f32×3`, extent `f32×3` | 967 (one per file) |
| `Gun` | `GUN0`–`GUN15`, `REP0`, `BEAM0`     | 32        | point                                                      | 1275 |
| `Eng` | `Eng0`–`Eng2`                       | **64**    | point + attached object name `[u8; 32]`                    | 463 |
| `Dok` | `Dock`                              | 52        | point + dock name `[u8; 20]` (`LEECH0`, `RES1`, `CENTER`…) | 5737 |
| `Sal` | `SALVAGE`                           | 52        | point + salvage name `[u8; 20]` (`POINT1`…`POINT8`)        | 557 |
| `Dmg` | `Dmg0`–`Dmg34`                      | 32        | point                                                      | 5543 |
| `Nav` | `Nav`                               | **68**    | position `f32×3`, colour `u8×4` (R, **B**, G, pad), light name `[u8; 20]`, attached object name `[u8; 32]` | 2951 |

No trailing bytes after the last chunk in any file. Chunk counts per file range from 2 (just
`Col`+`Rct`, 470 files) to over 80.

### Differences from Homeworld 1

* **`Eng` grew from 32 to 64 bytes and `Nav` from 36 to 68 bytes.** Both gained a trailing
  32-byte name field. In 90 chunks it is non-empty, and each value checked (`LeechHead`,
  `LeechThrustL/R`, `RammingFrigateNose`, `Thruster(1)`, `Thruster(2)`, `sentinel`, `Swarmer`,
  `bswarmer`, `Dreadnought`, `RammingFrigate`) is the name of a polygon object in the matching
  `.geo` — and those objects are the ones animated by the ship's `.mad`. So it is almost certainly
  the mesh object the engine glow / nav light is parented to, so it moves with mesh animation
  (e.g. the Leech's head and thrusters). Empty string = attached to the root. The reader accepts
  the short HW1 forms too (`attached_to = None`).
* **`Dmg` chunks** are not read by the HW1 engine code at all. They are points (same layout as
  `Gun`, almost always cone 30°/edge 5°), numbered `Dmg0..DmgN`, up to 35 per ship. Best guess:
  locations for damage smoke/fire effects (Cataclysm shows localised damage effects on hulls).
  It is unknown whether HW1's shipped `.mex` files already contained them.
* No other new chunk kinds exist in Cataclysm data — every chunk in the archive is one of the eight
  tags above.
* Dock points are far more numerous (5737) because of Cataclysm's `LEECHn`/`RESLEECHn` attachment
  points for Beast leeches / worker attachment.

Gun chunk names are upper case (`GUN0`) while HW1 code looked up `Gun0`; lookups are
case-insensitive, as in the original.

Odd data: `etg/meshes/junk.mex` has a collision box with z extent −9999993 (a placeholder). 265
`Gun`, 217 `Eng` and 80 `Dmg` chunks have unset (`0xCD`) angles.

---

## `.mad` — mesh animation

29 files, one per animated ship (`race_*/<ship>.mad`). **Layout is identical to Homeworld 1**
(identifier `"Mellonn\0"`, version `0.91f`); no Cataclysm changes.

### Header (40 bytes) + animation table

| offset | type    | field                                                        |
|--------|---------|--------------------------------------------------------------|
| 0      | [u8; 8] | `"Mellonn\0"`                                                |
| 8      | f32     | version, `0.91`                                              |
| 12     | u32     | string block length (block = last N bytes of the file)       |
| 16     | u32     | runtime pointer slot; in every file it equals the string block's file offset |
| 20     | f32     | total timeline length, seconds                               |
| 24     | f32     | authoring frame rate (30 in all files)                       |
| 28     | u32     | object path count                                            |
| 32     | u32     | file offset of the object path table                         |
| 36     | u32     | animation count (engine limit 32)                            |
| 40     | 16 × n  | animations                                                   |

Animation (16 bytes): name offset into the string block `u32`, start `f32`, end `f32`, flags `u32`
(bit 0 = loop; 0 in every file).

Object path (48 bytes): name offset (string block) `u32`, CRC slot `u16` (0 on disk; computed at
load), pad `u16`, membership bits `u32` (bit *n* = object moves in animation *n*), keyframe count
`u32`, then nine file offsets: times (`f32`), TCB parameters (`f32×3` each), then x, y, z,
heading, pitch, bank channels (`f32` each).

In every file the data is perfectly contiguous: header, animations, object table, then per object
times / TCB / six channels (40 bytes per keyframe), then the string block. The reader does not rely
on this and follows the offsets.

All animations share one timeline in seconds; each animation is a `[start, end]` window (first
keyframe is often at −1.0 s). All TCB parameters seen are zero (plain Catmull-Rom style splines).

Totals: 94 animations, 157 object paths, 3057 keyframes. Examples: `bworker.mad` — `JackOut`,
`JackClose`, `DockClose`, `DockOpen`, `BladesSpin`, `JackSpin`, `DrillAndBlades` over 12 objects;
`bionarrayfrigate.mad` — `Array_Close/Open` plus `_DMG` variants; `brammingfrigate.mad` —
`RamCollapse`, `RamRecover`, `AftBurnIgnite`, `AftBurnFiring`, `CancelAftBurn`.

Cataclysm-specific *content* (not format): animation names are no longer limited to HW1's
gun/door/wings/special set — e.g. `Make_Ambassador`, `Link`/`Unlink` (Acolyte docking), `CombatIn`/
`CombatOut`, `ShieldDeploy`, `TurretDeploy`. How names map to ship states lives in ship scripts /
code, not here. `kadvancesupportfrigate.mad` contains three junk animations named
`AdvanceSupportFrigateLowerArm.lwo` (an exporter slip).

---

## `.btg` — background

46 files in `btg/`. **Layout is identical to Homeworld 1 version `0x600`.** Records are packed (no
alignment padding — the 4-byte flags are directly followed by `f64`s).

### Header (84 bytes)

| offset | type  | field                                                        |
|--------|-------|--------------------------------------------------------------|
| 0      | u32   | version `0x600`                                              |
| 4      | u32   | vertex count                                                 |
| 8      | u32   | star count                                                   |
| 12     | u32   | polygon count                                                |
| 16     | i32×2 | editor scroll x, y                                           |
| 24     | u32   | editor zoom — actually the bits of an `f32` (0.25, 0.5, 1.0, 4.0 seen) |
| 28     | i32×2 | page width, height (2560 × 2048 everywhere)                  |
| 36     | i32×3 | colour multiplier RGB                                        |
| 48     | i32×3 | background clear colour RGB (all zero in Cataclysm files)    |
| 60     | u32×5 | editor toggles: vertices, polygons, stars, outlines, blends  |
| 80     | i32   | editor render mode (1; `mission17.btg` has 2)                |

### Body

1. Vertices, 40 bytes: flags `u32`, x `f64`, y `f64`, R G B A `i32×4`, brightness `i32`
   (always 255).
2. Stars, variable: flags `u32`, x `f64`, y `f64`, R G B A `i32×4`, name length `u32`, name bytes
   (no terminator; 7–16 bytes seen, the engine buffer is 48). Textures: `B02.tga`, `B03.tga`,
   `seanblu_dark.tga`, `seangrn_low.tga`, ...
3. Polygons, 16 bytes: flags `u32`, vertex indices `u32×3` (all in range).

Vertex coordinates are page pixels (0..2560, 0..2048), all within the page. Flags: 11 vertices,
121 stars and 2 polygons have flag 1, everything else 0 (likely editor selection state).

Totals: 33318 vertices, 8313 stars, 62816 polygons; `default.btg` = 770 / 97 / 1501. Several
backgrounds are byte-identical copies (`ez14`–`ez17`, `ez11`/`ez12`, `nis10`/`nis11`); `default` and `ez09` share counts and size
but differ in content.

---

## Open questions

* MEX: is the `Eng`/`Nav` trailing name exactly a `.geo` polygon-object name in all 90 cases
  (spot-checked, not exhaustively cross-referenced)? How are an attached point's coordinates
  expressed — mesh space at rest pose or object-local?
* MEX: exact use of `Dmg` points (effect type, how the cone/edge angles are used), and whether
  the `level` field of `Col`/`Rct` means anything beyond a constant.
* MEX: HW1 version number is not checked by the HW1 code, so we do not know if `0x106` differs
  from HW1's.
* MAD: animation start/end are in seconds but some windows start slightly before the first key of
  an object; confirm playback clamps rather than extrapolates.
* MAD: the keyframe rotation channel units (degrees vs radians) and axis convention need confirming
  when the animation player is written.
* BTG: meaning of vertex/star/polygon flag value 1, and whether `render_mode` 2 (mission17) changes
  runtime rendering or is editor-only.
