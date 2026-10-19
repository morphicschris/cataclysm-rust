# HWC_Modern — recovery and Rust recreation plan

Goal: a 1:1 behavioural recreation of Homeworld: Cataclysm (Barking Dog / Sierra, 2000) in Rust,
built from recovered source, running on modern systems, loading the original game data unchanged.

The original install at `D:\Games\Homeworld Cataclysm\Cataclysm` is **read-only** to this project.
Everything we produce lives under `D:\Git\HWC_Modern`.

## 1. What we are working from

### Original binaries (inventory: `docs/inventory/`)

| File | Size | Notes |
| --- | --- | --- |
| `cataclysm.exe` | 5.36 MB | PE32 i386, linked 2000-08-25 with MSVC 6.0. No PDB, no relocations. `.text` 3.1 MB of plain code. Extra `.cms_t`/`.cms_d` sections hold the entry point: a CD-protection wrapper (looks like early SecuROM) around the real game. 17 exports. |
| `rgl.dll` | 128 KB | Relic GL front end (269 exports), shared by the renderer back ends. |
| `rglsw.dll` | 408 KB | Software rasteriser back end. |
| `rgld3d.dll` | 84 KB | Direct3D (DirectDraw) back end. |
| `rglfx3.dll` | 64 KB | 3dfx Glide 2 back end. |
| `binkw32.dll` | 164 KB | RAD Bink video, third-party; replace, don't port. |
| `3Dfx\WinNT\opengl32.dll` | 688 KB | 3dfx mini-GL driver, third-party; not ported. |

`cataclysm.exe` imports winmm (wave out), wsock32 (networking), ddraw, gdi32 (OpenGL pixel formats),
advapi32 (registry), binkw32 and the usual kernel32/user32.

Strings show the original source tree was `C:\hwexp\src\game\*.c` and `C:\hwexp\src\win32\*.c` —
the **same layout as Homeworld 1** — plus the WON "Titan" networking library from `D:\code\titan\lib\...`.

### Original data

| File | Size | Format |
| --- | --- | --- |
| `cataclysm.big` | 76.9 MB | `RBF1.23` archive, 16,528 files (128 MB unpacked), 16,081 LZSS-compressed. Same format as HW1 (32-byte aligned TOC entries; HomeworldSDL later packed it to 27 bytes). Full listing: `docs/inventory/cataclysm_big_toc.tsv`. |
| `Music4Cat.wxd` | 68.8 MB | `WXD1` music bank. |
| `BeastSpeech.vce`, `SectSpeech.vce` | 16.7 / 43.1 MB | `VCE0` speech banks. |
| `Movies\*.bik` | ~200 MB | Bink animatics. |
| `default_keys.cfg`, `devstats.dat`, `*.cfg` | small | text config / tuning tables. |

Inside the big file: per-race ship data (`race_beast`, `race_sect`, `race_taidan`, `race_kushan`, `race_traders`,
`race_derelict`, `race_p1`) as `.lif` textures (12k), `.geo` meshes, `.mex` mesh extras, `.peo`/`.shp` ship stats,
`.script` tweak files, `.mission`/`.level` files, `etg` effects, `btg` backgrounds, `feman` front-end screens,
`.nis` in-game cinematics, fonts, LUTs and so on.

### The Homeworld 1 source code (the big accelerator)

Relic released the Homeworld 1 source in 2003. The community port lives at
<https://github.com/HomeworldSDL/HomeworldSDL> and is cloned read-only to `reference/HomeworldSDL`
(215 C files, ~340k lines). Cataclysm is a modified HW1 engine, so most of `cataclysm.exe` is HW1 code
with Barking Dog's changes.

A first, crude check: of 10,112 string literals in the HW1 source, 2,958 (29%) appear verbatim in
`cataclysm.exe`. Most of the misses are debug/assert strings compiled out of a release build. Files that
are mostly runtime strings match strongly: `MultiplayerGame.c` 78%, `TitanInterface.cpp` 76%,
`Tutor.c` 70%, `StatScript.c` 66%, `Universe.c` 53%. Per-file numbers: `docs/inventory/hw1_string_overlap.json`.

This changes the job from "decompile 3 MB of x86 from scratch" to "**diff Cataclysm against HW1
function by function, and decompile only what Barking Dog changed or added**".

## 2. Tooling (set up)

