# Saved games in the original Cataclysm

How `cataclysm.exe` saves and loads games, worked out from the binary (Ghidra, function names follow HW1)
and compared with HW1's `SaveGame.c`, `GamePick.c` and `KASFunc.c`. Addresses are virtual addresses in
`cataclysm.exe`. Where a global has no recovered name it is given by address.

The format stayed HW1's chunked raw-memory dump. Cataclysm adds about a dozen kinds of state, puts the
mission-start autosave off until the end of the tick, and removes HW1's quick save keys.

## 1. Version number and what `SaveGame` writes

`SaveGame(filename)` is at `0x004f8db0`. It opens the file with flags `0x50` (write, return NULL on
failure). The open handle is `0x008fcd48` and the write-error flag is `0x008fcd4c`. If any `fwrite` fails,
the partly written file is deleted and the function returns 0. On success it returns 1.

**Version: `0x10ad0027`.** This is the same `SAVE_VERSION_NUMBER` as HW1, so the version check alone
cannot tell a HW1 save from a Cataclysm save. `VerifySaveFile` (`0x004f9200`) returns 0 for OK, -1 when the
file can't be opened or read, and -2 for a bad version. The callers show `strErrorInvalidSaveGameFile`
for -1 and `strErrorInvalidSaveGameFileVersion` for -2.

### Chunk encoding (unchanged from HW1)

Each record is `u32 type, u32 size, size bytes`:

- `0x20000000`: info number, always 4 bytes (`SaveInfoNumber` `0x004f8c00`).
- `0x80000000 | id`: fixed-size struct (`SaveStructureOfSize` `0x004fae30`).
- `0x40000000 | id`: variable-size struct.
- `0x40000d00`: string (`Save_String` `0x004facf0`).
- `0x10000000`: linked list of space objects, written as a count followed by registry indices.

Before an object is written, its pointers are swapped for indices: space objects through the
SpaceObjRegistry (`SpaceObjRegistryObjPresent`), blobs through the BlobRegistry, list members through
`ConvertPointerInListToNum`. Everything else in the struct is copied byte for byte.

### Order of writes (Cataclysm)

1. The version dword (written directly).
2. Pre-game block. `PreLoadGame` reads this back before the level is built.
   - `singlePlayerGame` flag (`0x008e9b4c`).
   - Current music track (`musicEventCurrentTrack`).
   - `universe.numPlayers` (`0x00a0c878`, low 16 bits).
   - Player names, `0x90` bytes at `0x00ade980`.
   - For each of 8 players: race (player struct +0x14, players start at `0x00a0c880`, stride `0x6530`),
     then team colour base and detail (`0x00a583a0` / `0x00a583a4`, stride `0x30`).
   - Single player: `singlePlayerGameInfo.currentMission` (`0x00aaf900`) and the float at `0x0088a0cc`.
     That float is the skill-level build multiplier (see the `spSkillLevelModifiersGet` notes). It is new in
     Cataclysm; HW1 saves only the mission number here.
   - Multiplayer: `tpGameCreated` (`0x0087bef0`, `0x2b0` bytes).
   - `universeSaveEverythingNeeded` (`0x00537f30`), skipped in the tutorial (`0x008e9b50 == 1`). It writes
     the marker `0xf1a5deed`, then one bit-packed "needed" word for each ship type of each of the 7 races
     (ShipStaticInfo stride `0x458`), then "needed" flags for derelict, resource and dust/gas types.
     When the game is loaded, these say which static data and meshes to load. The level `.mission` file
     itself is not read again.
3. **New:** every ship's damage effects are stopped (`dmgStopEffect(ship, 3)` over the ship list
   `0x00a07e70`). The two registries (`0x00abbaa0`, `0x00abbaa8`) are reset. A `0x9c`-byte block from
   `0x00ad7f40` is written (the per-object render/effect state from `rndSetupObjectRenderStates`, with
   three fields cleared first).
