# Front end (menu system)

`crates/hwc-ui` recreates the engine's front-end manager — `FEFlow.c`, `FEReg.c`, `UIControls.c`,
`Region.c`, `FontReg.c`, `ColPick.c`, `PlugScreen.c` and the font/2D primitives — with
Cataclysm's changes (pull-down menus removed, slightly different control draws), checked against
the retail executable. `apps/cataclysm` is the game executable: the front end, and from it Mission 1
on the simulation (see "Playing a mission" below).

```
cataclysm [data] [--fullscreen | --window] [--skip-intro] [--no-sound]
          [--scale original|fit|integer|stretch] [--filter nearest|linear]
          [--size WxH] [--config file] [--tour outdir] [--exit-after s]
          [--ingame-preview] [--mission n]
```

`data` is the directory written by `hwc-import` (default `$HWC_DATA_DIR`, then `$HWC_GAME_DIR`);
for development an install directory or a bare `cataclysm.big` also works (`GameData::open`).
The original's switches `/fullscreen` (the default), `/window` and the hidden `/disableAVI`
(same as `--skip-intro`) are accepted, ignoring case. F11 toggles borderless full screen, F10
cycles the scaling modes. `--tour` renders a scripted walk through the intro and the menus to
PNG files (see `screenshots/shell/`) and prints the sound events and music cues;
`HWC_UI_DEBUG=1` prints the diagnostics the engine only printed in debug builds (callbacks not
found, links missing).

## From launch to the main menu

As `WinMain` and `utyGameSystemsInit` do it:

1. Full screen (the default), the display switches to 640x480 for the movies (32 bits, else
   16); in a window, the window has the configured size from the start.
2. The sound system starts with the saved volumes.
3. Unless `/disableAVI`: `Movies\sierra.bik` with music track 60, a two-second pause (the last
   picture stays up), `Moviesdogintro.bik` with track 61. Before each movie with music the
   engine waits 150 ms; each track stops (no fade) when its movie ends. The frames are drawn
   unscaled: centred in a window, at the top-left of the 640x480 mode (no display callback,
   so `binkPlay` uses its GDI blitter). Only releasing **Escape** does anything: it stops the
   movie playing (`binkStop` from `WM_KEYUP`); the pause and the next movie still follow. No
   pointer is drawn (the mouse is started after the movies). `apps/cataclysm/src/intro.rs`
   plays them with `hwc-video`, tables from `GameData::bink_tables()`; frames are due by
   wall-clock time and missed frames are decoded but not shown, like `binkUpdateFrame`. The
   scaling modes other than `original` scale the movies too.
4. The display goes back to the configured mode, the front end starts on `Main_game_screen`,
   the bitmap pointer appears, and music track 17 starts (`soundEventPlayMusic(0x11)`).

### Sounds

The controls fire the engine's UI sound events (`hwc_ui::consts::sound`, queued in
`FrontEnd::sounds`, handed to the application as `Request::SoundEvent`), all on the press, none
on hover or release:

| Control | Event |
| --- | --- |
| button press, button hot key, Space on a button, Escape to the default "back" button | `0x108` |
| list item clicked, arrow/page keys in a focused list with a selection, scroll-bar track click, text entry clicked | `0x108` |
| check box / toggle pressed (mouse, hot key or Space) | `0x10c` if it was checked, `0x10d` if not |
| `CP_RaceOne` / `CP_RaceTwo` pressed with the mouse | `0x13a` / `0x13b` |
| radio button pressed, hot key, Space, arrow keys | `0x10f` |
| "Sound test" (`OP_SoundFX_Test`) | `0x1c8` (plays ship event `0x821`) |
| "Speech test" (`OP_Speech_Test`) | speech event `0x2001024` (`Request::Speech`) |

Sliders make no sound. The volume sliders apply as they move (`normalizeSmoothie`: level =
value x 1.025 / 100, clamped; music immediately), and so do the channel count, automatic
channels, sound quality (0 normal, 1 automatic, 2 low), speaker presets and equaliser bands
(gain 1 - value / 100): the front end sends `Request::AudioSettings` and the application
reapplies the audio options. Quitting fades the music (0.5 s), pauses the sound and closes
(`utyGameQuit`). The front-end music level is the in-game curve the engine falls into before a
game has run (main screen, nothing near, inactive).

