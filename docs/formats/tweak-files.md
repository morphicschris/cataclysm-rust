# Tweak files (text configuration)

Reader: `hwc_formats::script` (`crates/hwc-formats/src/script.rs`). Test over the real data:
`crates/hwc-formats/tests/script_real.rs` (`HWC_EXTRACTED_DIR=<extraction root>`, run with
`--nocapture` for the survey tables).

## Syntax as the engine accepts it

The original engine has one line-splitter shared by every tweak loader; the per-keyword setter then
`scanf`s the value string. There is no real tokenizer, so the rules are:

| Rule | Detail |
| --- | --- |
| Lines | One entry per line. CRLF in all shipped files. The line buffer is 650 bytes; no shipped line comes close. |
| Headers | A line whose **first column** is `[` or `]` is skipped. `[Name]` is cosmetic: all keys in a file are global. `.shp`/`.lod` use it for the object name; `.script` for the subsystem name; `.mission`/`.level` for the mission title. One `.shp` (`race_derelict/dslipgatelarge.shp`) has no header. |
| Comments | From the first `;` **or** the first `//` to end of line. No quoting/escaping, so a `;` inside a display string truncates it. `/* */` is *not* a comment (see below). |
| Keyword | Leading blanks skipped; keyword ends at the first space, tab or `=`. Case-sensitive exact match. |
| Value | Rest of the line, leading/trailing blanks trimmed. `key=value` works; `key = value` yields the value `= value`. Empty values are allowed. |
| Repeats | Each line calls its setter, so for scalars the last line wins; list keywords (`DockPoint`, `NavLight`, `trailColor`, `flyPastDist ...`, `Resources`, `IncludeShips`, ...) are meant to repeat. |
| Array keywords | Brackets are part of the keyword text: `thruststrength[TRANS_UP]`, `specialDoorOffset[R1][Carrier]`, and Cataclysm's per-difficulty `AIPLAYER_UPDATE_RATE[AI_BEG]`. |
| Blocks | `{` and `}` on their own lines are entries. In `.shp` they delimit `GUN n` blocks (271 blocks, all matching `NUMBER_OF_GUNS`); in `gametypes.script` they delimit `GAME <name>` blocks. |
| Fields | Setters treat commas and blanks as separators. Numbers: decimal and negative ints, floats with or without fraction, exponent (`5e-5`) or trailing `f` (`240.0f`); `0x` hex only occurs in comments. `?` means "unknown" and is read as 0 (mission files, `??.lod` in the template ship). |
| Includes | None. `IncludeShips` etc. in mission files are ordinary keywords. |
| Encoding | 8-bit; localised string tables use Windows-1252 accents. Decoded as Latin-1 so every byte round-trips. |

The Rust model is `ScriptFile { sections: Vec<Section { name, line, entries: Vec<Entry { key, raw,
values: Vec<Value>, line }> }> }` with `Value = Int | Float | Word | Unknown`. `raw` keeps the exact
value text for setters that scan it in their own way (`%s %s %f` style).

## File types in `cataclysm.big` (16 528 files)

Text detection: no control bytes other than TAB/LF/CR/^Z. Every text file parses without error.

| Ext | Files | Kind | Entries | Notes |
| --- | ---: | --- | ---: | --- |
| `.script` | 115 | text | 15 793 | 83 tweak files; 32 are C-preprocessed (`#line`) NIS/animatic event scripts in `movies/` and `nis/` (`at <time>` / event lines) with their own grammar |
| `.shp` | 173 | text | 22 737 | ship, derelict, missile, mine and resource statics |
| `.lod` | 172 | text | 5 036 | LOD lists |
| `.dist` | 322 | text | 1 684 | resource distributions (`Asteroid0  3`) |
| `.missphere` | 128 | text | 7 106 | MissionMan output |
| `.mission` | 29 | text | 1 644 | MissionMan output |
| `.level` | 12 | text | 378 | MissionMan output |
| (none) | 36 | text | 1 539 | MissionMan object files inside `singleplayer/missionNN/` (`general`, `ai`, `player`, ...) |
| `.plug` | 60 | text | 358 | credits screens |
| `.cred` | 2 | text | 12 | credits entry screens |
| `.ebg` | 255 | text | 23 248 | ETG effect sources, all C-preprocessed; separate grammar (keyword lines still split) |
| `.mif` | 17 | text | 210 | build leftovers (`TEXTURES += x.lif`), not read by the game |
| `.txt` | 14 | text | 16 825 | map `description.txt`, `fonts/english.txt`, and `bigcontentsculled.txt` (archive file list) |
| `.lst`, `.list`, `.bat` | 1 each | text | | animatic list, autodemo list, a stray batch file |
| `.lif` 12 096, `.geo` 967, `.mex` 967, `.peo` 762, `.hff` 102, `.jpg` 85, `.btg` 46, `.tga` 42, `.hsf` 34, `.mad` 29, `.fib` 21, `.lut` 15, `.nis` 14, `.bnk` 4, `.bin`/`.dem`/`.doc`/`.kp`/`.ll`/`.wxh` 1 each | | binary | | not tweak files (`.peo`, `.mad`, `.hff` are binary, not text) |

## Ship keywords: Cataclysm vs Homeworld 1

The HW1 set is every keyword the HW1 loader binds for statics files (generic ship table, per-ship-type
tables, derelict/missile/mine/resource tables, dock/nav-light/salvage lists, trail table, `GUN`
blocks); it is embedded as `HW1_SHIP_KEYWORDS` / `HW1_GUN_KEYWORDS`. `ShipKeywordSurvey` compares
against it. 298 distinct top-level and 35 gun-block keywords occur across the 173 `.shp` files.

