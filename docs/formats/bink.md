# Bink movies (`Movies\*.bik`)

Decoder: `crates/hwc-video` (pure Rust; it never runs `binkw32.dll`).

The codec's constant tables (Huffman codebooks, scan orders, quantiser tables) are **not shipped**.
`BinkTables::from_binkw32` finds them in the player's own `binkw32.dll` by their structure, not by
address:

* the codebooks are a run of 16 complete direct-lookup prefix tables;
* the scans are 64-byte permutations of 0..63;
* the quantiser tables are 16×64 runs of positive 32-bit values.

A table is accepted only if its FNV-1a checksum matches the expected one. The importer stores the
result with `import_tables` as `bink_tables.bin` (magic `HWCBINKT`, about 9.8 KB), and
`BinkTables::load` reads it back.

## What ships

There are 20 files. All are **`BIKf`** (Bink 1, revision `f`), 640×480, YUV 4:2:0, with video
flags 0 (no alpha plane, not grayscale). None has an audio track, and each has a single keyframe
(frame 0).

| file          |   frames | rate                    | length   |
|---------------|---------:|-------------------------|----------|
| Animatic01–18 | 750–3689 | 30/1 fps                | 25–123 s |
| bdogintro     |      807 | 30/1 fps                | 26.9 s   |
| sierra        |      240 | 1000000/66667 (≈15 fps) | 16.0 s   |

That is 38,674 frames in total. Every frame decodes bit-identically to ffmpeg's `binkvideo`
decoder. This was checked for all 20 movies, and setting `HWC_FFMPEG` enables a regression test
for two of them.

No `.bik` declares an audio track (track count 0 in all 20 headers), so there is no Bink audio
(RDFT or DCT) to decode and none of RAD's audio tables is needed. The animatic soundtracks are
`Movies\<name>.script` NIS event scripts instead (see "Soundtrack scripts" below); the two
start-up movies play a music track (60, 61) for their length.

## Container

The file starts with a 44-byte header, all integers little-endian:

* `"BIK"` and a revision byte
* file size − 8
* frame count
* largest frame size
* frame count, repeated
* width, height
* fps numerator, fps denominator
* video flags
* audio track count `n`

The header is followed by these tables:

* `n` × u32: the largest decoded audio packet per track.
* `n` × (u16 sample rate, u16 flags): flag 0x1000 means DCT coding, 0x2000 stereo, 0x4000 16-bit.
* `n` × u32: track id.
* `frames + 1` × u32 frame offsets, where bit 0 marks a keyframe.

Each frame packet holds, for every audio track in order, a u32 length followed by that many
bytes. The rest of the packet is video data.

## Video (revisions f–k)

* **Bitstream and planes.** Bits are read LSB-first. Planes are coded in the order Y, U, V. Each
  plane ends on a 32-bit boundary. Two later revisions differ:
  * Revision `h` and later code V before U.
  * Revision `i` and later start the frame with 32 unused bits, and store colours directly
    rather than as sign-magnitude values around 128.
* **Bundles.** Planes are split into 8×8 blocks; the chroma block grid is `(w+15)/16` blocks
  wide. Nine bundles carry the per-block parameters: block types, sub-block types, colours,
  8-bit patterns, motion x, motion y, intra DC, inter DC and run lengths.
  * Each plane reads one header per bundle. The header chooses one of 16 fixed 16-symbol Huffman
    codebooks and gives a symbol permutation.
  * Before each block row, every bundle whose decoded values are used up reads a value count and
    decodes that many values. A count of zero ends that bundle for the rest of the plane.
  * Colours use 16 extra trees for the high nibble; the previous high nibble chooses which one.
* **Block types:**

  | type | meaning |
  |-----:|---------|
  | 0 | skip: copy from the previous frame |
  | 1 | scaled: an 8×8 sub-block of type run, intra, fill, pattern or raw, doubled to 16×16 |
  | 2 | motion copy |
  | 3 | run-length fill along one of 16 scan orders |
  | 4 | motion plus residue (a bit-plane coded additive correction) |
  | 5 | intra DCT |
  | 6 | fill |
  | 7 | motion plus inter DCT |
  | 8 | two-colour pattern |
  | 9 | raw pixels |
* **DCT coefficients** are coded by bit planes over a hierarchical group list, followed by a
  4-bit quantiser index.
  * Dequantisation multiplies each coefficient by a value from a per-quantiser table indexed by
    **raster position**, with 11 fractional bits.
  * The quantiser tables, Huffman codebooks and scan orders were read from the data section of
    the game's own `binkw32.dll` at import time (see above).
  * The inverse transform is an integer AAN-style 8-point IDCT, applied to columns and then rows,
    with a final `(x + 127) >> 8`.
* **Output** is BT.601 limited-range YUV. `hwc_video::yuv_to_rgba` converts it to RGBA, using
  nearest-neighbour chroma.

Performance (release build, one core): decoding runs at 3,900–4,700 frames/s, and the YUV→RGBA
conversion at about 1,200 frames/s. Both are well over 100× real time at 640×480.

## How the original game plays them