### Pointer

`CursorBitmaps\cursor.LiF` (`mouseStartup`) is drawn alpha blended with its top-left pixel on
the mouse position (no hot-spot offset, as `mouseDraw` places the quad); the system pointer is
hidden. Front-end screens always use the normal pointer. It is drawn into the front-end canvas,
so the scaled modes scale it with the menus.

### Display mode changes

Accepting the options with a different resolution switches to it (exclusive full-screen mode of
that size when full screen and the monitor has one, else the window size) and opens
`Countdown_Message_Box` over the main menu. "Yes" keeps it; "No" restores the old mode; after
22 seconds without an answer the old mode comes back and every screen is replaced by the main
menu (`opTimerExpired`). The box's last line always reads 20 seconds: the engine prints the
timeout less two and never counts down.

## Modules

| Module | Original | What |
| --- | --- | --- |
| `fe` | FEFlow.c, UIControls.c (logic), Region.c | screens, stack, regions, events, focus, links, callbacks |
| `region` | Region.c | region tree, handles, `regRegionInside` |
| `controls` | UIControls.c | slider, scroll bar, list window, text entry state |
| `fer` | FEReg.c | textured drawing: boxes and cut-outs, buttons, marks, sliders, scroll bars, lists, decorations |
| `text` | font.c, FontReg.c | `.hff` printing, hot-key letters, drop shadows |
| `canvas` | prim2d.c | RGBA8 framebuffer, alpha-tested blits, rectangles, lines |
| `cataclysm` | utility.c, Options.c, KeyBindings | the game's callbacks for the main menu, single player, options |
| `colpick` | ColPick.c | team colour picker |
| `credits` | PlugScreen.c | credits slide show |
| `options` | Cataclysm.cfg / registry | option store and file |
| `known` | — | every callback name the shipped `.fib` files use |
| `scale`, `present` | feResReposition* | window placement; wgpu presenter |
| `session` | main.c (FE part) | ready-to-run front end used by the app, the tour and the tests |
| `ingame` | ConsMgr.c, LaunchMgr.c, ResearchGUI.c, Trader.c, MainOverlay.c, ShipPrint.c, PopupMenu.c, UserKeys.c, GamePick.c, Objectives.c | the in-game interface (below) |

### Drawing

The engine draws its menus with OpenGL in 2D mode: blending off, an alpha test on each bitmap,
nearest sampling at 1:1. That is reproduced exactly on the CPU into an RGBA framebuffer, which
the window shows as one texture (`present::Presenter`, wgpu). Rules that matter:

* `ferDraw(x, y, tex)` anchors a bitmap by its **bottom**-left corner (`y - height .. y`).
* Frame pieces, buttons, check boxes, radios and sliders use `alpha > 0`; decorations up to 256
  pixels use `alpha > 1/255`; bigger decorations and bitmap buttons are copied whole.
* Corner pieces are stored for the top-left, sides for the left; other orientations are
  quarter turns clockwise of those (`ferFlipBitmap90` on the vertically mirrored data).
* Box frames (`ferDrawBoxRegion`) choose corner/side textures from the atom's two draw-style
  words (out-of-range values give medium textures with an external glow); cut-out regions bite
  notches into the frames of the base region and of earlier regions they overlap.
* Text: 4-bit glyph coverage becomes alpha `value * 16`; `&` marks the hot-key letter, printed
  with each channel times 350/256; the shadow is printed first, offset one pixel.
* Buttons and other controls choose their textures from the region status: disabled, pressed
  (with/without mouse inside), keyboard focus, mouse over, normal; controls of a screen other than
  the top one draw "off".

### Screens and regions

* Loading (`feScreensLoad`): labels (kind 0 with text) become static text; list windows lose the
  18 pixels of their scroll bar; every label keeps all its language strings (English, French,
  German are shipped) and the current language is drawn.