### Cataclysm-only top-level keywords (real stats)

| Keyword | Files |
| --- | --- |
| `forceFieldProtectionDistanceScale` | 140 |
| `fowRadius` | 138 |
| `explosionType` | 124 |
| `supportUnits`, `providesSU`, `repairDroids`, `externalBuild` | 117 each |
| `TimeShockedByEMP` | 92 |
| `pilotViewOffsetX/Y/Z` | 88 |
| `UpgradeByDocking` | 43 |
| `upgradeTimeModifier` | 40 |
| `beastAttackCascadeCost`, `beastAttacksRequired`, `beastAttackConvertTo` | 37 (Beast infection) |
| `bUseRayTriangleIntersectionForHitLocation` | 24 |
| `RKOEatMeTime`, `RKOGetsMissiles` | 23 (frigates and larger: Somtaaw `s*`, Kushan `k*`, Taiidan `t*`, P1) |
| `NUMBER_OF_ANIMS` (18), `ANIM` (17) | animation lists, e.g. `bacolyte.shp` |
| `linkDistance` | bacolyte, sacolyte, smimic, decclesiam |
| `regenerateRUs`, `regenerateRUsIfUnder`, `regenerateRUrate` | bmothership, smothership, dbeastmothership, trghostship |
| `hyperspaceBoxScale` | bmothership, smothership, dbeastmothership |
| `salvageFlyToDistance` | bworker, sworker, ksalcapcorvette, tsalcapcorvette, p1standardcorvette |
| `repairDistance`, `repairCone`, `repairBeamWidth`, `jackRetractTime` | bworker, sworker |
| `RamDamage`, `RamPushTime`, `RamWaitAfterPushTime`, `cuttingBeamDamagePerSecond`, `cuttingBeamUpgradedDamagePerSecond` | brammingfrigate, srammingfrigate |
| `EMP_Range`, `EMP_Max_Combine_Distance` | bacv, sacv |
| `MultiBeamRangeRatio` | bmultibeamfrigate, smultibeamfrigate |
| `leechHPsPerSecond`, `leechRUsPerSecond` | sdreadnought |
| `scaleCap` | brecon, srecon (HW1 spells this `N-LIPS`, which 125 Cataclysm files also use) |
| `smallExplosion`, `bigExplosion` | kadvancesupportfrigate, kheavyinterceptor |
| `badAI` | battackbomber, bcloakedfighter |
| `trail2Width/Height/Length/Style/ScaleCap` | scarrier |
| `NUMBER_OF_ASTEROID_PIECES`, `AsteroidBreakInfo` | resources/asteroids/asteroid2-4 (replaces HW1 `breakinto[...]`) |

### Cataclysm-only gun-block keywords

`GunSize`, `StartUpgradeLevel`, `EndUpgradeLevel`, `DamageMult` (68 files each; the upgradeable
weapon system — HW1's `DamageLo`/`DamageHi`, `BulletType`, `SoundType` are no longer used at all),
`FireTime2`, `BulletSpeed2` (28 files; second fire mode), `fireEffect`, `hitEffect` (dkushancarrier,
trjunk_lgun, trjunk_sgun).

### Not real keywords (typos and stray text the engine silently ignores)

* Case/typo variants: `IMMOBILE` (trjunkyardhq; HW1 binds `immobile`), `resourceValue` (dcrystal0/1;
  HW1 `resourcevalue`), `canTargeMultipleTargets` (trghostship), `harvestableByMultiplyShips`
  (dustcloud2, nebula0), `fightmanAIP` (smimic). Unless Cataclysm's loader is case-insensitive or
  added these spellings, these lines have no effect.
* Comment text missing its `;`: `Relative` (14 files, "Relative frequenciesfor:"),
  `animation(1-yes,0-no)/`, `animation(when`, `format` ("format is NavLight ..."), `Explosions`,
  `l0`.

### HW1 keywords no Cataclysm file uses (130)

Includes all `specialDoorOffset[...]`, `bulletRange[n]`/`minBulletRange[n]`, salvage points
(`NUMBER_OF_SALVAGE_POINTS`, `SalvagePoint`, `NEED_BIGR1/2`, `WILL_FIT_CARRIER`), dock overrides,
the R1/R2 dock distances, `sv*` ship-view ratings, gun `DamageLo/DamageHi/BulletType/SoundType`, and
the 4th trail set. The full list is printed by the test.

## Ambiguities / open questions

* **Booleans.** HW1's bool reader only accepts `TRUE`/`YES` (case-insensitive); its digit test
  compares against byte values 1..9 rather than `'1'..'9'`, so a literal `1` reads as false. The Rust
  `Value::as_bool` treats nonzero numbers as true (the evident intent); verify against Cataclysm's
  binary if a file depends on it.
* **`;` inside strings.** `german.script` `strToJoin` contains `;` and is truncated by the HW1 rules.
  Whether Cataclysm's string-table loader differs is unverified.
* **`/* ... */` in `demoresearchdepend.script` and `etg/*.script`.** Not comments; the inner text
  lines become unbound keys (harmless).
* **Section scoping.** The engine ignores `[headers]`; multi-header files (missions) rely only on
  keys. The reader keeps sections, but `ScriptFile::get` searches all of them like the engine.
* **Preprocessed scripts.** `movies/*.script`, `nis/*.script` and all `.ebg` contain `#line`
  markers and parenthesised expressions; they belong to the NIS and ETG parsers and only get a
  line-level split here (`ScriptFile::dialect() == Dialect::Preprocessed`).
* **Case sensitivity in Cataclysm.** Assumed identical to HW1 (`strcmp`); the typo list above
  depends on that.