- Ghidra 12.1.4 in `tools/ghidra_12.1.4_PUBLIC` (Java 21 already on the machine).
- ghidra-mcp 7.0.0 built from source in `tools/ghidra-mcp` (Gradle, no Maven needed).
- Ghidra project `ghidra/HWC.gpr` with `cataclysm.exe` and the four `rgl*.dll` imported and auto-analysed.
- `tools/start-ghidra-mcp.ps1` starts the headless ghidra-mcp server on `127.0.0.1:8089`;
  `.mcp.json` at the repo root wires the MCP bridge into Claude Code for this folder.
  Verified: the server reports 10,982 functions in `cataclysm.exe`, and decompiling `FUN_004254d0`
  (found via its error string) gives HW1's `bigFileLoadAlloc` almost line for line, including the
  32-byte TOC stride.
- `tools/scripts/`: `peinfo.py` (PE headers/imports), `strs.py` (source-path strings), `biglist.py`
  (big-file TOC), `overlap.py` (HW1 string overlap).

## 3. Staged plan

Each stage ends with something runnable or checkable. Stages 1–2 are analysis; porting starts at 3.

**Stage 0 — Foundations (this thread).** Inventory, Ghidra project, MCP server, plan. Done when the
MCP server answers queries about `cataclysm.exe`.

**Stage 1 — Map the binary to HW1.**
1. Find the real entry point behind the protection wrapper and the CRT (`WinMain` → `main.c`).
2. Apply HW1 types to Ghidra: import HW1 headers (`Types.h`, `SpaceObj.h`, `Universe.h`, `ObjTypes.h`, …)
   as a data-type archive, adjusted to MSVC 6 / 32-bit layout.
3. Name functions by anchors: string references, the KAS function table, the command/tweak tables
   (`StatScript`/`Tweak` scriptEntry tables name thousands of globals), `Switches`, keybindings, the
   `TitanInterface` C++ vtables, CRT/library signatures (MSVC 6 FID).
4. Bulk match the rest against HW1: build HW1 (original Relic release, MSVC-like settings) and use Ghidra
   BSim / function hashes, then hand-check. Output: a table of every function in `cataclysm.exe` tagged
   `same-as-HW1`, `modified`, `new`, or `library`.
   Done when ≥90% of `.text` is named and classified.

**Stage 2 — Recover source.** Write C-like recovered source per original module (`recovered/src/game/*.c`)
using HW1 files as the starting text and applying Barking Dog's changes from the decompile. Recover the
Cataclysm-only systems: Beast/infection, worker/mimic, ship upgrades, siege cannon, time compression,
the new races' ship modules, and the missions (HW1 compiled KAS mission scripts to C; Cataclysm's 17
missions will be in the exe the same way). This tree is the recovered-source deliverable in its own right.

**Stage 3 — Rust foundation (can start in parallel with stage 2).** Data layer first, because it can be
tested against the real files with no game logic: big-file reader (incl. LZSS), `.lif`, `.geo`/`.mex`,
`.peo`/`.shp`, `.script` tweak parser, WXD/VCE audio banks, `.mission`/`.level`, fonts. Each loader gets
a round-trip/golden test over every matching file in `cataclysm.big`.

**Stage 4 — Simulation core, deterministic.** Port the universe update (`Universe`, `UnivUpdate`,
`SpaceObj`, `Physics`, `Collision`, `Damage`, `Gun`, `Formation`, `Tactics`, `CommandLayer`, `AI*`, ship
modules, `KAS`). Keep HW1's fixed tick (`UNIVERSE_UPDATE_RATE`) and its `ranRandom` generator so runs are
deterministic. Verify by recording original-game command streams (Cataclysm's demo record/playback
files) and comparing world state tick by tick against an instrumented original.

**Stage 5 — Presentation.** Renderer on wgpu (replacing rgl + its three back ends), keeping HW1's
camera, LOD, ETG effects, BTG backgrounds, trails, nebulae; front end (`FEMan`/`FEFlow`) screens;
audio via cpal with the original mixer/codec (`fquant`, `fqcodec`) ported; Bink replaced by a decoder
crate or a one-off conversion step. Input/keybindings via winit.

**Stage 6 — Modernisation.** Widescreen and high-DPI, uncapped render rate with interpolation over the
fixed sim tick, modern multiplayer (replace WON Titan with direct/lobby networking over the same
deterministic lockstep), mod-friendly data overrides, cross-platform builds.

## 4. Proposed Rust workspace layout