* Positioning: layouts are 640x480. On a bigger screen every atom is offset by half the extra
  size; atoms with the *background* flag are stretched to each screen edge they touch; atoms with
  no corner on the 640x480 layout are hidden (they keep their regions, so hidden hot-key buttons
  such as `Quit_game`'s Escape button still work). Screens with menu items keep 640x480
  coordinates.
* Regions are created last atom first and appended, so file order is drawn back to front and the
  last atom gets the mouse first. Afterwards `feFindChildren` makes regions lying on a "stackable"
  region (label on a button) its children. Cataclysm indexes the stackable table with a
  decorative region's *name* (the region ID is the bitmap path), reading past the table; the
  reimplementation reproduces the outcome with a 256-bit mask of the bytes found there.
* Events: children first, then the region (enter/exit, press/hold/release, wheel, double click,
  hot keys by "stuck" key state, key capture for focused text entries and lists). A modal
  screen's base region stops events for everything processed after it (the screen below).
* Screen stack (`feScreenStart`): a non-popup screen replaces the top screen when it has the same
  parent; popups become children of the current base region. Starting the screen already on top
  does nothing. Call-on-create callbacks run last atom first, after which every screen with tab
  stops gets the Tab/arrow/Space/Return/Escape/Home/End handlers (no tab stops: no handlers).
* Links: a link button follows the link of the same name of the *current* screen; a missing link
  does nothing (the unused `Effects` screen).

### Callbacks

`FrontEnd::callback_add(name, fn)` binds a function to an atom name like `feCallbackAdd`;
`draw_callback_add` binds user-region drawing. A user region without a registered draw callback
gets no region at all, as in the engine. `cataclysm::register` installs the game's functions and
then a logging stub for every remaining name in `known::FUNCTIONS` / `known::DRAW_FUNCTIONS`
(353 function and 87 draw names over the 21 shipped `.fib` files); every name used by
`front_end.fib` has a real implementation.

Implemented: main menu (training, single player, multiplayer entry, options, credits, quit,
version string), training selection and the training alert, single-player setup with difficulty,
team colour picker and race preview, load-game screens (lists stay empty: no saved games yet),
all option screens (audio, advanced audio, equaliser, video, custom effects, texture info,
gameplay, key bindings from `default_keys.cfg`), presets (effects High/Low/Custom, Homeworld
classic / Cataclysm default, fastest / default frame rate), language radio buttons, credits slide
show, multiplayer connection screen with Back. Starting a mission, sounds, opening the support
web page and changing the display device are reported to the application as `Request`s.

## In-game interface

`hwc_ui::ingame` recreates the screens of a running game against a host trait, so it does not
depend on the simulation's internals:

* `GameUiHost` (in `ingame::host`) is what the screens read: player numbers (RUs, support
  units, race, single/multiplayer, paused), factories and their construction lists
  (`BuildRow`: class headings and ships with buildable / hidden / greyed status, caps, support
  units), build jobs (count, batch size, progress, cost paid so far), labs and technologies
  with the research list rows, launchers with docked ships and slot counts, mothership modules,
  the Beast power allocation, the selection (`ShipInfo`: health, upgrades, tech level, guns,
  owner), the commands available to it, objectives, messages, tooltips, and whether Escape has
  something to cancel in the game. `HostView::capture` copies it every frame.
* Everything the UI does comes back as `GameRequest`s (build / unqueue / cancel / pause jobs,
  research / clear lab, launch, auto-launch, repair priority, Beast systems, ship commands, key
  actions the game handles, pause, save / load / restart / quit, selection changes, sounds and
  speech). The UI never waits for them: the next frame's copy shows the result.
* `InGameUi` is the per-frame API: `handle_input(host, InputEvent)` (returns whether the UI
  used the event), `update(host, dt)` (returns the requests), `draw(canvas)` over a canvas that
  already holds the 3D view, `open(host, Manager)`, `open_popup`, `game_over`, and
  `ship_view()` / `set_ship_view_image()` for the build manager's 3D ship picture (rendered by
  the application). `action_held()` answers the polled actions (pan, zoom, band-box
  modifiers).
* `ingame::mock::MockHost` is a Somtaaw (or Beast) fleet that applies the requests the way the
  game would. The tests drive every screen with it (`tests/ingame.rs`, screenshots in
  `screenshots/ingame/` when `HWC_SCREENSHOTS` is set), and `cataclysm --ingame-preview` opens
  it when a game is started from the menu (a starfield stands in for the 3D view).