4. Space objects are registered, and every ship in `0x00a07ef4` is added to the second registry.
5. `SaveAllSpaceObjs` (`0x004fa340`) writes a count, then one chunk per object, switching on `objtype`:
   - Ship: `SaveShip` `0x004fb030`, chunk `0x40000010`. The base Ship struct is `0x638` bytes. The
     type-specific ShipSpecifics are appended (`static+0x108` gives their size, minimum 4). The chunk is
     `0x7c` bytes longer when `static+0x88 != 0xff`, and `ship+0x4bc` can add a further variable tail.
     Child chunks follow:
     - A private ShipStaticInfo copy (`0x80000200`, `0x458` bytes) when flag `0x40000` is set.
     - Command info (`0x38` bytes) with a selection.
     - A variable block at `+0x368`.
     - Attack targets (`0x80000030`).
     - A `0x18`-byte-per-entry table at `+0x3a8`.
     - Gun info (`0x80000050`, `0x88` bytes per gun).
     - Dock info.
     - Ships-inside-me (`0x800000f0`).
     - Per-ship single-player info.
     - The static info's per-type save callback (`static+0x14c`).
   - Bullet: `SaveBullet` `0x004fc1c0`, `0x80000011`, `0x164` bytes.
   - Asteroid: `0x80000012`, `0x1b4` bytes.
   - Dust cloud: `0x80000013`, `0x1a8` bytes, plus a nebula-chunk index.
   - Gas cloud: `0x80000015`, `0x19c` bytes, plus `SaveCloudSystem`.
   - Derelict: `SaveDerelict` `0x004fc310`, `0x80000016`, `0x294` bytes.
   - Missile/mine: `SaveMissile` `0x004fc580`, `0x80000018`, `0x258` bytes.
   - Effects (type 4) are never saved; meeting one is a fatal error.
6. `SaveAllBlobs` (`0x004f99a0`): collision blobs (`0x80000040`).
7. **New:** `SaveMiscGlobals` (`0x004f8cc0`). One chunk `0x80004000` built from a table of
   `{pointer, size}` pairs at `0x00888598`:
   - `0x0091065c`.
   - `0x00910664`, the real-time clock base.
   - `0x00879820`, the song number.
8. `SaveUniverse` (`0x004fe770`):
   - The whole universe struct (`0x00a07d80`, `0x3d9bc` bytes, chunk `0x80000020`). The 8 player
     records (`0x6530` bytes each) are inside it at offset `0x4b00`. The current-player pointer becomes
     a player index, and per-player object pointers become registry IDs.
   - The command layer (`Save_CommandToDo` for each entry).
   - The main camera command and its camera stack (`Save_CameraCommand`, `0x80000090` / `0x80000080`).
   - Four object lists stored as ID lists.
   - Six further space-object lists, starting with the ship list.
   - Mine formations.
   - Attack memory.
   - `swSave` and `snSave` (sensors).
   - Resource volumes.
   - Two selections.
   - For each player: the research topic list (`0x800000c0`) and **new** `SavePlayerFogOfWar`
     (`0x004fa970`). Fog of war is written only for the local player and not while recording: a `0x20`-byte
     grid header (`0x80002000`), then the cell buffer (`0x80003000`, sized by `fowGridMemSize`).
   - **New:** `gameStatsSaveCataclysm` (`0x0053b1e0`). Salvaged and beastified tables (`0x1220` bytes each,
     `0x80008000` / `0x80007000`), killed and lost tables (`0x180` each, `0x80006000` / `0x80005000`),
     and one number (`0x009141bc`).
   - The flag at `0x009141c4`.
9. Four ID-to-pointer tables (ship, resource, derelict, missile; `0x009dfbe0`, `0x009dfbf8`,
   `0x009dfbe8`, `0x009dfbd8`).
10. `nebSave_Nebula`, `SaveConsMgr` (one `0x1b48`-byte chunk for each ship in production), `aiplayerSave`.
    In Cataclysm `aiplayerSave` also calls `kasVarsSave` (`0x0048d1a0`), which writes the KAS variable
    table (count, capacity, and `0xc`-byte entries).
11. `SaveSinglePlayerGame` (`0x00517000`):
    - `singlePlayerGameInfo` (`0x54` bytes).
    - Ships in hyperspace.
    - Ships to hyperspace.
    - **New:** the KAS input-lock flags `0x00910208` (bit 1 mouse off, bit 2 main region off with the
      cursor hidden, bit 4 selection cleared).
    - **New:** eight globals at `0x008f1f90`..`0x008f1fb0`, `0x00900604` and `0x0088a040`.
    - **New:** `MimicSaveDisguiseTableInfo`.
