# Fonts, front-end screens, scene lighting and cinematics

Readers (temporarily under `hwc_formats::font` until they are re-homed in the crate root):

| Extension | Module | Source file |
| --- | --- | --- |
| `.hff` | `hwc_formats::font` | `crates/hwc-formats/src/font.rs` |
| `.fib` | `hwc_formats::font::fib` | `crates/hwc-formats/src/fib.rs` |
| `.hsf` | `hwc_formats::font::hsf` | `crates/hwc-formats/src/hsf.rs` |
| `.nis` + `.script` | `hwc_formats::font::nis` | `crates/hwc-formats/src/nis.rs` |

Every reader exposes `parse(&[u8]) -> Result<T>` with a per-module `Error` (bounds-checked
offsets, no panics on bad input). Real-data test: `crates/hwc-formats/tests/misc_real.rs`
(`HWC_EXTRACTED_DIR=<extraction root>`, `--nocapture` prints the survey numbers below).

| Type | Files | Parsed | Content |
| --- | --- | --- | --- |
| `.hff` | 102 | 102 | 19 703 glyphs |
| `.fib` | 21 | 21 | 154 screens, 219 links, 4 664 atoms |
| `.hsf` | 34 | 34 | 3 lights each (one file has 2) |
| `.nis` | 14 | 14 | 607 paths (593 actors + 14 cameras), 8 609 keys |
| `.script` (NIS) | 14 | 14 | 1 396 events, 532 `with` blocks, all resolved to a path |

All four binary formats are memory images of the engine's C structs with pointers saved as file
offsets (0 = null), little-endian, and uninitialised bytes filled with the debug-heap pattern
`0xCD`. Strings are NUL-terminated 8-bit text, decoded as Latin-1.

## `.hff` bitmap fonts

Identical to Homeworld 1: identifier `"Orannge\0"`, version `0x100`.

| Offset | Type | Field |
| --- | --- | --- |
| 0 | `[u8; 8]` | `"Orannge\0"` |
| 8 | u16 | version `0x100` |
| 10 | u16 | flags, always 0 |
| 12 | i32 | number of characters present (94 or 213) |
| 16 | i32 | spacing: extra advance after each glyph; also the advance of a missing character (1 in 83 fonts; −1, −2 or −3 in the heavier display fonts) |
| 20 | i32 | full height (line height) |
| 24 | i32 | baseline, always equal to full height |
| 28 | u32 | name offset, always 0 |
| 32 | i32 × 2 | image width, height (height = full height: one strip) |
| 40 | i32 | palette colours, always 2 |
| 44 | u32 | palette offset (RGBA ×2: black, white; unused by rendering) |
| 48 | u32 | image offset |
| 52 | u32 | runtime handle slot, junk `0xABAB` |
| 56 | `[u8; 12]` | reserved |
| 68 | u32 × 256 | per character code: offset of its 12-byte record, 0 if absent |

Character record: `i16 u, v, width, height, offset_x, offset_y`. The bitmap is the
`(width − offset_x) × (height − offset_y)` block at `(u + offset_x, v + offset_y)`; the offsets
are 0 everywhere and every glyph is the full image height. Glyphs are packed left to right.

The image is one byte per pixel, top row first, holding **4-bit coverage** (0–15; observed values
0, 2 and 4–15) — not palette indices. HW1 expands it to alpha as `value × 16`; the reader's
`GlyphBitmap::alpha()` uses `value × 17` so 15 maps to 255. Horizontal layout: advance by
`width + spacing` per glyph, `spacing` for a missing code; `&` in UI strings marks the hot-key
letter (drawn highlighted, not printed).

Fonts by language: `english/` has 26 files, `french/ german/ italian/ spanish/` 19 each. 94-glyph
fonts (`arial_09`, `arial_13`) cover `!`–`~` plus space but **not `"`**; the 213-glyph fonts add
Latin-1 accented letters for localisation. FIB files only reference three fonts:
`HW_EuroseCond_11.hff` (1 467 labels), `EuroseWideHeavy_i17.hff` (148), `Arial_Narrow_9.hff` (5);
names are matched case-insensitively against the lower-case files.

## `.fib` front-end screen layouts

Same identifier (`"Bananna\0"`) and version (`0x212`) as HW1.

Header (12 bytes): identifier, u16 version, u16 screen count; the screen array follows.

Screen (20 bytes): name offset, u32 flags (always 0), u16 link count, u16 atom count, links
offset, atoms offset.

Link (12 bytes): name offset, u32 flags (`1` enabled, `2` default-prev, `4` default-next,
`8` retain-previous; shipped values 1, 3, 5), target screen name offset. Link names follow the
convention `From-TO-Target`; every link target is a screen in the same file.

Atom (76 bytes):

