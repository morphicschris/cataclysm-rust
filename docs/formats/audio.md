# Audio formats

Reader: `crates/hwc-formats/src/audio.rs`. Tests: `crates/hwc-formats/tests/audio_real.rs`
(`HWC_EXTRACTED_DIR` for `soundfx\*.bnk|*.lut|*.wxh`, `HWC_GAME_DIR` for `Music4Cat.wxd` and the
`.vce` files; set `HWC_AUDIO_DUMP_DIR` to write sample WAVs).

All values little-endian. Offsets below are byte offsets.

## Files

| File | Where | Magic | Contents |
|---|---|---|---|
| `guns.bnk`, `ships.bnk`, `specialeffects.bnk`, `ui.bnk` | archive `soundfx\` | `BNK0` | 37 / 216 / 77 / 61 effect patches (391 total, ~22.7 min) |
| `*events.lut` (8 files) | archive | `PKG0` | event -> patch tables |
| `beastsentence.lut`, `sectsentence.lut` | archive | `LUT1` | speech event -> sentence |
| `beastphrase.lut`, `sectphrase.lut` | archive | `LUT2` | sentence variant -> fragment offsets in `.vce` |
| `volume.lut` / `range.lut` | archive | `VOL1` / `RNG1` | distance attenuation tables (3 x 92, int) |
| `frequency.lut` | archive | `FRQ1` | per-band distance EQ (8 x 3, float) |
| `music4cat.wxh` | archive | `WXH1` | music index, 62 tracks |
| `Music4Cat.wxd` | install | `WXD1` | music data, 68.8 MB, 103.9 min stereo |
| `BeastSpeech.vce` / `SectSpeech.vce` | install | `VCE0` | speech fragments, 1438 / 2096 |
| `soundfx\scripts\*.script` | archive | text | speech EQ/effect presets (`key value` text; not parsed here) |

Banks and event LUTs are paired by a "generation checksum" stored in both:
guns `0x40568380` (gunevents, gunspecevents), ships `0x9c7c3600` (shipcmnevents, shipevents,
derelictevents), specialeffects `0xa4f3b600` (specexpevents, spechitevents), ui `0x584ee300`
(uievents). Speech: beast `0xb6bb1a00`, sect `0x0d692490` (sentence LUT == phrase LUT == `.vce`).
Music: `0x0d5b9cf2` in both `.wxh` and `.wxd`.

## `.bnk` (BNK0)

```
0   char[4] "BNK0"
4   u32     checksum
8   i32     patch count
12  patch[count], 56 bytes each:
    0  char[4] "PCH0"
    4  u32  priority
    8  i32  pitch
    12 u32  data offset   (from start of file)
    16 u32  data size
    20 u32  loop start    (relative to data start, block aligned)
    24 u32  loop end      (relative to data start, block aligned)
    28 u16  bitrate       (codec bits per block; 256..1152 seen)
    30 u16  flags         (bit0 looping, bit1 "stretch")
    32 u8   volume
    33 i8   pan
    34 u8[2] padding (0xffff)
    36 WAVEFORMATEX: u16 tag=1, u16 channels=1, u32 rate (22050|44100), u32 bytes/s,
                     u16 block align=2, u16 bits=16, u16 cbSize=0
    54 u16  padding (0xffff)
```

Payloads follow the table back to back (sometimes 4 bytes apart); the last one ends the file.
Every shipped patch is mono and codec-compressed (no PCM; a zero bitrate is treated as PCM by the
reader, as a fallback). The `WAVEFORMATEX` only describes the source recording: 22 050 Hz and
44 100 Hz patches are coded identically and no patch codes anything above bin 127 (11 kHz).

## `.lut` tables

`PKG0` (sound-effect events):
```
0 "PKG0"  4 u32 checksum  8 u16 variations  10 u16 events  12 u16 objects
14 i16 table[objects][events][1 + variations]   ; [count, patch, patch, ...], unused = -1
```
Object = ship/gun/effect type, event = per-file event enum. Patch numbers index the paired bank.

`LUT1` (speech sentences):
```
0 "LUT1"  4 u32 checksum  8 u16 columns(=4)  10 u16 events(=863)  12 u16 actors(=1)
14 u16 bitrate[actors]       ; 512 for both races
.. i16 table[actors][events][columns] = priority, variations, max_variable, first phrase row
```
HW1 hard-coded four bitrates (one per speech actor); Cataclysm has a single actor and a single
bitrate. The reader derives the bitrate count from the file size, so both layouts parse.

`LUT2` (speech phrases): `0 "LUT2" 4 u32 checksum 8 u16 columns(=5) 10 u16 rows`, then
`i32 row[columns]` = duration (ms), probability (low byte; upper bytes are runtime scratch for the
variation shuffler), fragment count (low byte, same), then `columns - 3` = 2 fragment offsets into
the `.vce` (unused = -1). Rows for a sentence are `first + variation * max_variable + variable`.
Beast: 5579 rows (3468 placeholders with no audio), sect: 7538 rows (5149 placeholders). Every
real offset points at a fragment start; 13 beast and 18 sect fragments are never referenced.

`VOL1` / `RNG1`: `0 magic 4 u16 columns 6 u16 rows 8 i32[rows][columns]`. Range rows are
ascending distance breakpoints, volume rows the matching levels (row per ship/effect class).
`FRQ1`: same header, `f32` values in 0..1 (attenuation per EQ band per distance step).

## Music: `.wxh` + `.wxd`

```
WXH1: 0 "WXH1" 4 u32 checksum 8 i32 count, then 12-byte entries:
      u32 offset (of the DATA record in the .wxd), u16 flags (bit0 loop), u16 bitrate (512),
      u8 volume (255), u8 channels (2), u16 padding