The managers are the `.fib` screens of `FEMan\` on the same front-end manager as the menus (one
`FrontEnd<Cataclysm>`), so the in-game option screens reuse the menu's option callbacks.

| Screen | Behaviour |
| --- | --- |
| Construction manager (`Construction_manager`, `Beast_Construction_manager`) | Class headings and ships (a heading only above a visible ship). Left click selects (Shift toggles) and, on a selected row, adds one (Ctrl: five); right click removes one. Counts stay in the manager until BUILD, unless the factory already builds that type: then they go straight into its queue. Removing never takes the ship in progress. Held arrows repeat every 0.25 s. Refused rows (caps, support units, greyed) flash red for 0.3 s; cap failures speak `0x2001039` / `0x2001038`. PAUSE BATCH toggles the selected types (pauses unless all are paused); CANCEL BATCH cancels them (the game refunds what was paid). Ship and batch progress bars per row; mothership and two carrier icons (`Build_race{1,2}_icon{1,2}_{disabled,off,on}`) with the connector line; support-units bar from the `b_Build_Support*` textures (used, pending dimmed, free masked); totals; INFO shows the description over the ship view. Sounds `0x12f` + `0x104` on open, `0x130` + `0x100` on close; speech as `cmBuildShips` / `cmPauseJobs` / `cmCancelJobs`. The game keeps running. |
| Launch manager (`Launch_Manager`, `Beast_Launch_Manager`) | Docked ships in the engine's print order under FIGHTER / CORVETTE headings (" 3 x ACOLYTE"); LAUNCH ONE / LAUNCH GROUP (selected type) / LAUNCH ALL (all but dEcclesiam); auto-launch boxes (checked = stay docked); slot bars (green free, red used, count). Somtaaw module panel: name, health bar, three repair-priority buttons `R1_Sys{1,2,3}_*` (hover taken from the mouse x like the engine). Beast emergency power radio (enabled with a Beast mothership). |
| Research manager (`Research_Manager`, `Beast_Research_Manager`) | Techs under module headings (those that cannot be researched are hidden), coloured by state and kind (new capability / upgrade), progress bar behind researching techs, green dot on owned ones. RESEARCH (or a double click) starts the tech in its module's lab if that lab is idle; CLEAR LAB stops the selected lab. Five lab buttons with `Research_icon_*` icons, progress bar and scan line; clicking one shows its module picture. Tech picture `ResearchGUI\Race<n>\<TECH>.lif`, description under INFO. The Beast's single lab is the animated blob. |
| Trader (`Trader_Interface`) | Offers with prices (red when unaffordable), dialogue, picture and description, RUs; ACCEPT OFFER / DONE. |
| Escape menu (`In_game_esc_menu*`, `SP_Game_Over`, `Quit_game2`, save / load, `Game_options` and its option screens) | Variant by mode (tutorial, campaign, skirmish, multiplayer); pauses except in multiplayer; option Accept / Cancel return to `Game_options`. |
| Fleet intelligence (`Single_Player_Objective`) | The clicked objective's text (`Arial_b12`, (0,200,200)); pauses. |

The build and research managers are shown in the modern HUD's style by default (a
user-requested change; the original screens stay open underneath for their keys, callbacks and
rules, and the new panels act through the same functions as the original's regions). The
switches are `--build-ui original|modern` and `--research-ui original|modern`, options-file
keys `hwcModernBuildManager` and `hwcModernResearchManager` (default 1); the original HUD
(`--hud original`, `hwcModernHud = 0`) brings back both original screens too. The escape
menu with all its screens (options, key bindings, save / load, confirmations), the objectives
screen and the systems (launch) manager are restyled the same way (`--menus original|modern`,
`hwcModernMenus`, default 1), with a modern options screen (ENHANCEMENTS) in the options. See
[modern-ui.md](modern-ui.md) for the layout, what each control calls and what is not 1:1.

Drawn directly over the 3D view:

* **Main overlay**, from `MainOverlay.script` ("shown,backdrop" per panel, per single / multi
  player and view 1-3; Next_Overlay_View cycles): selected ships top right ("2 x ACOLYTE", grey
  or blue by owner, hover green; a click narrows the selection; with Multiple_Select held it
  marks the line and the key release removes the marked types), band-box list, objectives
  bottom left (a click opens fleet intelligence, the hyperspace objective jumps), tooltip,
  messages (five lines, older ones dimmer, fading out 8 to 13 s after they were posted, scroll
  buttons), player names, construction and research progress (`Build_ProgressBar` /
  `Research_ProgressBar`), SU / RU / game time bottom right (SU pulses red-yellow over the
  cap), taskbar view buttons and backdrop (gameplay options), sensors pan arrows, "PAUSED".
  Holding Toggle_Overlay_Options shows the options menu, where 1-9 toggle panels
  (Change_Overlay_Backdrops_Toggle switches them to the backdrops); Show_Keys_1-3 show the
  binding pages. `overlay::draw_health_bar` draws a ship's health bar for the game's 3D pass.
* **Ship info** (Toggle_Ship_Info): type and rank, upgrades (green installed, yellow
  researched, red possible), class, health, armour level, guns. It is dragged while the
  options key is held, and has its own 1-8 option menu then.
* **Right-click menu**: the main menu (Special Action..., Upgrade, Move, Start Waypoints, Dock,
  Retire, Cancel, and the Tactics, Formations, Focus, View, Sensors submenus) and the
  special-actions menu, from the commands the host offers. arial_12 (arial_15 with large
  fonts), `PopupMenu_*` icons; hover opens submenus, Enter and the arrows work, each item runs
  its key action; the selected ships are outlined while an item is highlighted. The game opens
  it (`InGameUi::open_popup`) when a right click has no order to give.

### Keys

`ingame::keys` has the 147 actions in the engine's order with their `keys.cfg` names, up to
three bindings each (`ALT+SHIFT+CTRL+<vk>`). `default_keys.cfg` is read at start; the player's
`keys.cfg` (beside the options file) is used unless missing or older (its `v` line); accepting
the options writes it. A key fires every action bound to it with exactly the held modifiers (a
binding of Shift, Ctrl or Alt itself ignores that modifier). Ship_Special, Enable_Height_Move and
Multiple_Select also have release handlers; pan, zoom, the band-box modifiers, the overlay
options and the key pages are polled. UI actions (managers, Escape, overlay, ship info, menu,
message paging) are handled by the UI; ship commands become `GameRequest::Command`, everything
else `GameRequest::Action`. Escape closes the menu, else goes to the game when it has something
to cancel, else opens the escape menu.

The options key list (front end and in game) lists the 147 actions with their group titles.
Enter or a double click waits for a key ("??????") and binds it with the held modifiers, taking
the combination from actions that may not share it (only height move / multiple select, sensors
/ skip sensors and the seven cancel actions share); binding the same combination again clears
the action; keys without a name open the "invalid key" box. RESET TO DEFAULT goes back to the
saved `keys.cfg` (the engine reloads it); Cancel restores the bindings.

## Playing a mission

Single Player -> New Game (or `--mission n` straight from the front end) runs
`apps/cataclysm/src/play.rs`: the menus go and the loading screen comes up (`HorseRace.c`, single
player): `FEMAN\Horse_Race.fib` screen `Horse_Race_Single` over
`SinglePlayer\MissionNN\loading.jpg` resampled to 640x480 and drawn unscaled in the middle
(black around it), a solid 152x3 progress bar at (243, 234) of the layout in the colour of the
mission's `loading.script` (default 255,63,63), and "Skill Level ..." in white Arial_12 at
(10, height - 40); no pointer. One piece of loading is done per frame (statics, level and KAS
script, ship art, interface, GPU upload) and the bar moves on.

In the mission (`game.rs`, `world.rs`, `host.rs`): the 3D view is `hwc-render` with the game
camera, read back into the interface canvas so the in-game screens (`InGameUi`) blend over it
exactly as on the CPU; the interface reads the simulation through `SimHost` (`GameUiHost`) and
its requests go to the command layer (build/unqueue/cancel/pause jobs, research, clear lab,
launch, auto-launch, dock, retire, harvest, halt, tactics, formations, hot-key groups, focus,
pause, quit/restart). The main view follows `mainrgn.c`: left click selects (with the ship's
formation; Multiple_Select toggles), drag band-boxes (Bandbox_Attack attacks, Bandbox_Focus
focuses, Multiple_Select adds), a click on an enemy with ships selected attacks, Bandbox_Focus +
click focuses; right drag rotates the camera, both buttons zoom, the wheel and the zoom keys
zoom; a right click without moving opens the right-click menu; Move (M) brings up the pie plate
(the destination follows the pointer on the selection's plane, Enable_Height_Move gives height,
left click moves, right click or Escape cancels); Escape cancels the plate, then the selection,
then opens the escape menu (which pauses). Music, speech with subtitles (`subtitle.script`
region 1, Arial_13 with shadow), location cards and the simulation's gun/hit/explosion sounds
play through `hwc-audio`. `MissionFailed` opens the game-over escape menu; `MissionCompleted`
shows a note and returns to the menu after 10 s (the next mission cannot start yet).

Known differences of the mission: no opening NIS (and its loading screen); no ship picture in
the build manager; no selection speech; the overlay's message lines are simple "built" /
"research complete" notes; KAS `TechSet` / `EnableShipConstruction` are only recorded by the
simulation, so the build list shows what the research table allows; scuttle, guard, waypoints
and special actions are not wired; subtitle actor pictures are not drawn.

## Options

### The original

Cataclysm keeps player settings in two places:

* `Cataclysm.cfg` in the game folder, written at exit by `utyOptionsFileWrite` from one table of
  tweakable variables as `name value` lines (`%s %d`, strings `%s %s`) and read back at start-up
  with the tweak-script parser (`scriptSetFileSystem`). The retail install ships it with only
  `language=0`.
* The registry key `HKEY_LOCAL_MACHINE\SOFTWARE\Sierra On-Line\Cataclysm` for the display:
  `deviceCRC`, `deviceCaps`, `deviceCaps2`, `deviceIndex`, `deviceToSelect`, `glToSelect`,
  `d3dToSelect`, `screenWidth`, `screenHeight`, `screenDepth`, `HW_Language` (DWORDs and
  strings).

The options screens edit the variables directly; entering them takes a snapshot
(`opOptionsSaveSettings`) that Cancel restores, and the Advanced, Equaliser and Custom Effects
sub-screens keep their own restore points. Accepting the custom effects screen always selects the
"Custom" effects level (its toggles mark it changed as soon as the screen opens).

### Our file

`hwc-options.cfg` (in `<data>/config/`, or the current folder when running from an install or
archive, or `--config`) keeps every value under its original name, one per line:

```text
# hwc options v1
MusicVolume = 45
PlayerName = "Fleet Commander"
```

Integers or double-quoted strings (`\"`, `\\` escapes), `#` comments; unknown keys are kept. When
the file does not exist yet the install's `Cataclysm.cfg` is imported. It is written when the
options are accepted (atomically: temporary file, then rename). Colours are stored packed like
the engine's `colRGB` (`0xAABBGGRR` as a signed integer).

