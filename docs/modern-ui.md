# The modern UI: managers, menus and options in the modern HUD style

Three parts, all user-requested changes, **on by default**, each with a switch back to the
original screens:

* the build and research managers (below);
* the in-game menus: the escape menu and every screen behind it, the objectives screen, and
  the systems manager ([Menus and the systems manager](#menus-and-the-systems-manager));
* the modern options screen, ENHANCEMENTS, in the in-game options
  ([The modern options](#the-modern-options)).

## Build and research managers

The build manager (`construction_manager.fib`) and the research manager
(`research_manager.fib`, the Beast's `beast_research_manager.fib`) restyled in the modern HUD's
look, with switches back to the original screens:

| | |
|---|---|
| Switches | `--build-ui modern\|original`, `--research-ui modern\|original`; options file `hwcModernBuildManager` and `hwcModernResearchManager` (default 1) |
| Global revert | `--hud original` (or `hwcModernHud = 0`) brings back the original HUD and both original managers, unless `--build-ui` / `--research-ui` says otherwise |
| Code | `crates/hwc-ui/src/ingame/modern_manager.rs` (the shared look: panel placement and scale, frame, title, tabs, buttons and key caps, rows, bars, scroll bar, readouts, cells, picture box, text pane, hover easing, the double-click rule, the switches), `modern_build_manager.rs` and `modern_research_manager.rs` (layout, hit-testing, input, drawing of each), `apps/cataclysm/src/ship_preview.rs` (the build manager's ship render) |
| Hooks | `ingame/mod.rs` (route mouse, tick, draw, ship view while active; fresh state on open), `consmgr.rs` and `research.rs` (a few functions made `pub(crate)`), `modern_hud::manager_area` (the HUD panels keep clear of the panel), `options.rs` (the keys), `game.rs` (renders the ship view), `main.rs` (the switches, `--help`) |
| Tests | unit tests in the three modules (layout at many sizes, hit-testing, thumb drag, drawing helpers, hover easing, double clicks, the switches); `crates/hwc-ui/tests/modern_build_manager.rs` and `modern_research_manager.rs` (mock host, every control); `apps/cataclysm/src/buildui_shots.rs` and `resui_shots.rs` (Mission 1, game data). The original managers' tests (`tests/ingame.rs`, the HUD tests over a manager, the game-path smoke test) run with the original screens |
| Screenshots | `screenshots/buildui/` (`m1_build_manager_*`, `mock_*`) and `screenshots/resui/` (`m1_research_manager_*`, `_researching_`, `_info_`, `_lab_`, and `mock_*`) |

### How they work

The original screen is still started and kept open underneath. It is not drawn and gets no
mouse events, but it keeps the keyboard, so the hot keys, Return and Escape (its default
buttons), the Tab / arrow focus, the callbacks and every rule in `consmgr.rs` and `research.rs`
stay the same code. The new panel draws over the 3D view and turns mouse input into the calls
the original's regions make. Buttons click on the press and run their callback on release
over them (`uicButtonProcess`); a press dragged off a button cancels it; the second press of a
double click is the double-click message (`WM_LBUTTONDBLCLK`), which only the lists see.
Clicks on the modern HUD's panels are taken by those panels first; clicks elsewhere outside
the panel do nothing (the originals covered the screen).

The Build Manager's Research button (and R) opens the modern research manager, whose Build
button (and B) comes back to the modern build manager; Systems (S) opens the systems (launch) manager, in the same look with the modern menus (below).

#### Build manager

| Original region | Modern control | Behaviour kept |
|---|---|---|
| `CM_MotherShipDraw`, `CM_Carrier1Draw/2` (`cmSelectMotherShip`...) | factory tabs | a press selects; an empty slot is disabled; the second press of a double click is not seen |
| `CM_Ships` (`cmSelectAvailable`) | ship rows | left: select, or add when already selected; Shift toggles the selection; Ctrl adds 5; right: remove (and select); the double-click message acts once more; the wheel scrolls 3 rows |
| `CM_LeftArrowDraw` / `CM_RightArrowDraw` | - / + boxes | a press fires, holding repeats every 0.25 s (on the row under the pointer), Ctrl means 5, a double click's second press is ignored |
| `CM_Scroller` | thin scroll bar | drag the thumb, click the track to page |
| `CM_BuildShips`, `CM_PauseJobs`, `CM_CancelJobs`, `CM_Close`, `CM_Research`, `CM_Launch`, `CM_ExtendedInfo` | buttons | as above |
| keys | (the screen's) | B build, P pause/resume, L cancel, C close, I info, R research, S systems, Return = Build, Escape = Close (one press) |

#### Research manager

| Original region | Modern control | Behaviour kept |
|---|---|---|
| `RM_Lab1Draw`..`RM_Lab5Draw` (`rmSelectLab1..5`) | lab tabs | a press selects the lab and shows its module's picture; an absent lab does nothing; the second press of a double click is not seen. The Beast's lab button is off its screen (the blob stands there), so its one tab only shows the lab |
| `RM_TechListWindow` (the screen's own list window) | topic rows | the release selects a topic (click sound), shows its picture and selects its lab; the double-click message researches it (`CM_DOUBLE_CLICK`); headings cannot be picked; the wheel scrolls 3 rows. The rows, selection and scroll position are the list window's, so the list's keys move the same selection |
| list scroll bar | thin scroll bar | drag the thumb, click the track to page |
| `RM_ResearchItem`, `RM_ClearLab`, `RM_ExitMenu`, `RM_Construction`, `RM_Launch`, `RM_ExtendedInfo` | buttons | as above |
| keys | (the screen's) | R research, L clear lab, I info, B build, S systems, C close, Escape = Close (one press) |

What is offered is unchanged: the list is `rmUpdateTechList`'s, gated by
`hwc_sim::research::available_topics` (the mission's `TechSetResearch`, the lab being built).
Research starts the topic only when it is offered and its lab is idle (the Beast's one lab for
everything), Clear lab stops the selected lab, with the original's speech.

### Layout

Both are laid out for 1080 lines: a 960 x 760 panel, centred, with its scale following the
window height (never below 0.8) but never bigger than the window, below the band where the
HUD's folded headers sit. The two panels have the same frame, header and footer, so switching
between them keeps the Build / Research and Systems buttons in place.

| Window | Scale | Panel |
|---|---|---|
| 1920x1080 | 1.0 | 960 x 760 at (480, 160); clear of the HUD's side columns |
| 1024x768 | 0.8 | 768 x 608 |
| 640x480 | 0.59 | 566 x 448 |
| 3840x2160 | 2.0 | 1920 x 1520 |

Text never goes below 8.5 px.

#### Build manager

* **Header:** "BUILD MANAGER" in letter-spaced gold caps with a soft glow, the current
  factory's name, and the Research and Systems buttons.
* **Factory tabs:** the command ship and two carrier slots, each with how many ships are
  queued there; the bottom edge fills with the progress of the ship being built. An empty
  slot is greyed out ("CARRIER 2 — NONE").
* **Left column:** CURRENT RUS and SUPPORT UNITS (gold for the units used, paler for what the
  queues and picked counts need, orange-red at the cap); the list (class headings in gold caps
  with a hairline; each ship row has its name, - count +, and the cost; selected rows have a
  gold wash and a marker, picked ships are gold, ships in production a glowing bar with the
  batch as a hairline, paused ones grey bars and a PAUSED tag, greyed ships are dim, a refused
  add flashes red); TOTAL SHIPS and TOTAL COST.
* **Right column:** the selected type rendered and turning in the player's colours inside
  gold corner brackets; its name, class, cost, support units, count in queue and build state
  (Info lays the description over the view); IN PRODUCTION, the factory's jobs.
* **Footer:** Build (the primary, gold), Pause batch, Cancel batch, and Close.

#### Research manager

* **Header:** "RESEARCH MANAGER", the selected lab's module name (as `RM_ModuleNameDraw`), and
  the Build and Systems buttons.
* **Lab tabs:** the five labs (hangar, armour, engineering, advanced engineering, weapons;
  the Beast's one lab two tabs wide). Each shows its name, what it researches (or IDLE, or
  NOT INSTALLED greyed out for a module not built) and the progress as a percentage and
  along its bottom edge; a paused lab is grey. The selected lab is the current tab (gold
  underline). On a narrow tab the percentage moves to the second line.
* **Left column:** LABS IN USE (labs researching of those built) and RESEARCHED (topics known
  of those listed); the list: lab headings in gold caps with a hairline, and each topic with
  its status on the right: AVAILABLE (gold), LAB BUSY (its lab researches something else),
  the progress with a glowing bar under the name while researching, PAUSED with a grey bar,
  RESEARCHED (green, with the original's green dot before a dimmed name), UNAVAILABLE. An
  upgrade carries a small UPGRADE tag. The selected topic has the gold wash and marker.
* **Right column:** the original's picture (`ResearchGUI\Race<n>\<TECH>.lif`, the module's
  picture for a lab, the default picture otherwise), filtered up to fill the box inside gold
  corner brackets; INFO lays the original's text over it (the topic's description, or the
  module's name). Under it the topic's name, its lab, and STATUS, TYPE, TIME and TIME LEFT;
  for a lab, its topic, status, progress and times. IN RESEARCH lists the topics being
  researched with their bars and time left.
* **Footer:** Research (the primary, gold while the selected topic can start), Clear lab, and
  Close.

### Look and behaviour

* **Panels:** rgba(8,10,14,0.75) with the 3 px #FFC800 accent on the left, a faint gold wash
  under the header, a soft shadow ring; the list sits in a slightly darker well.
* **The scene:** the 3D view around the panel is dimmed about 36%. The modern HUD's panels
  stay up and fold, or keep only their header, where they would cover the panel, by the same
  rule as over the original managers; a header click still unfolds them.
* **Text:** Segoe UI via `ab_glyph` (the game's bitmap font without it). Values are white
  with a soft gold glow; bars are the HUD's thin #FF9D00 to #FFD84A gradient with a glow.
* **Hover and press:** eased in and out (about 70 ms). Hover warms a control to gold, a press
  fills it gold with dark text; a fired - / + flashes for 0.18 s; the screen's keyboard focus
  (Tab) is a gold ring. Every button shows its hot key in a small key cap.

### What is not 1:1

* **The ship view:** the original draws the ship with its 3D engine (`svShipViewRender`). The
  modern build manager uses a small software renderer (the most detailed mesh, team-coloured
  textures, two lights, 2x2 supersampling) turning at a fixed pitch, without the engine's
  lighting, effects or nav lights; dragging to turn it is not supported.
* **The original's art:** the factory and lab tabs use names instead of the icon bitmaps
  (`Build_raceN_iconM_*.lif`, `Research_icon_*.lif`); the lab tabs have no scan line; the
  Beast's lab tab does not show the blob animation (`bresearch_ani_*`, its sound still plays
  as before); the support gauge is a bar instead of the textured icon. The research pictures
  are the original's, scaled up with filtering.
* **Rows:** the build list shows 17 rows instead of 18, and the research list 18 rows, not as
  many as the original's list window holds; each screen's list is set to that page, so
  scrolling (and the list's Page Up / Page Down) ends at the last row.
* **Hover:** the original buttons, arrows and tabs have no hover state.
* **Disabled look:** Build, Pause and Cancel look inactive when they would do nothing (nothing
  picked, nothing selected), as do Research (no topic that can start now) and Clear lab (the
  selected lab is not researching). They can still be clicked and act exactly as before; the
  original never greys them.
* **Added read-outs:** the research manager's LABS IN USE and RESEARCHED read-outs, the LAB
  BUSY and UPGRADE marks, the times (from the technology's research time, eight times as fast
  in the tutorial, as `rmUpdateResearch`) and the IN RESEARCH list are not in the original
  screen (the original shows lab progress on its buttons and the topic's bar in the list).
* **Text:** wrapped at word boundaries in Segoe UI, not the original's bitmap fonts.
* **The HUD panels at 4:3:** at 1024x768 and 640x480 the panel is wider than the gap between
  the HUD's side columns; their panels fold to headers there (at 640x480 only headers stay),
  as some already do over the original managers at 640x480.

## Menus and the systems manager

The in-game escape menu (`in_game_esc_menu.fib`: every pause menu variant, save, load, record,
the quit / delete / invalid-key confirmations, game over, the options hub and all the option
screens: audio, advanced speech, equalizer, video, custom effects, texture memory info,
gameplay, key bindings), the objectives screen (`single_player_objective.fib`, fleet
intelligence) and the systems manager (`launch_manager.fib`, the Beast's
`beast_launch_manager.fib`: the build and research managers' Systems button) in the modern
look.

| | |
|---|---|
| Switches | `--menus modern\|original`; options file `hwcModernMenus` (default 1). One switch for the menus and the systems manager |
| Global revert | `--hud original` (or `hwcModernHud = 0`) brings back the original menus too, unless `--menus` says otherwise |
| Code | `crates/hwc-ui/src/ingame/modern_proxy.rs` (the mouse handed on to the original screen), `modern_menu.rs` (the menus: the model read from the screen's atoms, layout, hit-testing, input, drawing), `modern_options.rs` (the ENHANCEMENTS screen), `modern_launch_manager.rs` (the systems manager); new controls in `modern_manager.rs` (sized frames, switches, sliders, vertical sliders, segments, drop-downs, text fields, focus marks, key caps) |
| Hooks | `ingame/mod.rs` (route the mouse, wheel and keys, tick, draw; fresh state on open; the application's own pointer kept apart), `modern_hud::manager_area` (the HUD panels keep clear of the systems manager), `options.rs` (`hwcModernMenus`), `fe.rs` (`FeAtom::layout_rect`), `apps/cataclysm`: `main.rs` (`--menus`, `--help`), `app.rs` (the command line's options shown as locked; the options applied on ACCEPT), `game.rs` (`apply_modern_options`) |
| Tests | unit tests in the four modules (layout at many sizes without overlaps, hit-testing, the proxy's mapping, drop-downs opening up or down, the switch); `crates/hwc-ui/tests/modern_menus.rs` and `modern_launch_manager.rs` (mock host: every screen at three sizes, every kind of control driven with the mouse and the keyboard); `apps/cataclysm/src/menus_shots.rs` (mission 1, game data, three sizes). The original screens' tests (`tests/ingame.rs` and the others with the original HUD) run with the original screens |
| Screenshots | `screenshots/menus/` (`mock_*` from the mock host, `m1_*` in mission 1, each at 1920x1080, 1024x768 and 640x480) |

### How they work

The original screen is started and kept open underneath, as for the managers: undrawn, but
with the keyboard, so the hot keys, Tab / Shift+Tab / the arrows between its tab stops, Home
and End, Return on its default button, Escape on its back button, Space on the focused
control, the list window's keys, key rebinding and typing into a text entry are the original
code. Its callbacks and every option rule (`cataclysm.rs`: the option store, the restore
points of Cancel and of the advanced speech / equalizer / custom effects screens, the presets,
the key list) run unchanged.

The modern screen is not a copy of the original's layout: it is **built every frame from the
original screen's atoms** and their state:

* a button is the atom with the label lying on it, and its hot key;
* a check box takes the text beside it (or over it), a radio group the text over the group and
  each button's own label, a slider the text over it and the LO / HI ends under it, a vertical
  slider the text under it, a list window the text over it; a user region that draws a figure
  beside a slider (the channel count) becomes the slider's read-out;
* the title is the topmost large text, the subtitle the one under it; the rest are section
  headings (a control goes under the nearest heading above it within the heading's columns on
  the 640x480 layout), a confirmation's lines, or (with nothing else on the screen) a page of
  text;
* the bottom row of buttons is the footer: the default button first (gold), the back button on
  the right.

Shown: whether a box is checked and which radio button is, a slider's level, the list's rows,
selection and scroll position, the keyboard focus (the original's `RSF_CURRENT_SELECTED`), the
text typed, disabled controls greyed.

The mouse goes through `modern_proxy`: each control stands for a rectangle of the original
screen (a button, a check box, one radio button, a slider, a list row, a text entry); the
pointer over it is moved onto that rectangle at the same relative place and the buttons pass
through unchanged. So the original's region processor sees exactly what it would have: a
button clicks on its release over it and a press dragged off cancels it; a check box and radio
button toggle on release with their sounds; a slider follows the pointer while held (the
original keeps the pointer on the slider until release, so a drag past its end stops at the
end); a list row selects on release and its double click is the original's double-click
message (key rebinding); a text entry takes the keyboard on a press. Only the scroll bars
(they set the list window's first row, as its own scroll bar does; a click on the track pages
with the click sound), the wheel over a list (the list window's own three-row step), the
drop-downs' opening and closing and the ENHANCEMENTS screen are handled by the modern code.

| Original control | Modern control |
|---|---|
| Button | button with its hot key in a key cap; the footer's default button gold |
| Check box | a row with the label and an on / off switch (the whole row clicks) |
| Radio group | segments (up to four short choices), else a drop-down (the speakers) |
| Horizontal slider | a row with the label, a slider with a knob, the value (percent, or the user region's figure) and the LO / HI words |
| Vertical slider (equalizer) | a bank of vertical sliders with their frequencies |
| List window | a list in a well with a thin scroll bar; key bindings in columns (group, action, key caps; NONE in red; PRESS A KEY while binding) |
| Text entry | a text field with a caret |
| Static text | title, subtitle, section headings, confirmation lines, paragraphs |
| Pop-up screens (confirmations, the invalid key box) | a smaller panel over the screen under it (which stays drawn, dimmed) |

The options hub gets one button more, ENHANCEMENTS (E), before BACK.

### The systems manager

Same frame as the build and research managers (960 x 760 at 1080 lines, scaled the same way),
with BUILD and RESEARCH in the header where the others have their switch buttons:

* **Header:** "SYSTEMS MANAGER" (the screen's own title) and the launcher's name.
* **Launcher tabs:** the command ship and two carrier slots with how many ships are docked;
  an empty slot is greyed out ("CARRIER 2 — NONE"). Each stands for `LM_MotherShipDraw`,
  `LM_Carrier1Draw`, `LM_Carrier2Draw` (`lmSelect*`, a press).
* **Left column:** FIGHTER SLOTS and CORVETTE SLOTS (used / available, a bar, orange-red when
  full); DOCKED SHIPS, the list window's rows (class headings in gold caps, each type with its
  count), selected on release as the list does; REMAIN DOCKED (the screen's own words), a switch
  per launcher for the auto-launch check boxes (`LM_AutoLaunchM`, `C1`, `C2`).
* **Right column:** for the Somtaaw command ship, MODULE STATUS: each module's name, health
  (bar coloured by the original's thresholds, and a percentage) and REPAIR PRIORITY 1 / 2 / 3
  as segments standing for the three places of `lmSlotNProcess`'s buttons (greyed without the
  upgrade); for the Beast, EMERGENCY POWER ALLOCATION: its six radio buttons as rows with a
  radio mark.
* **Footer:** LAUNCH ONE (L), LAUNCH GROUP (G, gold while a type is selected), LAUNCH ALL (A),
  CLOSE (C).

### Layout

Every menu scales with the window's height (1.0 at 1080 lines, never under 0.8 unless the
window is too small for the panel, then it fits the window); text never goes below 8.5 px. A
menu of buttons is 460 wide (a confirmation 520, its two buttons side by side); a form is 700
wide, 820 with a list or text, and goes into two columns (1000 wide) when it would be taller
than 540, splitting its sections where the columns come out most even. The panels are nearly
opaque over the dimmed scene and the HUD. The systems manager is laid out like the managers
(see their table above).

### Keyboard

The original screen's keys, unchanged: Tab and Shift+Tab, the arrows (the arrows also step a
radio group), Home / End, Space, Return (the default button, else the focused one), Escape
(the back button: one level back), the hot keys in the key caps. Added:

* **Escape in a text entry** (save, record): the entry lets go of the keyboard (the original's
  handling) and the same press then goes back, so one Escape is enough (the original needed
  two).
* **An open drop-down:** Escape closes it (and only that); any other key closes it and goes on
  to the screen.
* **E** on the options hub opens ENHANCEMENTS.

### What is not 1:1

* **The layout** is the modern one (rows, sections, columns), built from the original's atoms:
  the order is the original's columns top to bottom within each section. The custom effects
  screen's icons (`icon_*.lif`, which toggle the same option as the check box beside them)
  are not shown; their check boxes are.
* **Hover and focus:** the original controls have no hover state; the focus is drawn as a gold
  ring (buttons) or a gold mark (settings rows).
* **Sliders and check boxes without tab stops** (most of the original's) stay mouse-only, as
  in the original; the keyboard reaches what the original's tab stops reach.
* **Text:** Segoe UI, the original's words (in the current language), without the hot key's
  underline (the key is in the cap). A check box on a slider's heading line (the channels'
  AUTO) is labelled with the slider's heading ("# CHANNELS AUTO").
* **Added words:** the key list's column names and "DOUBLE CLICK OR ENTER TO CHANGE", "PRESS A
  KEY" while binding (the original shows ??????), the systems manager's NO SHIPS DOCKED and
  NO MODULES.
* **One Escape** leaves a text entry and the screen (above).
* **The systems manager:** the launcher tabs use names instead of the icon bitmaps
  (`Build_raceN_iconM_*.lif`), the slot gauges are bars, the repair-priority buttons are
  segments 1 / 2 / 3 instead of the red / green / blue bitmaps (`R1_SysN_*.lif`), the Beast's
  choices are rows instead of radio buttons beside labels; the docked list shows the original
  list window's page (11 rows).
* **Only in-game:** the front end's own option screens (from the main menu) keep the original
  look.

## The modern options

ENHANCEMENTS, reached from the options hub of the modern menus (or E there): the recreation's
own options, each an options-file key that a command-line switch can also set:

| Section | Option | Key | Command line |
|---|---|---|---|
| DISPLAY | RENDER SCALE (50..200%, a slider in steps of 5) | `hwcRenderScale` | `--render-scale` |
| | ANTI-ALIASING (MSAA): OFF / 2X / 4X / 8X | `hwcMsaa` | `--msaa` |
| | FXAA EDGE SMOOTHING | `hwcFxaa` | `--fxaa` |
| | SCALED MODES: 3D AT WINDOW RESOLUTION | `hwcSceneAtWindow` | `--scene-res` |
| CAMERA | FULL-DETAIL SHIPS AT ANY DISTANCE | `hwcFullDetailShips` | `--lod` |
| | DRAW DISTANCE: ORIGINAL / FAR | `hwcRenderList` | `--draw-distance` |
| | SMOOTH WHEEL ZOOM | `hwcSmoothZoom` | `--zoom` |
| | FOCUS AGAIN ZOOMS IN | `hwcFocusZoom` | `--focus-zoom` |
| | ALT FOCUS KEEPS THE ZOOM | `hwcAltFocusKeepsZoom` | `--alt-focus` |
| | KEEP THE CAMERA OUT OF HULLS | `hwcZoomClear` | `--zoom-limit` |
| | SENSORS CAMERA FOLLOWS THE FOCUS | `hwcSensorsFollow` | `--sensors-follow` |
| | FOCUS IN SENSORS STAYS IN SENSORS | `hwcSensorsFocusStays` | `--sensors-focus` |
| CONTROLS | RIGHT CLICK OPENS THE MOVE PLATE | `hwcRightClickMove` | `--right-click` |
| | DOUBLE CLICK SELECTS ALL OF A TYPE | `hwcDoubleClickSelectType` | |
| INTERFACE | MODERN HUD | `hwcModernHud` | `--hud` |
| | MODERN BUILD MANAGER | `hwcModernBuildManager` | `--build-ui` |
| | MODERN RESEARCH MANAGER | `hwcModernResearchManager` | `--research-ui` |
| | MODERN MENUS AND SYSTEMS MANAGER | `hwcModernMenus` | `--menus` |
| | CUTSCENE LETTERBOX: TALL / ORIGINAL | `hwcOriginalLetterbox` | `--letterbox` |

It works as the original option screens do: the changes wait until ACCEPT (A, or Return),
which keeps them, saves the options file (`options_commit`) and asks the game to apply them
(`GameRequest::ApplyOptions`); CANCEL (C, Escape) forgets them; DEFAULTS (D) puts every one
back to its default (until accepted). All of them apply at once in the running mission: the 3D
view's quality (render targets are rebuilt for the new scale and samples, MSAA limited to what
the GPU has), the camera and controls, the letterbox, and the interface switches (turning the
modern menus off shows the original options hub at once). None needs a restart (the screen can
mark one that would: "APPLIES AFTER A RESTART"). One set on the command line is shown as it is,
greyed, with "SET ON THE COMMAND LINE", and is not changed (the command line holds for the
run). The three modern managers' and the menus' switches only count with the modern HUD on
("WITH THE MODERN HUD ONLY" while it is off).

Keyboard: Tab / Shift+Tab, Up / Down move the focus (the entries, then the footer), Left /
Right step a choice or a slider (switch a toggle), Space toggles or steps on, Return accepts
(or presses the focused footer button), A / D / C the footer's buttons, Escape cancels.