WXD1: 0 "WXD1" 4 u32 checksum, then per track: "DATA" i32 size, payload
```
Tracks are contiguous; payloads are stereo, one 64-byte left block then one 64-byte right block
per 11.6 ms. Tracks 0-24 are flagged looping. Tracks 37-39 are 0.7 s of pure silence (every block
is an empty reset block).

## Speech: `.vce`

`"VCE0" u32 checksum`, then fragments, each `"INFO" i32 n, n bytes subtitle text (Latin-1, not
terminated)` followed by `"DATA" i32 size, payload`. Beast never has text (all `INFO` sizes 0);
sect has 624 subtitled fragments (the narrated intro, etc.). Payloads are mono, bitrate from the
sentence LUT (512). Records are not aligned (subtitle lengths are odd). HW1 also accepted an older
layout with `DATA` before `INFO`; Cataclysm files never use it and the reader does not.

## Codec ("fquant")

A block is `bitrate/8` bytes per channel and always covers 11.61 ms: two MDCT frames (primary,
secondary) on a fixed 86.13 Hz bin grid. Rendering at 22 050 Hz uses 128 bins/frame and a
256-sample window (256 output samples per block, what the game mixer does); 44 100 Hz uses 256 bins
and 512 samples; 11 025 Hz 64 bins and 128 samples. Because no shipped data codes bins above 127,
44.1 kHz output contains no extra content.

Bitstream (LSB-first within a little-endian bit stream, zero padding past `bitrate` bits; reads are
capped at 260 bytes):

1. Header: 2 bits flags (bit0: reset exponents to 0, bit1: secondary frame = copy of primary),
   3 bits band-step width `B`, 2 bits exponent width `E`, 4 bits bin-step width `S`.
2. If `B` and `E` are non-zero, up to 26 exponent updates: `B`-bit band step (0 ends the list,
   except for the first), then `E`-bit exponent applied to every bin of that critical band. Band
   edges: 0,1,2,3,4,5,6,7,9,11,13,15,17,20,23,27,31,37,43,51,62,74,89,110,139,180,256. Exponents
   persist per channel across blocks.
3. If `S` > 0, the primary frame: repeated `S`-bit bin step (0 ends, except first) and a
   sign-magnitude coefficient of `exponent + 2` bits. Value = `q * 10^(e+1) / (2^(e+1) - 1)`;
   exponents >= 6 mean silence. Then the secondary frame the same way, unless flag bit1 copies it.
   Reading stops whenever the bit position reaches `bitrate`.

Synthesis: IMDCT of each frame (computed with an n/4-point complex FFT and pre/post rotation;
unit tests check it against a direct IMDCT), sine window, 50 % overlap-add, one hop of latency.
Output is on a 16-bit scale; the game then applies a log soft-knee above half scale before
clamping (`soft_limit`, used by `DecodedAudio::to_i16`/`to_wav`).

Differences from the game's synthesis, both deliberate and selectable:

* Window: the game uses `sin(pi*i/n)`, half a sample off the symmetric `sin(pi*(i+0.5)/n)`, so
  aliasing does not fully cancel. On loud low-frequency patches this leaves a click at each hop
  boundary (second-difference energy up to 8x the median at those positions, vs 3x with the
  symmetric window and ~1.1x on normal material). `SynthesisWindow::Symmetric` is the default;
  `SynthesisWindow::Game` reproduces the original. A unit test confirms the symmetric path
  reconstructs a forward-MDCT'd signal exactly.
* Gain: the game scales by `8/sqrt(n)`, which is right at its 22 050 Hz mixing size but makes
  11 025 / 44 100 Hz renderings 3 dB louder / quieter. The reader uses the 22 050 Hz gain for all
  rates (tests check 44.1 kHz and 22.05 kHz renderings carry equal energy).

Verified on all data: all 391 patches, sampled speech (~80 fragments) and the first 15 s of five
music tracks decode with finite samples, lengths exactly `blocks x samples_per_block` matching the
header sizes, peaks ~20-25k before the soft knee, no near-silent patches, plausible spectra (speech
centroid ~370-1200 Hz, music L/R correlation 0.5-0.7). Release build: every bank patch (1359 s of
audio, incl. 44.1 kHz re-renders) decodes in ~1 s; 75 s of music in 28 ms.

## Differences from HW1

* Sentence LUT bitrate array is `numactors` long (1) rather than a fixed 4.
* Banks carry 44 100 Hz source formats alongside 22 050 Hz (the codec does not care).
* Speech is split into two race-specific `.vce`/LUT pairs (Beast, Somtaaw "Sect") instead of one
  `speechphrase_comp.lut` / `speechsentence_comp.lut`; music header is `music4cat.wxh`.
* Phrase LUT rows are i32 (HW1 headers declare them i32 too, though some HW1 code byte-swaps them
  as i16).

## Open questions

* Exact encoder window / analysis: the remaining boundary residue with the symmetric window on very
  low-frequency material (~3x median) suggests the encoder differs slightly (window shape or a
  quantisation detail). Inaudible in practice but not proven bit-for-bit.
* Patch `pitch`, `priority` and `flags` bit1 ("stretch") semantics in Cataclysm's mixer.
* `soundfx\scripts\*.script` (EQ presets for speech actors) are plain text and left to the script
  reader; `frequency.lut` band count (8) vs the mixer's EQ band layout is unconfirmed.
* Volume/range LUT row assignment (which row belongs to which ship class) lives in game code.