12. **New:** `MimicSaveDisguiseTables` (`0x005b4bd0`): `0x17a0`, `0x24` and `0x24` bytes (`0x8000b000`,
    `0x8000a000`, `0x80009000`).
13. **New:** `moSave` (`0x004a8200`): one number (`0x008f1754`) from the objectives/messages module.
14. Single player only:
    - `kasSave` (`0x0048c840`):
      - Hyperspace gates.
      - Mission scope and a second number.
      - `0x30` bytes of scope name.
      - The current watch function as an index.
      - The current team as an index.
      - Skill level `0x0088a0d8`.
      - KAS selections, labelled paths, labelled vectors and labelled volumes, each as used/allocated
        counts followed by the items.
      - `timTimerSave`: count, capacity, and `0x18` bytes per timer.
      - **New:** six `rmCanBuildShip` results for the player (ship type ids `0x15`, `0x18`, `0x16`, `0x13`,
        `0x17`, `0x85`).

      HW1 had two name buffers here; Cataclysm has one `0x30`-byte buffer.
    - `objectiveSave`: fleet intelligence, then objectives (`0x34` bytes plus a description string each).
    - `tmSave`: the tech tables `0x00a49a00` (100 bytes), `0x00a49680` (200) and `0x008a0d40` (200), six
      dialog strings, and a number. HW1's TradeMgr wrote the same layout.
    - `pingSave`.
    - The current level save name string (`0x009100d0`). This is HW1's `CurrentLevelName`: the path of the
      last mission autosave.
15. Tutorial only: `tutSaveTutorialGame`.
16. The current selection (`0x00aafe60`) and the 10 hot-key groups (`0x00ab1e00`, stride `0xfa8`). These
    are saved with `SaveSelection`; HW1 used `SaveMaxSelection`.
17. The background, written inline (HW1 had `SaveBackground`): btg theta, btg phi, and the background file
    name string `0x008d3730`.
18. Lighting, inline: the lighting HSF file name `0x009df640`. On load, a name shorter than 2 characters
    means the default light set.
19. `smSave` (12 sensors-manager numbers) and `ranSave` (all random streams, `0x14` bytes each, from
    `0x00ad50c0` to `0x00ad52dc`).
20. `actorFlagsEnabled` (`0x0088b3f8`) and `smGhostMode` (`0x00900638`).
21. `SaveConsMgrDetermOptional` (HW1 v1.04 addition).

Not saved: the NIS itself, the effect list, and sounds already playing. `LoadGame` clears the effect list
(`listInit(0x00a07e64)`) just as HW1 does.

## 2. File format and location

