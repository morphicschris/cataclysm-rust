# Importing the original game (`hwc-import`)

The project ships only Rust code. On first run the importer reads the player's own Homeworld:
Cataclysm install and builds the game's **data directory**. The install is opened read-only; the
importer refuses an output folder that is inside the install (or contains it).

```
hwc-import <original install dir> <output data dir> [--verify] [--force] [-q]
hwc-import --verify-only <output data dir>
```

The launcher can call the same code as a library: `hwc_import::import(install, out, opts, &mut |p| ...)`
and `hwc_import::verify(out, opts, &mut |p| ...)`, with `Progress` events for stages, per-file
start/skip, byte counts, asset counts and warnings.

## Reading the data (`hwc_formats::GameData`)

The engine reads the result through `hwc-formats`, so it does not depend on the importer:

* `hwc_formats::GameData::open(path)` accepts an imported data directory (has `manifest.json`),
  or, for development, an original install directory (has `cataclysm.big`; read in place) or a
  bare `.big` file. `GameData::from_env()` uses `HWC_DATA_DIR`, else `HWC_GAME_DIR`.
* `read(game_path)` / `read_with_origin` read the archive namespace, overrides first (imported
  layout only has overrides); `archive()` is the base `.big`.
* `music_path()`, `beast_speech_path()`, `sect_speech_path()`, `install_file(name)`: the stream
  files; `movies_dir()`, `movie_path(name)`; `missions_dir()` and `mission_ir(name)` (imported
  only; for an install, `exe_path()` gives the executable to extract from);
  `bink_tables()` returns `Imported(<data>/bink_tables.bin)` or, for an install,
  `Dll(<install>/binkw32.dll)`; `bink_tables_path()` is the imported file only.
* `hwc_formats::datadir::DataDir` is the manifest-based reader underneath (also re-exported as
  `hwc_import::DataDir`); `hwc_formats::manifest` holds the manifest types;
  `hwc_formats::check::check_asset` is the parse-by-extension check `--verify` uses.
* `hwc_audio::DataSource::from_game_data(game)` loads the sound data from any of these layouts
  (`DataSource::from_env` also honours `HWC_DATA_DIR`).

## What the install must contain

Matched ignoring case. Missing or malformed files are all reported together.

| File | Check | Required |
|---|---|---|
| `cataclysm.big` | `RBF1.23` signature, table of contents readable | yes |
| `cataclysm.exe` | `MZ` header; SHA-256 looked up in the table of known builds | yes |
| `Music4Cat.wxd` | `WXD1` | yes |
| `BeastSpeech.vce`, `SectSpeech.vce` | `VCE0` | yes |
| `default_keys.cfg` | present | yes |
| `Movies\*.bik` (Animatic01–18, bdogintro, sierra) | `BIK` signature | yes |
| `binkw32.dll` | source of the Bink codec tables (see below) | no: without it movies are unavailable |
| `Cataclysm.cfg` (player settings), `devstats.dat` | none | no |

Known executables (`hwc_import::install::KNOWN_EXES`):

| id | SHA-256 | size | mission reference |
|---|---|---|---|
| `retail-2000-08-25` | `b4ba4b044fcdad04fbbe0445dcf806fca3f03bfaaaf86978eda29baaf5128da1` | 5,359,747 | 18 missions, extractor `hwc-kas-extract/0.1.0+ir1` |

Each known build carries the SHA-256 of every mission's extracted IR (`missions/<name>.kasb`),
taken with a given extractor id. Import and `--verify` compare the extracted missions with it; a
missing, extra or differing mission, or IR made by a different extractor id, is a **warning**
listing the details, and the manifest records `missions.verified_exe: false`. When the extractor
changes the IR on purpose, re-take the digests (import the known build and copy
`missions[].sha256` from the manifest into `KNOWN_EXES`); the unit test
`reference_digests_match_current_extractor` fails until that is done.

An unknown build is not an error: the data is imported, a warning is shown, mission extraction is
still tried, and the manifest records `known_version: null` and `missions.verified_exe: false`.

## Data directory layout

```
<data>/
  manifest.json          versioned manifest (below)
  base/cataclysm.big     the original archive, unchanged: the base layer of the archive namespace
  overrides/             loose files that replace archive entries (empty after import; for mods)
  audio/Music4Cat.wxd    streamed music (index: soundfx\music4cat.wxh inside the archive)
  audio/BeastSpeech.vce  speech banks (phrase tables: soundfx\*phrase.lut inside the archive)
  audio/SectSpeech.vce
  movies/*.bik           Bink movies, copied as-is
  config/default_keys.cfg, config/Cataclysm.cfg, config/devstats.dat (when present)
  missions/<name>.kasb   mission IR extracted from cataclysm.exe (hwc_kas::binary form)
  bink_tables.bin        Bink codec tables extracted from binkw32.dll (hwc_video::TABLES_FILE_NAME)
```

Output file names are the canonical spellings above, whatever their case in the install.

### Why keep the `.big` and add `overrides\`

The archive holds ~16,500 files. Keeping it whole (rather than unpacking it) keeps the import fast
(one 77 MB copy), keeps the data byte-identical to what the original engine read, and needs no
extra disk space for decompressed copies. Modding works by layering:

* Any archive name (`race_sect\smothership\rl0\lod0\smothership.geo`, `soundevent.script`, ...)
  is looked up first under `overrides\` as a loose file at the same relative path, then in
  `base\cataclysm.big`.
* Matching ignores ASCII case and accepts `/` or `\`, like the original's archive lookup, so the
  same mod works on case-sensitive file systems.
* The importer never touches `overrides\` beyond creating it, so re-import keeps mods. `--verify`
  parse-checks override files too (an override shadows the archive entry of the same name, so
  the effective file is what gets checked).

To unpack the archive for editing, use `hwc-tools big-extract` and copy the files you change into
`overrides\`.

### Movies

Movies are copied unchanged and decoded at run time by `hwc-video`. The decoder needs RAD's
codec tables, which the project does not ship: the importer reads the install's `binkw32.dll`,
runs `hwc_video::import_tables` on it and writes the result to `bink_tables.bin` (about 10 KB),
which `hwc_video::BinkTables::load` reads. The file is listed in the manifest like the copied
files (kind `codec_tables`, `source` = the DLL with its SHA-256) and is re-extracted only when the
DLL's hash changes or the output is missing, changed or no longer loads. If the DLL is missing
or the tables cannot be found in it, the import continues with a warning that movies are
unavailable, and no `bink_tables.bin` is listed.

### Missions

Cataclysm's missions are compiled into `cataclysm.exe`. The importer runs
`hwc_kas::extract::extract_each` on the executable's bytes and writes each mission with
`hwc_kas::binary::encode` to `missions/<name>.kasb` (`mission01` ... `mission17`, `tutorial1`).
A mission that fails to lower is listed in the manifest with `path: null` and its error in
`missions.errors`; the status is `extracted`, `partial` or `failed`.

## manifest.json

```jsonc
{
  "format": "hwc-data",
  "manifest_version": 1,          // layout version; a different version means re-import everything
  "importer_version": "0.1.0",
  "updated_unix": 1792363150,     // last import that changed something
  "install_dir": "D:\\Games\\Homeworld Cataclysm\\Cataclysm",   // informational
  "exe": { "path": "cataclysm.exe", "size": 5359747, "sha256": "...", "known_version": "retail-2000-08-25" },
  "files": [                      // sorted by path
    {
      "path": "base/cataclysm.big", "kind": "archive",   // archive|music|speech|movie|config|codec_tables
      "size": 76864402, "sha256": "...",
      "mtime": { "secs": ..., "nanos": ... },            // of the output, when written
      "source": { "path": "cataclysm.big", "size": ..., "mtime": {...}, "sha256": "..." }
    }
  ],
  "missions": {
    "status": "extracted",
    "extractor": "hwc-kas-extract/0.1.0+ir1",            // a different value re-extracts
    "exe_sha256": "...", "verified_exe": true,
    "missions": [ { "name": "Mission01", "path": "missions/mission01.kasb", "size": 9801, "sha256": "..." } ],
    "errors": []                                         // omitted when empty
  },
  "overrides_dir": "overrides"
}
```

### Incremental re-import

A file is skipped when its source has the recorded size and mtime (and path) and the output still
has the recorded size and mtime; otherwise it is copied again (hashing while copying). Missions are
re-extracted when the exe hash or the extractor id changes, or an output is missing. The Bink tables are
re-extracted when the DLL's SHA-256 changes or the output is missing, modified or unloadable.
Copies go through `<name>.partial` and are renamed into place; the manifest is written last, atomically, and
only when something changed. Files listed by the previous manifest that are no longer produced are
deleted (only inside `base\`, `audio\`, `movies\`, `config\`, `missions\`, and `bink_tables.bin`). `--force` re-copies and
re-extracts everything and allows importing into a non-empty folder that has no manifest (unrelated
files in it are left alone).

## --verify

1. Re-hashes every file and mission listed in the manifest (size, then SHA-256).
2. Parse-checks every file visible in the archive namespace (overrides first) with `hwc-formats`:
   `.lif`, `.geo`/`.peo`, `.mex`, `.mad`, `.btg`, `.hff`, `.fib`, `.hsf`, `.nis`, `.bnk`, `.lut`,
   `.wxh`, and the text formats (`.script`, `.shp`, `.dist`, `.missphere`, `.level`, `.mission`,
   `.ebg`, `.plug`, `.mif`, `.lod`, `.cred`, `.lst`, `.list`, `.txt`) with the tweak-file
   parser; a `.script` next to a `.nis` of the same name is also parsed as a NIS event script.
3. Resolves every music track of `music4cat.wxh` inside `Music4Cat.wxd`, and every phrase offset of
   `beastphrase.lut`/`sectphrase.lut` inside the matching `.vce`.
4. Decodes every `missions\*.kasb`, and loads `bink_tables.bin` with `hwc_video::BinkTables::load`.
5. Warns (without failing) when the missions differ from the known build's reference digests or
   when there are no Bink tables.

Images (`.jpg`, `.tga`), demo/bin files and extension-less files have no parser and are counted as
unchecked. Exit code 1 if anything fails.