| Option (default) | Control |
| --- | --- |
| `SpeechVolume` (75), `SFXVolume` (75), `MusicVolume` (45; the original's 60, lowered 25% at the user's request) | audio sliders, 0-100 |
| `NumChannels` (8), `AutoChannel` (0) | channels slider (shown + 8, voices 8-32), Auto |
| `SoundQuality` (0), `speakerSetting` (1), `Equalizer0-7` (0) | Advanced: quality, speakers (presets fill the equaliser), equaliser bands (value x 1.025) |
| `VoiceCommands`, `VoiceStatus`, `VoiceChatter` (1), `opBattleChatter` (50) | Advanced: speech check boxes, chatter slider |
| `screenWidth`/`screenHeight` (640x480) | resolution list (applied to the window on accept) |
| `brightnessVal` (30, 0-70), `TextureDetail` (50) | brightness and texture memory sliders |
| `DetailQuality` (0 high, 1 medium, 2 low), `detailThreshold` (10; slider shows 30 - value), `noLOD` (0) | model quality / detail / full detail |
| `effectsLevel` (0 high, 1 low, 2 custom), `etgHistoryScalar` (256; slider 64-256) | effects radio, number of effects |
| `texLinearFiltering`, `showBackgrounds`, `InstantTransition`, `enableStipple`, `fuzzyBlobs`, `etg{Damage,Bullet,Fire,Hit}EffectsEnabled`, `enableCPV`, `enableLensFlare`, `enableRepairbots`, `enableSmoothing`, `enableTrails` | custom effects (check box and picture button pairs) |
| `opMouseSens` (50), `GameFramerateOption` (1) | gameplay: mouse sensitivity, frame rate |
| `HomeworldTaskbar`, `ShowTaskbarButtons`, `ShowTaskbarBackdrop`, `ExperienceMessages`, `LargePopupMenuFonts` (inverted), `AutoFocusOnWaypoints`, `Homeworld_CSA` / `RightClick_CSA` (exclusive) | gameplay check boxes |
| `ShowAreaVisited`, `ShowFOWBlobs`, `ShowAreaOccupied`, `ShowResources`+`ShowMeteors`, `ShowAsteriod0`, `ShowBackground`, `ShowCompass`, `ShowObjectivesPointer`, `ShowSlipgatePaths`, `ShowIcons` | sensors manager check boxes |
| `DifficultySetting` (2), `TutorialNeeded` (1), `PlayerRace`, `PlayerBaseColor`, `PlayerStripeColor`, `ColorsPicked`, `PrevColor0-4.base/detail` | single player setup and colour picker |
| `language` (0 English, 1 French, 2 German) | language radio buttons (fonts, labels, hot keys) |

