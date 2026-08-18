# hello_triangle

The fixed-timestep frame loop driving the renderer: a triangle over a
clear colour the simulation computes, in a window or into an offscreen
image.

```sh
cargo run -p renew-sample-hello-triangle --bin hello_triangle -- --frames 600
cargo run -p renew-sample-hello-triangle --bin hello_triangle -- --headless --frames 600
```

```console
$ cargo run -p renew-sample-hello-triangle --bin hello_triangle -- --headless --frames 600
renew-frame sample=hello_triangle seed=0 frames=600 ticks=600 dropped=0 schedule_hash=0x55ce27c8dcb97c4d state_hash=0x5d3ec02c32278af9
```

That line is the same on every run, in every process, and in a build
with the whole windowing stack compiled out
(`--no-default-features`) — which is the point of the sample.

## What it draws

![The triangle the sample renders: three vertex colours blended across it, on a dark backdrop](triangle.png)

Sixty-four pixels square, which is the size the headless run actually
uses -- shown at its own size rather than scaled up, because a blurred
enlargement would be a picture of something the sample never drew.

`--capture PATH` writes it:

```
hello_triangle --headless --frames 8 --capture triangle.png
```

**It is the same buffer the oracle compares**, read back the same way at
the same moment. A capture drawn through a second path would be a
picture of something no test looks at.

## What it does

`renew-frame` owns no loop. It answers one question — *given the
schedule so far and this instant, how many fixed steps are due, how many
did the budget refuse, and how far between steps is the renderer* — and
this sample is the other half of that arrangement. Both of its drivers
are the same three lines:

```rust
let plan = frame.begin_frame(now);
for step in plan.steps() { world.step(step); }
stats.absorb(&plan);
```

- **Windowed** (default): the OS owns the loop. The window seam calls
  `ready` once (device, swapchain target and pipeline are built there —
  it is the only place a live window exists), then `event` and `update`
  every iteration. The one clock read on the path is at the top of
  `update`; rendering happens on `RedrawRequested` and nowhere else.
  The title bar carries the frame-time readout (below).
- **Headless** (`--headless`): no window, no clock in the schedule. Time
  is synthetic — frame *k* happens at exactly `k × 16 666 667 ns` — so
  one step runs per frame and the whole run is a pure function of
  `(frames, seed)`.

The world is one integer value walked by a seeded stride. Its low three
bytes are the clear colour, converted with `k / 255`, which every
conformant adapter converts back to the byte `k` exactly. That is what
lets the headless test assert **every pixel** against a colour it
*computes* from the tick count, with no committed image and no refresh
ritual: run one step too many and the bytes change.

## Command line

| Flag | Meaning |
|---|---|
| `--headless` | No window: an offscreen image and a synthetic clock. |
| `--frames N` | Windowed, stop after N *presented* frames; headless, run N frames (default 600, ten seconds at 60 Hz). |
| `--seed N` | Selects the world's stride. The seed axis is a placeholder until there is a random-number service; it feeds the world so the shape of the flag survives. |
| `--dump-stats PATH` | Write the JSON report there, after the run. |

Two output channels, on purpose. Stdout carries exactly one line — the
digest line above, which the cross-process determinism gate
string-compares, needing no JSON parser and staying readable in a CI
log. The state hash in that line fingerprints behaviour only — the
seed is deliberately not folded in, so two seeds hash alike exactly
when they move the world alike, and the matrix comparing them is
measuring the simulation rather than the arithmetic of the hash.
 `--dump-stats` carries the machine-readable document:

```json
{"schema_version":1,"sample":"hello_triangle","seed":0,
 "frame":{"frames":600,"ticks":600,"steps_dropped":0,"schedule_hash":"0x…"},
 "state_hash":"0x…",
 "timing":{"count":600,"min_ns":…,"max_ns":…,"sum_ns":…,"drawn":600,"skipped":0}}
```

Everything gated lives in `frame` and `state_hash`; everything measured
lives in `timing` and is recorded, never gated.

Exit codes: `0` for a completed run or a skip, `1` for a failure, `2`
for a command line this build cannot honour.

## The frame-time readout

