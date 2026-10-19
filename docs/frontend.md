# Front end (menu system)

`crates/hwc-ui` recreates the engine's front-end manager — `FEFlow.c`, `FEReg.c`, `UIControls.c`,
`Region.c`, `FontReg.c`, `ColPick.c`, `PlugScreen.c` and the font/2D primitives — with
Cataclysm's changes (pull-down menus removed, slightly different control draws), checked against
the retail executable. `apps/cataclysm` is the game executable; for now it runs the front end.

```
cataclysm [data] [--fullscreen | --window] [--skip-intro] [--no-sound]
          [--scale original|fit|integer|stretch] [--filter nearest|linear]
          [--size WxH] [--config file] [--tour outdir] [--exit-after s]
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
MusicVolume = 60
PlayerName = "Fleet Commander"
```

Integers or double-quoted strings (`\"`, `\\` escapes), `#` comments; unknown keys are kept. When
the file does not exist yet the install's `Cataclysm.cfg` is imported. It is written when the
options are accepted (atomically: temporary file, then rename). Colours are stored packed like
the engine's `colRGB` (`0xAABBGGRR` as a signed integer).

| Option (default) | Control |
| --- | --- |
| `SpeechVolume` (75), `SFXVolume` (75), `MusicVolume` (60) | audio sliders, 0-100 |
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

* No game behind the menus: starting a mission or loading a game reports the request and
  returns to the main menu.
* Display mode: the renderer list shows the wgpu presenter; resolutions are a fixed list of
  common modes. Full screen uses an exclusive mode when the monitor offers it.
* The movies do not pause when the window loses focus (`binkPause`), and keys pressed during the
  movies are not passed on to the front end.
* The original only applied the music level "now" from the music slider; cancelling the options
  restored the volumes with the normal fade. Here every audio change applies immediately.
* Changing the window size in `original` mode keeps the open screens (their create callbacks run
  again); the engine dropped back to the main menu after a resolution change.
* Key bindings are shown read-only; saved-game lists are empty; multiplayer stops at the
  connection-method screen; in-game screens (taskbar, managers) load and draw but their
  callbacks are stubs.
* Spanish and Italian labels are not in the shipped files; only English, French and German can be
  selected.
* The colour picker colours the race preview with the plain team-colour blend (the engine also
  applied per-race brightness curves from `colpick.script`).