The other table entries (`InfoOverlay`, `FirewallDetect`, `MPGameFlags`, ...) are carried
unchanged.

## Scaling

| Mode | Behaviour |
| --- | --- |
| `original` (default) | the engine's: layout at native pixels, centred, background atoms stretched to the edges (`feResRepositionX/Y`, `feResRescaleBackground`); the canvas is the window size |
| `fit` | the 640x480 canvas scaled uniformly to fit, letter- or pillar-boxed |
| `integer` | the 640x480 canvas scaled by the largest whole factor that fits |
| `stretch` | the 640x480 canvas stretched to the window |

Mouse positions are mapped back into canvas pixels (`scale::Placement::to_canvas`); while a
slider or the colour picker holds the mouse, positions are clamped to it like `mouseClipToRect`.

## Known differences

* Only Mission 1 can be played (the simulation cannot yet carry a fleet into later missions);
  the training mission and other missions show "not playable yet" over the loading screen and
  return to the main menu. Loading a saved game reports the request only.
* Display mode: the renderer list shows the wgpu presenter; resolutions are a fixed list of
  common modes. Full screen uses an exclusive mode when the monitor offers it.
* The movies do not pause when the window loses focus (`binkPause`), and keys pressed during the
  movies are not passed on to the front end.
* The original only applied the music level "now" from the music slider; cancelling the options
  restored the volumes with the normal fade. Here every audio change applies immediately.
* Changing the window size in `original` mode keeps the open screens (their create callbacks run
  again); the engine dropped back to the main menu after a resolution change.
* Saved-game lists are empty; multiplayer stops at the connection-method screen.
* In game: the HW1-style taskbar and sensors manager screens are not recreated (the sensors
  view belongs to the game); the overlay has no taskbar auto-hide, tactical legend icons,
  multiplayer bounties or "end of recording" text; message time-outs count from the message
  time rather than from when a line becomes the oldest shown; the right-click menu lacks the
  waypoint-mode, ship-name, mimic-disguise and alliance submenus and outlines every selected
  ship rather than only those able to do the item; the ship info panel's upgrade names are
  matched to bits in order; the key list has no title row, and the key binding pool list stays
  empty (as in the game, which has no handler for it); the trader's dialogue lines are built in
  (the game takes them from mission scripts).
* Spanish and Italian labels are not in the shipped files; only English, French and German can be
  selected.
* The colour picker colours the race preview with the plain team-colour blend (the engine also
  applied per-race brightness curves from `colpick.script`).
