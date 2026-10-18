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
| `cataclysm.big` | 76.9 MB | `RBF1.23` archive, 16,528 files (128 MB unpacked), 16,081 LZSS-compressed. Same as HW1 except the TOC entry is the naturally aligned 32-byte struct, not packed 27 bytes. Full listing: `docs/inventory/cataclysm_big_toc.tsv`. |
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
