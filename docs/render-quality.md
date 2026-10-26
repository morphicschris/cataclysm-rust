# 3D view quality options

Modern options for the in-game 3D view. All of them are off by default, so the game looks
as the original does unless you turn one on. Each can be set on the command line or in the
options file (`config/hwc-options.cfg`). None of them is in the in-game options screen,
because the original's screens are laid out by its own `.fib` files and have no free slot.

| Option | Flag | Options-file key | Default |
|---|---|---|---|
| Internal render scale | `--render-scale 0.5..2.0` | `hwcRenderScale = 50..200` (percent) | 1.0 / 100 |
| MSAA | `--msaa 1\|2\|4\|8` | `hwcMsaa = n` | 1 (off) |
| FXAA | `--fxaa on\|off` | `hwcFxaa = 1/0` | off |
| Scene at window resolution in scaled front-end modes | `--scene-res window\|layout` | `hwcSceneAtWindow = 1/0` | window |
| Smooth wheel zoom | `--zoom smooth\|original` | `hwcSmoothZoom = 1/0` | smooth (asked for by the user) |
| Cutscene / fleet-intel letterbox at least as tall as on a 4:3 screen | `--letterbox tall\|original` | `hwcOriginalLetterbox = 0/1` | tall (asked for by the user: the original's 1.85:1 bars are 18-21 pixels on a 16:9 screen) |
| Default music volume | (options screen) | `MusicVolume = 0..100` | 45, the original's 60 lowered 25% at the user's request; a saved value is kept |
| Frame profile | `--profile` or `HWC_PROFILE=1` | - | off: prints a per-section CPU / GPU breakdown once a second |

## The resolution the 3D view is drawn at

- **`--scale original` (the default).** The canvas is the window: the configured
  resolution, or the monitor's size with borderless full screen (F11). The 3D view is drawn
  at exactly those pixels, 1:1, so it is already at the real window resolution, widescreen
  included. Nothing changed here.
- **`--scale fit`, `integer` or `stretch`.** These scale the 640x480 front-end layout.
  Before this change the mission's whole frame, the 3D view included, was drawn at 640x480
  and enlarged. Now the 3D view is drawn at the viewport's real pixels (for example
  1200x900 in a 1600x900 window with `fit`), and the 640x480 interface is drawn over it
  with the same scaling as before. The camera keeps the layout's aspect ratio, and pixel-sized
  things (stars, lines, sprites) keep their size relative to the layout, so the framing, the
  FOV and the picking are unchanged. This is the default because it only makes the picture
  sharper. Two limits: where the interface draws over the scene, or a full-screen fade
  covers it, those pixels come from the 640x480 frame. `--scene-res layout` brings back the
  old behaviour.

## How each option works

- **Render scale.** The scene, effects, trails and nebulae are drawn offscreen at the
  window size times the scale. The result is filtered back to the window size on the GPU:
  shrinking uses an exact area-weighted box filter, and enlarging (a scale below 1) uses
  bilinear filtering. The HUD and text are drawn afterwards at native resolution. Line
  widths, star sizes and the camera framing follow the unscaled size, so the picture only
  gets smoother, never thinner or wider. A whole-number scale (2.0) gives the crispest
  result. A fractional scale, including one cut by the budget below, is a little softer.
- **MSAA.** Every scene pipeline is built for the chosen sample count: ships, sky, stars,
  nav lights, engine glows, additive and alpha sprites, lines, trails, screen-space flares,
  the hwc-fx effects and the nebulae. The samples are resolved once, at the end of the
  scene. Lens-flare occlusion uses the game's collision-box ray tests and reads no depth,
  so it works the same with MSAA. Counts other than 1 and 4 are only offered when the
  adapter supports them for both the colour and the depth format. The device is opened
  with the adapter's own format features to allow this. An unsupported count falls back to
  the next lower one and prints a note.
- **FXAA.** A cheap post filter over the finished picture (after scaling). It smooths
  edges, and it also softens some fine texture detail.

## Driver-reset (TDR) safety

The scene targets are planned by `hwc_render::quality::plan`:

- No target may be larger than the device's `max_texture_dimension_2d`.
- One frame may draw at most **16.6 M samples** (render pixels x MSAA samples; that is
  3840x2160 with 2x MSAA). Above that budget the render scale is reduced first, but not
  below 1 unless you asked for less. Then the MSAA samples are halved. Only after that does
  the scale drop below 1, and only for windows larger than 4K. A note is printed when this
  happens. For example, at 1600x900, scale 2 with 4x MSAA becomes scale 1.70 with 4x
  (2715x1527), and scale 2 with 8x becomes scale 1.20 with 8x (1920x1080).