| Offset | Type | Field |
| --- | --- | --- |
| 0 | u32 | name offset: callback name, link name, radio group, or (decorative regions) the `.lif` bitmap path |
| 4 | u32 | flags (below) |
| 8 | u32 | runtime status, junk |
| 12 | u8 | kind (below) |
| 13 | u8 | border width (`0xCD` on text labels) |
| 14 | u16 | tab-stop order |
| 16 | `[u8; 4]` | border colour RGBA (also the drop-shadow colour of text) |
| 20 | `[u8; 4]` | content colour RGBA (text colour of labels) |
| 24 | i32 × 4 | left, top, right, bottom (exclusive) on a 640×480 canvas |
| 40 | u32 | data: label text offset, or radio-button value (not an offset) |
| 44 | u32 | attribs: font file name offset (labels), bitmap name offset (bitmap buttons) |
| 48 | u8 | hot-key modifier mask (`1` Ctrl, `2` Alt, `4` Shift) |
| 49 | `[u8; 5]` | hot key per language: English, French, German, Spanish, Italian |
| 54 | `[u8; 2]` | padding |
| 56 | u32 × 2 | draw styles (first is 1/2/3/4/5 or junk for labels; second junk) |
| 64 | u32 × 3 | runtime region pointer and padding, junk |

Kinds (count in shipped files): 0 plain region / text label (2 356), 1 user region (199, drawn by
a named callback), 3 button (532), 4 check box (150), 5 toggle button (14), 6 scroll bar (2),
8 text entry (62), 11 menu item (24), 12 radio button (163), 13 cut-out region (50),
14 decorative bitmap (944), 15 divider (1), 16 list window (56), 17 bitmap button (33),
18 horizontal slider (52), 19 vertical slider (24), 21 opaque decorative region (2). Kinds 2
(static text), 7, 9, 10 and 20 never occur: labels are stored as kind 0 with non-null data and
become static text at load time.

Flags: `0x1` link (press follows the link of the same name), `0x2` function (press calls the
callback of the same name), `0x4` bitmap (never set), `0x8` modal, `0x10` popup, `0x20`
call-on-create, `0x40` contents visible, `0x80` default OK, `0x100` default back, `0x200` always
on top, `0x400` draggable, `0x800` border visible, `0x1000` disabled, `0x2000` don't cut out base,
`0x4000` call-on-delete, `0x8000` hidden, `0x30000` justification (1 left, 2 right, 3 centre),
`0x40000` background, `0x0FF00000` text drop-shadow direction bits (N, S, E, W, NE, NW, SE, SW).

Rust model: `Fib { screens: Vec<Screen { name, flags, links: Vec<Link>, atoms: Vec<Atom> }> }`;
`Atom` has `kind`/`raw_kind`, `rect`, colours, `data: AtomData::{None, Text { text, font },
RadioValue, Raw}`, `bitmap`, `hotkeys`, plus helpers `has(flag)`, `justification()`,
`drop_shadow()`, `text()`, `font()`, `bitmap_path()`.

## `.hsf` scene lighting

Not hyperspace: "Homeworld scene file", the lighting rig loaded per mission, per NIS environment
and for the showroom (`hsf/default.hsf` is the fallback). Same as HW1: identifier
`"rrasberry\0\0\0"` (12 bytes), u32 version `0x100`, u32 light count, then 44-byte records:

| Offset | Type | Field |
| --- | --- | --- |
| 0 | u32 | kind: 0 distant, 1 point, 2 spot, 3 linear, 4 area, 5 ambient |
| 4 | f32 × 3 | position (exporter axes; HW1 swaps into GL axes, and uses it as a direction for distant lights) |
| 16 | f32 × 3 | heading, pitch, bank (radians) |
| 28 | f32 | spot cone angle (degrees) — `0xCDCDCDCD` for distant lights, 0 for ambient |
| 32 | f32 | spot edge angle (same) |
| 36 | u8 × 3 | colour RGB |
| 39 | u8 | padding |
| 40 | f32 | intensity (0–3) |

Every file has the ambient light first, then one or two key lights. `ez01.hsf`/`ez02.hsf` use a
spot light (cone 30.25°, edge 5°) as their key light — HW1's loader treats every non-ambient
record as a directional light, so it would ignore the cone.

## `.nis` cinematics

Same identifier (`"Crannnberry\0"`) and version (`0x101`) as HW1, but the per-path records
changed meaning (see differences below).

Header (68 bytes, then 64 reserved bytes; data starts at offset 132):

| Offset | Type | Field |
| --- | --- | --- |
| 0 | `[u8; 12]` | identifier |
| 12 | u32 | version `0x101` |
| 16 | u32 | flags (0) |
| 20 | u32, u32 | string block offset, length (the tail of the file) |
| 28 | f32 | length in seconds |
| 32 | f32 | loop point; −1 (no loop) in every file |
| 36 | u32, u32 | object path count, offset |
| 44 | u32, u32 | camera path count (always 1), offset |
| 52 | u32, u32 | light path count (always 0), offset |
| 60 | u32, u32 | event count (always 0), offset |

Object path (56 bytes): u32 offset of the per-key duration array; u32 string-block offset of the
name (`sRecon(2)`: type name plus optional instance); u32 parent-name offset (always 0); u32 race;
i32 key count *n*; f32 start delay (always −1); u32 offset of key times; u32 offset of TCB
(tension, continuity, bias) triples; u32 × 6 offsets of the x, y, z, heading, pitch, bank arrays.
Camera path (44 bytes): duration-array offset, then the same fields from the key count onwards.
Arrays are grouped by channel across all paths (all time arrays, then all TCB arrays, ...).

