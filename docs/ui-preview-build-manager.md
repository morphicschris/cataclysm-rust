# Build manager in the modern HUD style (preview)

A restyled build manager (`construction_manager.fib`) to look at before deciding whether it
replaces the original's look. It is **off by default** and isolated:

| | |
|---|---|
| Switch | `--build-ui modern` (or `original`), options file `hwcModernBuildManager = 1` (default 0) |
| Preview build | `builds\cataclysm-buildui.exe` = `cargo build --release -p cataclysm --features buildui`; it starts with the modern build manager (`--build-ui original` turns it off again) |
| Code | `crates/hwc-ui/src/ingame/modern_build_manager.rs` (layout, hit-testing, input, drawing), `apps/cataclysm/src/ship_preview.rs` (the ship render) |
| Hooks | `ingame/mod.rs` (route mouse, tick, draw, ship view while active), `consmgr.rs` (a few functions made `pub(crate)`, `list_event` split out of `cmSelectAvailable`, `scroll_to`), `modern_hud::manager_area` (the HUD panels keep clear of the new panel), `options.rs` (the key), `game.rs` (renders the ship view), `main.rs` / `Cargo.toml` (switch, `buildui` feature) |
| Tests | unit tests in the module (layout at many sizes, hit-testing, thumb drag, drawing helpers); `crates/hwc-ui/tests/modern_build_manager.rs` (mock host, every control); `apps/cataclysm/src/buildui_shots.rs` (Mission 1, game data) |
| Screenshots | `screenshots/buildui/` (`m1_build_manager_*`, `m1_build_manager_info_*`, `m1_build_manager_built_*`, and `mock_*`) |

## How it works

The original screen is still started and kept open underneath. It is not drawn and gets no
mouse events, but it keeps the keyboard, so the hot keys (B, P, L, C, I, R, S), Return and
Escape (its default buttons), the Tab / arrow focus, the callbacks and every rule in
`consmgr.rs` stay the same code. Picked counts, live queue edits, unit caps, support units,
the red flash on a refusal, speech and sounds all work as before. The new panel draws over
the 3D view and turns mouse input into the calls the original's regions make:

| Original region | Modern control | Behaviour kept |
|---|---|---|
| `CM_MotherShipDraw`, `CM_Carrier1Draw/2` (`cmSelectMotherShip`...) | factory tabs | a press selects; an empty slot is disabled; the second press of a double click is not seen |
| `CM_Ships` (`cmSelectAvailable`) | ship rows | left: select, or add when already selected; Shift toggles the selection; Ctrl adds 5; right: remove (and select); the double-click message acts once more (`WM_LBUTTONDBLCLK`); the wheel scrolls 3 rows |
| `CM_LeftArrowDraw` / `CM_RightArrowDraw` | - / + boxes | a press fires, holding repeats every 0.25 s (on the row under the pointer), Ctrl means 5, a double click's second press is ignored |
| `CM_Scroller` | thin scroll bar | drag the thumb, click the track to page |
| `CM_BuildShips`, `CM_PauseJobs`, `CM_CancelJobs`, `CM_Close`, `CM_Research`, `CM_Launch`, `CM_ExtendedInfo` | buttons | click sound on the press; the callback runs on release over the button (`uicButtonProcess`); never on the second press of a double click |
| keys | (the screen's) | B build, P pause/resume, L cancel, C close, I info, R research, S systems, Return = Build, Escape = Close (one press; the key-up fix stays) |

Clicks on the modern HUD's panels are still taken by those panels first, as they are over
the original manager. Clicks outside the panel do nothing (the original covered the screen).

## Layout

The layout is made for 1080 lines: a 960 x 760 panel, centred, with its scale following the
window height (never below 0.8) but never bigger than the window. The panel stays below
the band where the HUD's folded headers sit. The sizes it comes out at:

| Window | Scale | Panel |
|---|---|---|
| 1920x1080 | 1.0 | 960 x 760 at (480, 160); clear of the HUD's side columns |
| 1024x768 | 0.8 | 768 x 608 |
| 640x480 | 0.59 | 566 x 448 |
| 3840x2160 | 2.0 | 1920 x 1520 |

Text never goes below 8.5 px. From top to bottom:

* **Header:** "BUILD MANAGER" in letter-spaced gold caps with a soft glow, the current
  factory's name, and the Research and Systems buttons.
* **Factory tabs:** the command ship and two carrier slots. Each tab shows how many ships
  are queued there. Its bottom edge fills with the progress of the ship being built (the
  current tab's gold underline). An empty slot is greyed out ("CARRIER 2 — NONE").
* **Left column:**
  * **Readouts:** CURRENT RUS (the HUD's RU bar), and SUPPORT UNITS. Its bar is gold for
    the units used, paler for what the queues and picked counts need. The value includes
    them, turning orange-red at the cap.
  * **The list:** class headings in gold caps with a hairline. Each ship row has its name,
    then - count +, then the cost.
    * A selected row has a gold wash and a marker on its left edge.
    * A picked (not yet built) ship is gold.
    * A ship in production has a glowing progress bar under its name, with the batch
      progress as a hairline below it.
    * A paused one shows grey bars and a PAUSED tag.
    * A greyed (unbuildable) ship is dim and its + is disabled. The - is disabled when the
      count is 0.
    * A refused add flashes the row red.
  * **Totals:** total ships and total cost.
* **Right column:**
  * **Ship view:** the selected type, rendered and turning slowly in the player's team
    colours, inside gold corner brackets.
  * **Details:** the ship's name (large, glowing), its class, and cost, support units,
    count in queue and build state. Info lays the original's description (firepower,
    armour, speed, required tech...) over the view.
  * **In production:** the current factory's jobs, with progress bars.
* **Footer:** Build (the primary, gold), Pause batch and Cancel batch on the left, Close on
  the right. Every button shows its hot key in a small key cap.

## Look and behaviour

* **Panels:** the panel is rgba(8,10,14,0.75) with the 3 px #FFC800 accent on its left, a
  faint gold wash under the header, and a soft shadow ring. The list sits in a slightly
  darker well, so it reads over a busy scene.
* **The scene:** the 3D view around the panel is dimmed about 36%. The modern resources,
  objectives, build queue, log and selected panels stay up. They fold, or keep only their
  header, where they would cover the panel, by the same rule as over the original manager,
  with the panel as the area to keep clear. A header click still unfolds them.
* **Text:** Segoe UI via `ab_glyph` (the game's bitmap font without it). Values are white
  with a soft gold glow. Bars are the HUD's thin #FF9D00 to #FFD84A gradient with a glow.
* **Hover and press:** they ease in and out (about 70 ms). Hover warms a control to gold,
  and a press fills it gold with dark text. A press dragged off a button cancels it. A
  fired - / + flashes for 0.18 s. The screen's keyboard focus (Tab) is drawn as a gold
  ring.

## What is not 1:1

* **The ship view:** the original draws the ship with its 3D engine (`svShipViewRender`).
  The preview uses a small software renderer: the most detailed mesh, its team-coloured
  textures, two lights and 2x2 supersampling. It turns slowly at a fixed pitch, without the
  engine's lighting, effects or nav lights. Dragging to turn it is not supported.
* **The original's art:** the factory tabs use the factory names instead of the race's
  icon bitmaps (`Build_raceN_iconM_*.lif`). The support gauge is a horizontal bar instead
  of the vertical textured icon (`b_Build_Support*.lif`).
* **Rows:** the list shows 17 rows instead of 18. The screen's virtual scroll list is set
  to that page, so scrolling ends at the last row.
* **Hover:** the original buttons and arrows have no hover state. The bitmap buttons' hover
  art is not used.
* **Disabled look:** Build, Pause and Cancel look inactive when they would do nothing
  (nothing picked, nothing selected). They can still be clicked and act exactly as before.
  The original never greys them.
* **The Info text:** it is wrapped at word boundaries in Segoe UI, not the original's
  bitmap font.
* **The HUD panels at 4:3:** at 1024x768 and 640x480 the panel is wider than the gap
  between the HUD's side columns. Their panels fold to headers there (at 640x480 only
  headers stay), as some already do over the original manager at 640x480.