The heaviest frame measured on the Intel UHD 750 is about 30 ms, far below the two-second
limit at which Windows resets the driver.

## Measured costs (Intel UHD 750, driver 101.5592, Vulkan)

These are times for Mission 1 combat at 1600x900, drawn by the game's own frame path
(`Game::render`: the 3D view, the read-back, the HUD). Each mode draws the same frame 48
times. The numbers come from the ignored test `quality_shots::combat_frame_times_and_shots`
(`cargo test -p cataclysm --release combat_frame_times -- --ignored --nocapture`). Run A
waits for the first fight, 41 effects at update 48. Run B uses `HWC_AA_MIN_FX=120`, a larger
fight with 133 effects at update 704. Times vary by about 2 ms from run to run.

| Mode | Drawn at | A mean / p95 (ms) | B mean / p95 (ms) |
|---|---|---|---|
| off (original) | 1600x900 1x | 13.9 / 15.2 | 13.2 / 14.1 |
| scale 0.5 | 800x450 1x | 15.0 / 16.8 | 14.3 / 15.3 |
| scale 1.5 | 2400x1350 1x | 19.9 / 21.2 | 18.1 / 18.8 |
| scale 2 | 3200x1800 1x | 21.9 / 23.7 | 19.4 / 20.3 |
| FXAA | 1600x900 1x | 17.5 / 18.8 | 15.6 / 16.4 |
| MSAA 2 | 1600x900 2x | 18.4 / 19.6 | 15.4 / 15.9 |
| MSAA 4 | 1600x900 4x | 20.0 / 21.5 | 15.6 / 16.4 |
| MSAA 8 | 1600x900 8x | 21.6 / 23.3 | 17.5 / 18.8 |
| scale 1.5 + MSAA 4 | 2400x1350 4x | 23.1 / 28.7 | 23.1 / 24.3 |
| scale 2 + MSAA 2 | 3200x1800 2x | 25.9 / 27.2 | 22.6 / 24.2 |
| scale 2 + MSAA 4 (cut) | 2715x1527 4x | 25.6 / 27.6 | 22.1 / 22.7 |
| scale 2 + MSAA 8 (cut) | 1920x1080 8x | 24.6 / 26.4 | — |
| `--scale fit`, scene at layout (old) | 640x480 1x | 12.3 / 13.4 | 10.8 / 11.5 |
| `--scale fit`, scene at window (new default) | 1200x900 1x | 15.8 / 17.3 | 13.3 / 14.7 |

About 12 to 13 ms of the original frame is fixed CPU work: the read-back, compositing and
the HUD. Scale 0.5 saves no time, because the resampling pass costs as much as it saves.
At 60 Hz the frame budget is 16.7 ms. MSAA 2 or 4 or FXAA fit within it, or nearly. Scale
1.5 or 2 brings the frame rate to about 45 to 50 fps on this GPU. `--frame-times` prints
the same measurements while you play (use `--no-vsync` to see times below the refresh
interval). For example, Mission 2 in a 1600x900 window with `--scale fit --msaa 4
--render-scale 1.5` measured a mean of 15 to 19 ms.

Screenshots for comparison are in `screenshots/aa/`:

- `m01-combat-{off,scale-2,msaa-4,fxaa,scale-2-msaa-4}.png`
- `compare-zoom-off-2x-msaa4-fxaa.png`: a 3x crop.
- `quality-*.png`: the synthetic test scene in every mode.

## Smooth wheel zoom

The original (`cameraControl`; HW1's `camera.c` is the same) multiplies the camera distance
by `CAMERA_WHEEL_ZOOM_IN` (0.70) or `CAMERA_WHEEL_ZOOM_OUT` (1.45), scaled by the mouse
sensitivity, once per notch. The result is clamped to the far limit and to the closest zoom
(or the focused ship's minimum). Nothing eases the distance. `CameraCommand`'s tracking
eases the look-at point and the angles, but copies the distance straight across.

With the smooth zoom, each notch applies the same factor and the same clamps to a target
distance, and notches add up. The camera then eases there exponentially in log distance
with a 0.08 s time constant: 95% of the way in 0.24 s, whatever the frame rate. The
wheel works the same in the sensors manager, where it moves the sensors camera within its
own limits. A key or mouse-drag zoom takes over from the point the easing has reached.
`--zoom original` restores the immediate step. This changes only rendering and the camera;
the simulation is not affected.