A windowed run shows its own cost in the title bar:

```console
renew — hello triangle — 16.67 ms (60.0 fps)
```

The engine renders no text yet, and a text renderer is a long way off,
so the title bar is the honest first version of "on
screen" — a real and conventional place for a sample to show a
measurement, and one that is visible in the task switcher too.

- **The number is a mean over the last quarter second**, not over the
  run and not the last frame alone. A run average stops moving after a
  few seconds, which is exactly when a hitch starts being interesting;
  a single frame is noise. Four relabels a second is as fast as a
  changing number can still be read, and it keeps the OS call down to
  roughly one frame in fifteen instead of one per frame.
- **The first frame is labelled immediately**, so a short run still
  shows a number and a window does not sit blank for a quarter second
  looking broken.
- **It reads no clock of its own.** The interval is measured with the
  instant `update` already took for the frame loop; there is one time
  source in this sample and the readout is not a second one.
- **It allocates nothing.** The text is written into a fixed-capacity
  buffer the readout owns and handed to the window seam borrowed — a
  `format!` per frame would break the steady-state allocation gate.
- Both numbers round to nearest rather than down, because 60 Hz is
  16 666 667 ns and truncation would report it as 16.66 ms at 59.9 fps.
  A frame too fast for the clock to resolve reads `-- fps`: no rate
  divides out of no elapsed time, and inventing one would put the only
  lie on the window.

Headless runs have no window and no readout; their frame times live in
the `timing` section of `--dump-stats` and nowhere else.

## Contract

- **`--headless` implies a synthetic time source.** Nothing measured
  reaches the schedule, the state hash or the digest line. The one clock
  the headless driver reads brackets each frame for the timing summary,
  which is the frame-time readout the allocation gate below exists to
  keep out of the frame path.
- **The simulation is integer-only.** Bit-determinism is scoped to one
  platform, and the transcendental functions differ between platform
  math libraries; a world holding an angle would quietly make the state
  hash a cross-platform promise the engine does not make. If the
  triangle ever spins, the angle is a tick count and the trig happens in
  the shader — render, not simulation.
- **Steady state is frames `[3, N)` of a headless run, and it allocates
  nothing.** Everything that allocates happens before frame zero:
  device, target, pipeline, and the readback buffer. Inside the boundary
  there is no file I/O, no logging and no serialization —
  `--dump-stats` writes after the loop exits — and the one thing that
  formats text, the title readout, formats into a buffer it owns.
- **An environment that cannot host the run is a skip, not a failure.**
  No GPU runtime and no display server are ordinary answers on ordinary
  machines: the binary prints `SKIP:` and exits zero. Set
  `RENEW_FRAME_STRICT=1` on a lane that exists to run this, and a skip
  becomes a failure — a lane that passes by skipping proves nothing.
- **A dormant window is not an error.** A minimized window or a stale
  swapchain presents nothing; the frame is counted as skipped (so it
  cannot inflate the frame-time summary), the simulation keeps stepping,
  and the target is rebuilt at the size the app knows. A run that stops
  presenting altogether for five seconds ends by saying it is wedged,
  rather than spinning forever.

## Two seam properties, stated so nobody "fixes" them

- `update` runs *after* the event phase, so a redraw requested in
  iteration N arrives in N+1: the render lags the step phase by one
  iteration and draws with the alpha stored then. Harmless — alpha is a
  hint, and an OS repaint with no intervening update correctly re-renders
  at the same alpha.
- `WindowApp::event` receives no loop control, so a close request cannot
  exit the loop where it arrives. It is latched and acted on in `update`.

## Removability

`renew-frame` has no dependency on the renderer, and this sample is
where the two meet. Built with `--no-default-features` there is no
windowing library and no window-system integration anywhere in its
graph — `renew-frame`, `renew-platform`, `renew-event` and `renew-rhi`
only — and the
same binary produces the same digest line as the windowed build's
headless mode. Asking that build for a window exits non-zero and says
why.

## Tests