- **Folders** (relative to the game directory):
  - `SavedGames\SinglePlayer\`
  - `SavedGames\MultiPlayer\`
  - `SavedGames\Training\` (tutorial lessons)
  - `SavedGames\RecordedGames\` (recordings)

  The strings are at `0x00876f48` / `0x00876f64` / `0x00876f98` / `0x00876f7c`. The install has all four
  folders, and all four are empty. There are no original save files to test against.
- **No extension.** The file name is exactly the description the user typed. The list scan uses `*.*`
  (`0x0087716c`) and skips names that begin with `.` or contain `.pkts`, which is the recorded-game packet
  companion file. There is no `.CMP`.
- **No header or metadata.** After the version dword come the pre-game chunks above. No description, date,
  mission name or thumbnail is stored. In single player the only metadata is the mission number inside
  the pre-game block.
- **Typed name** (`gpTextEntryWindowInit` `0x00482d50`): the text box is resized to 30 characters
  (`0x1e`) and has the file-name character filter (`0x100`). `gpSaveGame` (`0x00483190`, `IG_SaveGame`)
  builds the path by appending the name to the folder. A name shorter than 2 characters shows
  `strErrorInSaveGameName` / `strAtLeast2Chars`. If the box is empty, the highlighted list entry is used
  instead.
- **Which folder** (`gpGameWindowInit` `0x00482eb0`, i.e. `FE_GameWindowInit`):
  - `FE_RecordedGameWindowInit` selects RecordedGames.
  - `FE_TutorialGameWindowInit` selects Training and forces tutorial mode.
  - Otherwise, single player in game selects SinglePlayer, or Training if the tutorial is running.
  - With no game running, the folder is SinglePlayer unless the multiplayer screens are up (`0x008f1b80`),
    in which case it is MultiPlayer.
- **List rows** (`gpTitleListLoad` `0x00482b20`): the folder is scanned with `_findfirst`/`_findnext`,
  duplicate names are dropped, and the list is sorted case-insensitively by `compareGamesCB`
  (`0x00482a00`). That sort keeps an entry named `Quick Save` first; this is left over from HW1. The first
  row is preselected. `gpGameItemDraw` (`0x00482920`) prints only `"%s"` of the name, using the list
  font, with a highlight colour when the row is selected. Rows show no date or mission.
- **Overwrite:** if the target file exists, the path is copied to `0x008e95f8` and `OverwriteGamePopup`
  opens. `gpOverwriteYes` (`0x004830f0`) saves (or starts recording when the recording flag `0x008e96c0`
  is set), shows `strSavedGame` (or `strPatchUnableWriteFile` in red on failure), and closes all screens.
  `gpOverwriteNo` (`0x00483180`) simply returns to the list.
- **After a successful save** the message `strSavedGame` is shown, the save screen closes, and in single
  player the game unpauses (`0x00911a04 = 0`).
- **Delete** (`gpDeleteGame` `0x004839d0`, `IG_DeleteGame`, reached through the `DeleteGamePopup*`
  confirmation screens in the .fib): the file is removed with `fileDelete` (and so is the `.pkts`
  companion for recordings). The entry is taken out of the array, the selection moves up one if it was
  the last row, and the list is rebuilt in place.
- **Escape-menu variants** (`gpStartInGameEscapeMenu` `0x00483df0`):
  - `In_game_esc_menu2`: single player.
  - `In_game_esc_menu`: skirmish.
  - `In_game_esc_menu_nosave`: network multiplayer. There is no saving in network games.
  - `In_game_esc_menu_tutorial`: tutorial.
  - `In_game_esc_menu_recording` / `In_game_esc_menu_playing`: recording or playing back a game.

  After single-player game over, `SP_Game_Over` offers `SP_Restart_Level`.
- **Other automatic saves** in `univUpdate` (`0x00547400`), all HW1 debug features:
  - `/SaveMPGames` writes `SavedGames\multiplayer\AS%d` every 200 s of game time, rotating through 0..10.
  - The autosave-debug flag `0x0091a548` writes `SavedGames\AutoSaveDebug%d` (8 rotating files) or
    `SavedGames\AutoSave%f` every 32 ticks.
  - Starting a recording saves `0x009df2c0` (`recPackInGameStartCBSafeToStart`).

## 3. The mission autosave (`SaveLevel`)

HW1's `kasfSaveLevel` saved immediately, to `SinglePlayer\<name>`. Cataclysm splits it in two:

- `kasfSaveLevelRequest` (`0x004928b0`, the KAS `SaveLevel(levelNum, name)`) only stores the request:
  level number in `0x008e9e98`, name in `0x008e9e24`, pending flag `0x008e9e9c = 1`. It is called from
  the compiled mission code (44 sites) and from the console (`consoleCallKASFunction`).
- `kasfSaveLevelDeferred` (`0x004928f0`) is called at the **end of the same `univUpdate` tick**. KAS runs
  inside `aiplayerUpdateAll`. After that the tick processes the command layer, physics, collisions and
  dead-object deletion, and checks `singlePlayerLoadNewLevel`; then `0x00547e22` tests the pending flag.
  The deferred save does the following, in order:
  - Clears the flag.
  - Refreshes the collision blobs.
  - Sets the KAS global `G_TimesGameLoaded = 0`.
  - Creates the KAS global `G_AutoSave = 1`.
  - Builds the path into `0x009100d0` (`CurrentLevelName`):
    - Level 0 (the tutorial): `SavedGames\Training\<name>`.
    - Any other level: `SavedGames\SinglePlayer\%02d - <name>` (format `"%s%02d - %s"` at `0x00878030`).
  - Calls `SaveGame`.
  - Shows `strSavedGame`, or `strPatchUnableWriteFile` on failure.
  - Destroys `G_AutoSave`, so later manual saves do not contain it.
- **Names written** (English; each name comes from a per-language string table):
  - Mission starts:

    | File | Location |
    |------|----------|
    | `02 - Outskirts of Hiigaran System` | |
    | `03 - Coruc-Tel System` | |
    | `04 - Deep Space (Tel Sector)` | |
    | `05 - Aiowa System` | |
    | `06 - Kadiir Nebula` | |
    | `07 - Outskirts of Kadiir Nebula` | |
    | `08 - Deep Space (Koreth's Rift Sector)` | |
    | `09 - Close Orbit, Gozan IV` | |
    | `10 - Debris Field (Koreths Rift Sector)` | |
    | `11 - System AZ-23769` | |
    | `12 - Turanic Outpost (Kyori Sector)` | |
    | `13 - Location Unknown` | |
    | `14 - Galactic Rim (Gulf Sector)` | |
    | `15 - Deep Space (Gulf Sector)` | |
    | `17 - Naval Base Alpha` | |

    There is no start save for missions 1 and 16. `strSaveSinglePlayerLevel1` still exists but nothing
    reads it.
  - Mission ends: `NN - ~ End of Mission` for missions 1 and 3 to 16, and `17 - ~ End of Game`. These are
    written from `Watch_MissionNN_Objectives` when the mission is won. The `~` makes them sort after that
    mission's start save.
  - Tutorial: `LESSON 01 - Start of Tutorial` through `LESSON 12 - Other Features of Interest`, in
    `Training\`. `tutSaveLesson` (`0x0052a780`, KAS `TutSaveLesson`) is a separate path that saves at once
    to `Training\<name>`, with commas replaced by spaces.
- **On load:** `LoadGame` ends with `kasAutoSaveOnLoad` (`0x00492850`). If `G_AutoSave` is present, it
  increments `G_TimesGameLoaded` and writes the same file again at once, which bumps the counter stored in
  the autosave. It then deletes `G_AutoSave`.
- **`SP_Restart_Level`** (`gpLoadGivenGame` `0x00483780`) reloads the path in `0x009100d0`, which is the
  latest mission autosave. That path is itself saved and loaded with every single-player game.
- **Can it be turned off?** No. Neither function checks an option or cheat flag; the only condition is that
  a mission script calls `SaveLevel`.
- For our code: `kas_host.rs` sets `Game::pending_save = Some((n, name))`. To match the original, the host
  should save at the end of that same sim tick and use the `%02d - ` / Training naming above.

## 4. Quick save / quick load

**Cataclysm has none.** HW1's 2003 source has `gpQuickSave` / `gpQuickLoad` on Shift+F6 / Shift+F7
(`mainrgn.c`). Cataclysm has no such functions: the string `Quick Save` (`0x00876fb0`) is read only by the
list sort, and the localised `strQuickSave` (global `0x00a59490`) is never read. The key binding tables
have no save or load entry.

## 5. Loading: main menu vs in game

- **In game** (`IG_LoadGame`, `gpLoadGame` `0x00483850`):
  - `VerifySaveFile`.
  - Close the screen.
  - `soundEventStopTrack(all, 1.0)` and `soundEventPause(1)`.
  - `gameEnd()`.
  - `utyLoadSinglePlayerGameGivenFilename` (`0x005a5120`), or the multiplayer variant.
  - Unpause sound.
- **Main menu** (`LoadMultiSinglePlayerGameFromMain`, `utyLoadMultiSinglePlayerGame` `0x005a5260`):
  - `gpGetGameName`, then `VerifySaveFile`.
  - Shut down the multiplayer screens if they are open.
  - `feAllScreensDelete`, `singlePlayerInit`, `horseRaceInit`.
  - `gameStart(file)`.
  - `singlePlayerPostInit(1)`. Its argument 1 skips the mission-1 intro Bink.
- **`gameStart(file)`** (`0x005a3dc0`) runs these steps:
  - Stops menu track `0x11`.
  - `PreLoadGame` (`0x004f9280`) checks the version (a mismatch here is fatal), reads the pre-game block,
    and runs `universeLoadEverythingNeeded`.
  - Sets `numPlayers` from the save.
  - Skips `GetMissionsDirAndFile` / `levelPreInit` and all player/AI/research reset.
  - `universeStaticInit` loads the needed ship statics and meshes.
  - `LoadGame` (`0x004f93a0`), which does the following:
    - Stops damage effects.
    - Reads the `0x9c` render block.
    - `plasmaCloudTextureLoad`.
    - Objects, blobs, misc globals, universe, ID tables, nebulae, construction, AI.
    - Single-player info, mimic tables, `moLoad`, KAS, objectives, tech, pings, `CurrentLevelName`.
    - Selections and hot keys.
    - Background: `btgSetTheta` / `btgSetPhi`, background name, `btgLoadTextures`.
    - Lighting: `lightParseHSF` or the default light set.
    - `smLoad`, `ranLoad`, actor/ghost flags.
    - `FixAll*` and `snFix`.
    - Clears the effect list.
    - Closes the file.
    - `kasAutoSaveOnLoad`.
    - `InitStatShipInfoPost` for statics flagged `0x2`.

  After `LoadGame`, `gameStart` switches to the local player, starts the tutorial if needed, and then
  `soundEventPlayMusic(saved song)`. The tutorial plays track `0x12`. Ship upgrades are not reapplied and
  mimic tables are not re-initialised, because both came from the file.
- **Music:** the saved song number starts again from the beginning. Sound effects in progress are lost.
- **NIS:** no NIS object state is saved. `LoadSinglePlayerGame` restores the KAS input-lock flags
  (`0x00910208`), which disable the mouse, hide the cursor or clear the selection, and calls
  `nisTaskResume`. We did not check whether a save made during an NIS resumes it correctly.
- **Camera:** the main camera command and its stack are inside the universe chunk and are restored
  exactly. The background orientation comes from the saved theta/phi.
- **Restart level** loads like the main-menu path but with no sound stop (`gpLoadGivenGame`). Network
  multiplayer loads take their file from `LoadMPGameSet` (`0x009df3a0`).

## 6. Could we load the original's save files?

The files are raw memory images of the original's structs. Each pointer is replaced by an index, but every
other field keeps its in-memory offset, padding, flag bits and enum values. Several fields are indices
into tables in the exe itself: KAS watch functions, AI team indices, gun/effect event indices
(`saveEtglodGunEventToIndex`), static-info indices, and possibly function pointers in the
ShipSpecifics.

An importer would need, at minimum, the offsets and meaning of every field it keeps, in these structs:

| Struct | Size |
|--------|------|
| Universe | `0x3d9bc` bytes, including 8 Player structs of `0x6530` |
| Ship | `0x638` + specifics for every ship type with specifics, + `0x7c` / variable tail |
| ShipStaticInfo copy | `0x458` |
| Bullet | `0x164` |
| Missile/mine | `0x258` |
| Derelict | `0x294` |
| Asteroid | `0x1b4` |
| Dust cloud | `0x1a8` |
| Gas cloud | `0x19c` |
| Collision blob | (size not measured) |
| CommandToDo and its attack/move/dock sub-chunks | (size not measured) |
| Camera command / camera stack | (size not measured) |
| Gun info | `0x88` each |
| Ship-in-production | `0x1b48` |
| AI player / AI team / team moves | (size not measured) |
| Nebula chunks and tendrils | (size not measured) |
| KAS state, objectives, timers | (size not measured) |
| Fog-of-war grid | (size not measured) |
| Mimic tables | (size not measured) |
| Stats tables | (size not measured) |

The importer would then map all of this onto our Rust state, which has a different layout and semantics.
The KAS watch-function index would have to be mapped onto our interpreted mission state machine, in the
same order as the compiled mission's function table.

**Verdict:** possible in principle, but it is a large reverse-engineering project, roughly the size of
the whole sim's data model, and it can't be tested: the install has no save files, so test saves would
first have to be made with the original. Three options:

- **Do not import.** Use our own versioned format.
- **Cheap fallback.** Read only the pre-game block: the version, the single-player flag, the mission
  number at a known position, and `CurrentLevelName`. Offer "restart mission NN" from an original save.
- **Full import, later and opt-in.** Limit it to single-player mission-start autosaves, which are written
  right after level load and so contain the least mid-flight state. Even then the importer needs the
  universe, Player, Ship and specifics, KAS and AI layouts.

## 7. The recreation's saved games

The recreation does not write the original's format (a raw struct dump tied to the exe's memory layout,
section 6). It saves its own state, versioned, and follows the original's behaviour around it.

### What is saved

Everything a running single-player game needs to carry on exactly where it was saved, so that a loaded
game runs bit for bit like the game that was saved (tested, below):

- **The simulation** (`hwc_sim::mission::MissionRun`, all of it derives `serde`):
  - the universe: every ship, bullet and beam, missile and mine, resource, derelict, dust and gas cloud,
    the dead ships still careening (hulks), collision blobs, the object lists in their original order,
    the update counter and clocks;
  - the command layer (every ship's orders, attack and move state, waypoints, formations and parades,
    docking, clamping, right of way), tactics, flight manoeuvres;
  - the players, base building (resources, construction jobs, factories, unit caps, support units),
    research (labs, topics, technology), upgrades, veterancy;
  - the random-number streams (`ranSave`), fog of war, visibility, the nebulae, shock waves, mine
    formations, ship-module states (cloak, EMP, leech, mimic, gravwell...);
  - the effects in flight (`FxWorld`: effects, particle systems, owners, the effect random stream and each
    effect's creation history), which the original drops on load (`listInit(&universe.effectList)`);
  - the run-time tunables (`SimTweaks`; KAS can change some of them);
  - the KAS interpreter: variables, timers, teams with their FSM, state and message queues, the mission
    tick (`kasSave`);
  - the game-side mission state (`Game`): objectives, KAS selections, pings, the single-player state
    machine, hyperspace (the fleet in hyperspace, ships arriving, roll call and fades), the NIS or NISlet
    request waiting for the front end, the presentation queue (speech, music, location cards,
    letterbox calls not yet played), the camera state KAS drives, the log.
- **The interface** (`apps/cataclysm/src/saves.rs`, `UiState`): the main camera (look-at, angle,
  declination, distance, zoom limits and target, the focus and whether it follows it; the nebulae's update
  reads the camera, so it is part of the game), the selection and the ten hot-key groups
  (`SaveMaxSelection`), the music track (it starts again from its beginning, as in the original), the
  pings, the letterbox, the formation choice, and the latest mission autosave (`CurrentLevelName`).

Not saved, linked back on load (`hwc_sim::save::Relink`): the ship statics and the combat routines built
from them, the compiled effects and their event tables, the mission IR (KAS script), the meshes and
textures. They come from the game data the player has.

Not saved, as in the original: sounds and speech in progress, subtitles on screen (a fleet-intel event in
progress then ends by the original's safety timeout), the sensors manager's own view, interpolation.

### File format

`hwc_sim::save`, little endian:

| Bytes | Content |
|---|---|
| 8 | magic `HWCSAVE\0` |
| 4 | format version (`FORMAT_VERSION`, 5) |
| 4 | header length `n` |
| n | header: description, mission number and name, date (seconds since 1970, UTC), game time, update counter, autosave flag, skill, fingerprint of the mission script, program version, a 160-pixel-wide RGB thumbnail |
| rest | the payload (`MissionRun` and `UiState`), `bincode` with fixed-size integers and floats as their bits, deflate-compressed |

The header can be read alone (the save list). The version is checked before anything else is decoded:
a newer version is refused with "saved game format version N; this version of the game reads version 5".
A damaged file, or one whose payload does not match this build's types, is refused ("the saved game is
damaged or from a different build"); trailing bytes are an error. A save made with a different mission
script (a re-extracted mission) is refused by the fingerprint. Mission 1 saves are 30-130 KB.

Because `bincode` stores fields by position, `#[serde(default)]` alone does not let an older file load:
every saved field added moves everything after it. So `FORMAT_VERSION` goes up whenever a saved type
changes shape, and a field added at version `n` is read with
`#[serde(default, deserialize_with = "crate::save::added_in_n")]`, which leaves it at its default
without reading anything when the file is older. Older saves keep loading. Version 1 files were written
by builds that added fields without raising the version, so they are tried as each shape version 1 had
(`VERSION_1_SHAPES`, newest first) until one decodes whole. Version 2 added `Universe::letterbox`, version 3 `ShipBase::avoid_exception` (ship +0x608), version 4 `SimTweaks::spheresize_ignore_numdeclinations` (read with `added_in_or`, since an older file takes the compiled 600, not 0), version 5 `Ship::kamikaze_state` / `kamikaze_vector` (ship +0x2e4 / +0x2e8) and `CombatTweaks::kamikaze_speed_burst` (older files take the shipped script's 3.5).
Fields that are not saved (`#[serde(skip)]`, rebuilt on load) need none of this.

### Where

`saves\SinglePlayer\` and `saves\Training\` of the data directory made by the importer when it can be
written, else `%APPDATA%\hwc-cataclysm\saves\` (always when running from an original install or archive);
`HWC_SAVE_DIR` overrides both. Never inside the original install. A file is named by the description
(characters a file name cannot hold become `_`) with the extension `.hwcsave`, and written through a
temporary file so that a failed save leaves the old one.

### Screens and keys

- The pause menu's SAVE GAME and LOAD GAME screens (the original's `In_game_save` / `In_game_load`, drawn
  by the modern menus by default) and the main menu's Load Game screen list the folder: names sorted
  ignoring case with "Quick Save" first (`compareGamesCB`); the modern menus show each game's mission and
  date (UTC) beside its name (the original shows only the name).
- Saving under a name that exists asks first (`OverwriteGamePopup`, loaded from `front_end.fib` for the
  in-game screen); deleting asks first (`DeleteGamePopup`) and rebuilds the list in place.
- A name needs at least 2 characters (`strAtLeast2Chars`); success shows `strSavedGame`.
- Saving is refused during an NIS, a movie, or after the mission is over.
- Mission autosave: the KAS `SaveLevel` request is turned into `Game::autosave` at the end of its update,
  with `G_TimesGameLoaded = 0` and `G_AutoSave = 1`; the game writes `NN - <name>` (the tutorial's in
  `Training`) before the next update, then removes `G_AutoSave`. Loading an autosave adds one to
  `G_TimesGameLoaded` and writes it again at once. "Restart level" loads the latest autosave, or starts the
  mission again when there is none. Mission 1's script asks for `01 - Hiigara` at update 256.
- Quick save and load: the original has none. An opt-in addition (`hwcQuickSave = 1`): Shift+F5 saves as
  "Quick Save", Shift+F9 loads it. (F5..F9 alone are the original's formation keys and Ctrl+F5/F9 the
  sensors manager's toggles, so the plain keys were not used.)

### Tests

- `crates/hwc-sim/tests/save.rs` (game data): mission 1 played by the scripted player is saved at the
  first update, during the fleet's hyperspace arrival, with an NIS waiting, while fleet intelligence
  speaks, with effects in flight, in combat and late; each save is loaded and run 240 more updates: every
  world hash, and the whole encoded state at the end, equals the uninterrupted run's, and a loaded game
  re-encodes to the same bytes. A game saved late in mission 1 jumps on the same update and carries the
  same fleet into mission 2, which runs the same, and a mission 2 save resumes exactly. The version and
  script checks. The autosave request and its KAS globals.
- `crates/hwc-sim/src/save.rs`: float bits (NaN, -0, subnormals, infinity) survive, versions, damaged
  files, header-only reads.
- `apps/cataclysm/src/save_checks.rs` (game data and a GPU): mission 1 saved from the pause menu by
  typing a name, loaded behind the loading screen (selection, hot-key group, camera back) and run on next
  to the game that was saved, update for update; overwrite question (no keeps, yes overwrites); the load
  list with mission and date; delete after asking; loading from the list; the main menu's Load Game list
  and its load; quick save off by default, Shift+F5 / Shift+F9 when on; the mission autosave and its
  rewrite on load.