```
HWC_Modern/
  Cargo.toml                  # workspace
  crates/
    hwc-formats/              # big, lzss, lif, geo, mex, peo/shp, script, wxd, vce, mission, font (no game logic)
    hwc-math/                 # vector/matrix/fastmath, bit-exact with the original where sim depends on it
    hwc-sim/                  # universe, objects, physics, collision, combat, commands, AI, ships, research
    hwc-kas/                  # mission scripting runtime + recovered missions
    hwc-render/               # wgpu renderer: meshes, LOD, ETG, BTG, trails, HUD primitives
    hwc-audio/                # mixer, speech/music banks, sound events
    hwc-ui/                   # front end, taskbar, menus, overlays
    hwc-net/                  # lockstep command transport (replaces Titan/WON)
    hwc-game/                 # glue: game loop, save/load, options, single/multiplayer flow
  apps/
    cataclysm/                # the executable
    hwc-tools/                # CLI: extract big files, dump formats, compare sim traces
  recovered/                  # recovered C source tree mirroring C:\hwexp\src (stage 2 output)
  reference/                  # read-only third-party reference (HomeworldSDL); not shipped
  ghidra/                     # Ghidra project (large, git-ignored; exported .gzf snapshots kept)
  tools/                      # Ghidra install, ghidra-mcp, scripts
  docs/
```

Crate boundaries follow the original module groups, so a recovered `.c` file maps to one Rust module and
can be checked side by side. Game data is always read from the user's own install path at runtime; none
of it is committed.

## 5. Open decisions

- Whether to build the original Relic HW1 source release with MSVC 6 / a close compiler to get the best
  BSim matches (recommended; the HomeworldSDL fork has drifted from the 1999 code).
- Licensing (owner intends to publish, 2026-10-18). Constraints found, not legal advice:
  - The HW1 source EULA (`reference/HomeworldSDL/LICENSE.txt`) allows use and modification for
    non-commercial purposes only, formally limits distribution to RDN members (defunct; HomeworldSDL
    publishes openly anyway), assigns ownership of modifications to Relic, and grants no trademark rights.
  - Cataclysm's own code (Barking Dog/Sierra) has no source licence at all. Homeworld rights now sit with
    Gearbox, which Take-Two owns; Take-Two DMCA'd the re3/reVC GTA decompilation projects in 2021.
  - **Decided 2026-10-18: Rust only.** Publish the Rust engine; `recovered/` and `ghidra/` stay private.
  - Lowest-risk publishing shape: publish the Rust engine as a reimplementation that needs the player's own
    copy of the game, ship no game data, stay non-commercial, avoid "Homeworld" in the project name, and
    keep the raw decompiled/recovered C tree (`recovered/`) private as a research artifact.
- Repository hosting: `D:\Git\HWC_Modern` is not a git repo yet.

## 6. Progress log

- 2026-10-18 — Stage 0 done. Stage 1 started:
  - `reference/hw1-2003/` = first commit of HomeworldSDL (2003-10-12), the closest available copy of
    Relic's original release. It also contains the `rgl` renderer source (software, D3D and Glide back
    ends) and `src/Generated/` (KAS missions compiled to C), so the rgl*.dll files can be matched too.
  - The entry point is a copy-protection wrapper in `.cms_t` with its own CRT; it is not analysed or
    ported. Game code it reaches only indirectly was missed by Ghidra's first pass.
  - Recovered functions: 10,982 → 11,174 (aggressive instruction finder) → 13,332 (seeding 2,343
    table/immediate pointers into a 630 KB block at 0x5d1930–0x66b32f). That block is compiled KAS
    mission code (scope setup + hashed calls), i.e. Cataclysm's missions.
  - All HW1 headers (game + ships) preprocessed with MSVC x86 and imported into Ghidra: 5,674 types
    (`Ship` 1136 B, `ShipStaticInfo` 844 B, `SpaceObj` 116 B as HW1 defines them; Cataclysm's layouts
    will differ and get fixed per struct). Pipeline: `tools/scripts/filter_pp.py` + `import_types.py`,
    working files in `recovered/types/` (private).
  - Still unreached: ~1 MB of `.text` in gaps with orphan instructions; next is the KAS function
    table, tweak/script tables and string anchors to name functions, then BSim against a 32-bit build
    of `reference/hw1-2003`.
- 2026-10-18 (later) — Stage 1 game-code naming pass:
  - Ghidra now sees 16,503 functions (fixed two functions wrongly marked no-return, filled padded gaps).
  - Six workers compared decompiled code with the HW1 source file by file. In the game region (src/game,
    5,156 functions) 3,624 now carry HW1 names, each tagged same/modified; 665 are logged as Cataclysm-only.
    Per-file reports and logs are in `recovered/naming/` (private).
  - Cataclysm-only game modules identified: Beast/infection, Salvage, PopupMenu, OverlayIcons, a new camera
    system, time compression, ship linking/docking rewrite, research-module tech tree, upgrade builds.
  - Still to do in stage 1: the platform layer (src/win32), ship modules, the Cataclysm-only modules, the
    compiled missions, and re-running the automatic matcher with the new anchors.