`animBinkPlay(index, …)` is the entry point for animatics. It is called after a mission is completed
and from `utyVideoPlay`. (The launch intro — sierra then bdogintro, with music tracks 60/61 — calls
`binkPlay` directly from `utyGameSystemsInit`.) In order, it:

1. Builds the paths `Movies\<name>.bik` and `Movies\<name>.script`, and loads the NIS script.
2. Stops music and sound effects.
3. Calls `binkInit(renderMode)`.
4. Clears the screen to black, hides the cursor and saves the game volumes.
5. Calls `binkPlay(file, displayFn, animBinkDecode, …, music = -1)`.
6. Afterwards, restores the volumes, clears speech and subtitles, and shows the cursor again.

### Buffer and pixel format

* `binkInit` allocates one 640×480 buffer. It uses 4 bytes per pixel for the OpenGL renderers
  (render modes 2, 3 and −1) and 2 bytes per pixel for mode 0 (software).
* The paint callback calls `BinkCopyToBuffer` into that buffer with height 480 and a pitch of
  2560 (32-bit) or 1280 (16-bit).
* When no display callback is given, `binkPlay` uses its own blitters, `binkBlitFrame16` and
  `binkBlitFrame32`. The desktop depth decides which: 16 bits or less selects the 16-bit blitter.

### Display

`animBinkDisplay` runs once per new frame; with the 3Dfx Glide path, `animBinkDisplay3Dfx` runs
instead. What it does depends on the render mode:

* **Mode 2** flips the buffer vertically and draws it with `glDrawPixels` as 640×480
  `GL_RGBA`/`GL_UNSIGNED_BYTE`. The image is **unscaled** and drawn at the raster origin.
* **Mode 3** draws it with `glcDisplayRGBABackgroundScaled`, scaled to the screen.
* **Mode 0** draws the 16-bit buffer unscaled.

The GDI blitter `binkBlitFrame` flips the rows and `BitBlt`s the 640×480 image **centred** on
the screen, at ((screenW − 640)/2, (screenH − 480)/2). If the full-screen flag is set it draws at
0,0 instead.

The screen is cleared to black first, so higher resolutions get a black border. The movies also
contain their own letterbox bars.

After the frame, every subtitle region with text is drawn over it (`subTitlesDraw`, on the movie
clock). `binkPlay` clears the game's "rendering on" flag for the length of the movie, and the
translucent band behind the speech region is only drawn while it is set, so movie subtitles have
no band; speaker portraits are drawn when the theme has one (the animatic speech events all carry
the no-portrait flag 0x4000_0000). After the buffers are flipped, when the lowest subtitle
bottom is in the upper half of the screen, the top of the new back buffer is cleared to black
(from y = 0 down to that bottom, at least 128 px) so that text does not pile up where the
unscaled movie does not reach.

### Timing and frame dropping

* The frame rate is `frameRateNum / frameRateDen` from the Bink header.
* Each update works out which frame is due from the elapsed wall-clock time.
* If the player is behind, it decodes the missing frames in a loop and paints only the last one.
  The loop makes an extra Bink service call every 4th skipped frame.
* The copy flags always include 0x04000000 alongside the surface type. The game also keeps a
  flag word that holds 0x04000000 while it is 2 or more frames behind.
* Playback ends when the due frame reaches the frame count.
* `animBinkDecode(frames)` is called once per update with the number of frames decoded so far
  and converts it to seconds (`frames / fps`). It fires every script event whose time is at or
  before that, sets the game clock the subtitles run on to it, then updates the subtitles.

### Soundtrack scripts

`animBinkPlay` builds `Movies\<name>.script` next to the movie (with the language folder
inserted for other languages) and reads it with `animLoadNISScript`: the NIS keyword table,
events sorted by time then script order. The 18 scripts use six keywords:

* at time 0: `musicVolume`, `speechVolume`, `SFXVolume`. These go to `soundEvent*Vol`, whose
  master factor is always 1, so they are absolute levels that replace the player's settings for
  the movie; the levels saved before the movie are restored afterwards.
* `startMusic n` (tracks 42–58).
* `animaticSpeechEvent (n + flags)`, 81 in all: `speechEventQueue` with no object or actor. The
  actor flags choose the stream (Fleet Command / Tactical Officer on stream 1, the generic
  voice on stream 2, others on 0) and the subtitle theme; every one also has 0x4000_0000 (no
  portrait). The subtitles go to the speech region at the top of the screen.

Before the movie the music and effects stop and the subtitles are cleared; afterwards the music,
effects and speech stop, the subtitles are cleared again and the levels come back. The
recreation plays these in `apps/cataclysm` (`movie_script.rs`).

### Skipping and pausing

* While a movie is playing, the window procedure calls `binkStop` when Escape is released (`WM_KEYUP`); other keys and mouse clicks
  do nothing. This is
  the code path that otherwise forwards ESC. `binkStop` sets the stop flag, and the play loop
  exits on its next iteration.
* Losing focus pauses playback: `DeactivateMe` calls `binkPause(1)`. On resume, the clock base is
  reset to the current frame.