| File | What it proves |
|---|---|
| `tests/headless_frame.rs` | The readback holds the colour the world computed for the last tick; one more step is a different image; the triangle covers the middle and one tick drawn twice is the same bytes. |
| `tests/cli_determinism.rs` | Three separate processes print one digest line; a different frame count or seed prints a different one; four seeds each reproduce themselves and none of them hash alike; the stats file agrees with the line. |
| `tests/zero_alloc.rs` | The steady-state frame path performs no heap allocation, and neither does the title readout. |

The unit tests beside the source drive the window callbacks directly —
`event`, `update`, `surface_lost`, the draw and stall verdicts — with no
window at all. Only `ready` needs one, because it borrows a live OS
window. `surface_lost` is driven here of necessity: no desktop platform
emits a suspend, so this suite is the only place the gap between one
surface epoch and the next is exercised at all. The
readout's text is a pure function of two numbers, so it is tested
directly at both ends of the range: a frame too fast for the clock to
resolve, and one of `u64::MAX` nanoseconds.

## iOS

The same shape as Android, with a doorway that never returns.
`src/ios.rs` is entered from `main` when the run wants a window, and
hands to the event loop, which enters `UIApplicationMain` and owns the
process from then on. A run that asked for `--headless` takes the
command line instead — `simctl spawn` executes this binary directly, and
a windowed loop with no application around it traps.

**No Xcode project.** An iOS application is a directory: an executable
plus an `Info.plist` naming it, which `tools/ios-app-bundle.sh`
assembles. A *device* build would differ — installing on hardware needs
signing and provisioning — and this sample does not do one.

```
bash tools/ios-app-bundle.sh renew-sample-hello-triangle hello_triangle com.renewengine.hellotriangle target/ios-present
xcrun simctl install booted target/ios-present/hello_triangle.app
xcrun simctl launch booted com.renewengine.hellotriangle
```

Vulkan on this platform is MoltenVK, translating to Metal. The engine
opens its runtime by dlopening `libvulkan.dylib`, so the library ships
under that name and is found through `DYLD_LIBRARY_PATH`; the vendored
copy and its provenance are in `third_party/moltenvk/`.

**What presents today, and what does not look right yet.** A frame does
reach the screen. It covers about a third of the window in each
direction and sits against an edge, because the swapchain is built at
the window's logical size rather than its physical one — a known defect,
not a property of the platform. Both need macOS with Xcode's
command-line tools; a simulator build needs no signing identity.

## Android

The renderer's first phone build. The library gains a `cdylib` crate
type and a second doorway beside `main.rs` — `src/android.rs` defines
the `android_main` the activity looks up by name, wraps the sample in a
logging adapter, and sends every line the desktop prints to a log in
internal storage, ending with the same verdict the desktop prints.

What the triangle proves there is not the triangle: it is that the
renderer survives a platform that takes windows away. Backgrounding
closes the surface epoch, and the sample drops its pipeline, surface,
window handles and frame loop before the callback returns — the seam
verifies the release — while the device, the expensive half, survives
to serve the next epoch. Coming back rebuilds only what belonged to the
old surface.

Two commands produce an installable debug APK (the second from
`android/`):

```
cargo ndk -t arm64-v8a -t x86_64 -o samples/hello_triangle/android/app/src/main/jniLibs build -p renew-sample-hello-triangle --release
./gradlew assembleDebug
```

`cargo-ndk` needs `ANDROID_NDK_HOME` pointing at an r28+ NDK; Gradle
needs `JAVA_HOME` and `ANDROID_HOME`. The artifact lands under
`android/app/build/outputs/apk/debug/` debug-signed, and the log is at
`adb shell run-as com.renewengine.hellotriangle cat files/hello_triangle.log`.

**The activity is orientation-locked, deliberately.** A surface whose
panel is rotated reports a transform the swapchain declares and no
renderer folds yet, so unlocked rotation would present the scene
sideways. The window target reports that transform now
(`WindowTarget::transform`) and the fault layer can fake one, so the
fold is buildable and testable — it simply is not built yet. The lock
narrows when a rotation can appear; it does not promise none can, since
a panel whose natural orientation is the other one reports a quarter
turn even under a lock. That is precisely why the transform is reported
rather than assumed.