- 2026-10-18 — Stage 3: `hwc-formats` reads every data format in the game with tests over the full data
  (archive/LZSS, LIF, GEO/PEO, MEX, MAD, BTG, HFF fonts, FIB screens, HSF lighting, NIS + NIS scripts, tweak
  files and ship stats, audio banks/LUTs/music/speech incl. the transform codec). Format notes in `docs/formats/`.
- 2026-10-18 (evening) — naming coverage by region: game code 4,018 / 5,063 functions named (79%), platform layer
  782 / 1,255 (62%), ship modules 589 / 601 (98%). The full game-code file layout (150 source files incl. 17
  Cataclysm-only ones such as Beast, Salvage, PopupMenu, MainOverlay, FogOfWar, Shockwave, ShipUpgrade, UserKeys)
  and the ship module table (21 Cataclysm-only ship modules: Acolyte, ACV, Mimic, Leech, RammingFrigate, Sentinel,
  Slipgate, Worker, NomadMoon, Dreadnought, ...) are in `recovered/naming/` (private).
  Remaining stage 1 work: compiled missions (2,256 functions, to be turned back into mission scripts), the
  library region (CRT, WON/Titan, STL), and the leftover unnamed functions.
- Correction: the copy-protection wrapper lives in the `.cms_t`/`.cms_d` sections at VA 0xAFA000+, not at
  0x6FA000 (that was an RVA/VA mix-up). 0x6FA000–0x71F000 is ordinary library code (MSVC6 C++ std library,
  CRT tail) and ~2,700 compiler-generated exception-handling stubs. All Win32 API calls go through one
  pointer redirected by the wrapper, so the OS API behind each call is inferred from its arguments.
- Libraries identified (details private in `recovered/libraries.md`): MSVC6 debug CRT, IJG libjpeg 6, WON Titan
  SDK (WONMsg/WONCommon/WONCrypt/WONAuth/sockets), Crypto++ 3.x, WON AdApi, ImageMagick (GIF/BMP only). Only the
  CRT, JPEG load/save and the Titan socket/message layer matter to the port; the rest is dropped.
- Determinism traps for stage 4: x87 `__ftol` returns 0x80000000 for NaN/out-of-range where Rust `as`
  saturates, and MSVC `qsort` is not stable — both need exact emulation where the simulation depends on them.
- 2026-10-18 (night) — missions: all 17 single-player missions and the tutorial decompile to KAS-style scripts
  (private, `recovered/missions/`), with all 2,085 mission functions renamed kas2c-style in Ghidra. KAS labels are
  a case-insensitive ELF-style hash; 974 of 2,706 label hashes recovered exactly, 369 more as guesses. The exe
  embeds Cataclysm's own kas2c function table (486 entries), which corrected ~200 runtime (kasf*) names.
  Open: remaining label names, and a recompile-and-compare check of the translator.
- Stage 4 foundation: `hwc-math` reproduces the original's float behaviour (x87 at 53-bit precision, MSVC6
  evaluation order, x87 FSIN/FCOS, table sqrt), the 27-stream RNG, MSVC6 `__ftol` (64-bit FISTP; NaN -> 0) and
  `qsort`, Cataclysm's spline changes, and the 16 Hz universe / 240 Hz task timing with the new game-speed factor.
  Still missing: FPATAN/FPTAN/F2XM1 emulation (camera, cloak, Mimic).

## 7. Distribution model (decided 2026-10-18)

We ship only our own Rust code. A first-run importer (`hwc-import`, planned) takes the player's own Cataclysm
install, reads everything we need from it — `cataclysm.big`, music/speech banks, movies, and the mission logic
extracted from `cataclysm.exe` — and writes it into the new game's data directory in whatever form the engine
wants. Nothing from the original game is ever distributed with the project; tests that need game data read the
player's install and skip when it is absent.

## 8. Before anything is published

The repository is local-only. Before a first public push, audit and rewrite history so it contains only our own
code and notes:
- commit `fd0b09b` added Bink codec tables copied from `binkw32.dll` (removed in a later commit; history must go);
- `docs/inventory/` lists the archive's file names and HW1 string-overlap stats — keep or drop deliberately;
- `crates/hwc-kas/src/generated/` holds the kas2c runtime function table (names and parameter types read from
  the exe) — decide whether that interface description is acceptable, or generate it at import time;
- format/sim docs describe layouts and offsets found by reverse engineering (normal practice; keep);
- search for any embedded game strings, tables or asset bytes in tests and examples.
- 2026-10-19 — decompile map essentially complete: game code 98.5% named (4,985/5,063), platform 100%, ship
  modules 98%; the remaining missions-region entries are false function starts. Per-subsystem porting notes
  (docking/construction, sensors/HUD, research/upgrades, universe update, single-player flow, particles/ETG,
  render, UI, keys/KAS/NIS) are in `recovered/naming/n2/*-report.md` (private).