Keys: the first three keys share time 0 and the last two are spline end conditions. The trailing
end-condition keys can be one frame *earlier* than the last real key (e.g. 110.667 → 110.633), so
only the body of each path is time-sorted. `durations[i] = times[i+1] − times[i]` exactly for
every body key. Angles are in degrees (HW1 converted them to radians at load). Some actor paths
run past the sequence length (by up to 17 s in `n01.nis`); the engine simply stops at the length.

Race field: 0 Somtaaw (`s…`), 1 Beast (`b…`), 2 Kushan (`k…`), 3 Taiidan (`t…`), 4 Turanic
(`P1…`), 5 Bentusi/traders (`trFloatingCity`), 6 derelicts (`d…`, plus the anchor objects
`nisLightSource1/2` and `NISlensflareSource1`), 7 asteroids.

### `.script` event lists

Events are not in the binary at all; each `nXX.nis` has a text `nXX.script`, C-preprocessor
output (`#line` markers; other `#` lines are commented-out events). One keyword per line, the rest
of the line is its argument string:

- `at frame N` (30 fps) or `at SECONDS` sets the time of following events;
- `with OBJECT` + `{ ... }` scopes events to an object. Names are written
  `RACE_SECT\sRecon (2)` / `Asteroids\asteroid1 (1)`; dropping the directory prefix and blanks and
  lower-casing matches the path name in the `.nis` (`nis::object_key`, `Nis::find_object`; all 532
  resolve);
- anything else is an event.

Keywords used (count): show 541, hide 265, colorScheme 191, fleetSpeechEvent 75, minimumLODSet 32,
fadeTo 30, deathDamage 24, fadeSet 23, attack 23, haltAttack 20, cameraFOV 18, speechVolume 17,
health 16, wideScreenIn 14, startMusic/musicVolume/SFXVolume 13, subtitle 12, motionCueScale 12,
remainAtEnd 6, hideUniverseToggle 6, meshAnimStart 4, fog 4, speechEvent/mimic/fire 3,
lightDiffuse/lightAmbient/globalAmbient/ebolts 2, widescreenOut/stretch/repair/openSlipgate/
numberLights/closeslipgate/beastEffect 1. The reader keeps keyword + raw argument text; it does not
decode per-keyword arguments.

## Cataclysm vs Homeworld 1

| Format | Difference |
| --- | --- |
| `.hff` | None found. All fonts are monochrome-coverage (flags 0), as HW1 requires. |
| `.fib` | Same version and 76-byte atom. Rectangles are four 32-bit edges; HW1's in-memory struct reads each as a 16-bit value plus a 16-bit "loaded" copy (the high half). Hot keys are per language (5 slots), shipped for English plus some localised letters. |
| `.hsf` | Same layout; Cataclysm uses spot lights (kind 2) in some `ez*` files. |
| `.nis` | First word of an object path (HW1 "instance", filled at load) and of a camera path (HW1 path length, f32) is now an offset to a per-key duration array. Race codes differ: derelicts are 6 and asteroids 7 (HW1: asteroid 6, derelict 10). Object-name instance suffix `(n)` and string-block naming are unchanged. Script vocabulary adds `subtitle`, `fog`, `mimic`, `ebolts`, `openSlipgate`/`closeslipgate`, `beastEffect`, `stretch`, `repair`, `minimumLODSet`, `motionCueScale`, `globalAmbient`, `lightAmbient`, `lightDiffuse`, `numberLights`, `widescreenOut`. |

## Open questions

- `.fib`: ten link-flagged buttons name a link their screen does not have (`Effects` has no links
  at all; `Old_Skirmish_Load` references `Skirmish_*` links). Probably dead screens or links
  resolved against another screen at runtime; behaviour on press is unknown.
- `.fib`: atom rectangles beyond 640×480 (e.g. y = 656) — presumably off-screen/hidden panels that
  the loader flags hidden after its resolution rescale, as HW1 does.
- `.fib`: meaning of draw-style values 1–5 and of user-region callbacks is engine-side.
- `.fib`: hot-key bytes past English are mostly set (e.g. `S U S` for single player), but whether
  slot order is exactly English/French/German/Spanish/Italian in Cataclysm is assumed from HW1.
- `.nis`: what consumes the per-key duration array (likely a Cataclysm motion-cue / timing
  feature; `motionCueScale` in the scripts is a candidate).
- `.nis`: whether race codes 0–5 map exactly as listed for every Cataclysm ship (inferred from
  name prefixes).
- `.nis` scripts: argument grammar of the Cataclysm-only keywords (`fog` takes 10 numbers,
  `subtitle` an index and a duration in ms, `fleetSpeechEvent` an expression like
  `(343 + 2048 + 536870912)` combining a speech id with flag bits).
- `.hsf`: whether Cataclysm's engine honours spot cone/edge angles and the orientation triple.
