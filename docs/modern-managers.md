# Build and research managers in the modern HUD style

The build manager (`construction_manager.fib`) and the research manager
(`research_manager.fib`, the Beast's `beast_research_manager.fib`) restyled in the modern HUD's
look. A user-requested change, **on by default**, with switches back to the original screens:

| | |
|---|---|
| Switches | `--build-ui modern\|original`, `--research-ui modern\|original`; options file `hwcModernBuildManager` and `hwcModernResearchManager` (default 1) |
| Global revert | `--hud original` (or `hwcModernHud = 0`) brings back the original HUD and both original managers, unless `--build-ui` / `--research-ui` says otherwise |
| Code | `crates/hwc-ui/src/ingame/modern_manager.rs` (the shared look: panel placement and scale, frame, title, tabs, buttons and key caps, rows, bars, scroll bar, readouts, cells, picture box, text pane, hover easing, the double-click rule, the switches), `modern_build_manager.rs` and `modern_research_manager.rs` (layout, hit-testing, input, drawing of each), `apps/cataclysm/src/ship_preview.rs` (the build manager's ship render) |
| Hooks | `ingame/mod.rs` (route mouse, tick, draw, ship view while active; fresh state on open), `consmgr.rs` and `research.rs` (a few functions made `pub(crate)`), `modern_hud::manager_area` (the HUD panels keep clear of the panel), `options.rs` (the keys), `game.rs` (renders the ship view), `main.rs` (the switches, `--help`) |
| Tests | unit tests in the three modules (layout at many sizes, hit-testing, thumb drag, drawing helpers, hover easing, double clicks, the switches); `crates/hwc-ui/tests/modern_build_manager.rs` and `modern_research_manager.rs` (mock host, every control); `apps/cataclysm/src/buildui_shots.rs` and `resui_shots.rs` (Mission 1, game data). The original managers' tests (`tests/ingame.rs`, the HUD tests over a manager, the game-path smoke test) run with the original screens |
| Screenshots | `screenshots/buildui/` (`m1_build_manager_*`, `mock_*`) and `screenshots/resui/` (`m1_research_manager_*`, `_researching_`, `_info_`, `_lab_`, and `mock_*`) |

## How they work

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
button (and B) comes back to the modern build manager; Systems (S) opens the launch manager.

### Build manager

| Original region | Modern control | Behaviour kept |
|---|---|---|
| `CM_MotherShipDraw`, `CM_Carrier1Draw/2` (`cmSelectMotherShip`...) | factory tabs | a press selects; an empty slot is disabled; the second press of a double click is not seen |
| `CM_Ships` (`cmSelectAvailable`) | ship rows | left: select, or add when already selected; Shift toggles the selection; Ctrl adds 5; right: remove (and select); the double-click message acts once more; the wheel scrolls 3 rows |
| `CM_LeftArrowDraw` / `CM_RightArrowDraw` | - / + boxes | a press fires, holding repeats every 0.25 s (on the row under the pointer), Ctrl means 5, a double click's second press is ignored |
| `CM_Scroller` | thin scroll bar | drag the thumb, click the track to page |
| `CM_BuildShips`, `CM_PauseJobs`, `CM_CancelJobs`, `CM_Close`, `CM_Research`, `CM_Launch`, `CM_ExtendedInfo` | buttons | as above |
| keys | (the screen's) | B build, P pause/resume, L cancel, C close, I info, R research, S systems, Return = Build, Escape = Close (one press) |

### Research manager

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

## Layout

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

### Build manager

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

### Research manager

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

## Look and behaviour

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

## What is not 1:1

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
