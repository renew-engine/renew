# input_echo

Window input feeding a fixed-timestep world — with no renderer anywhere
in its dependency graph.

```sh
cargo run -p renew-sample-input-echo --bin input_echo -- --frames 600
cargo run -p renew-sample-input-echo --bin input_echo -- --headless --input-trace walk
```

```console
$ cargo run -p renew-sample-input-echo --bin input_echo -- --headless --input-trace walk
renew-frame sample=input_echo seed=0 source=walk frames=20 ticks=20 dropped=0 schedule_hash=0x64972ba6d4f30765 state_hash=0x833e49c9dd637b92
```

## What it does

Hold a direction key (WASD or the arrows) and a position moves — one
speed per *tick*, never per event and never per frame. That is the whole
point of a fixed timestep, and it is why key repeats are counted and
then ignored: acting on them would make movement depend on the
keyboard's repeat rate. Escape and the close button both end the run.
Everything the window seam delivers is echoed as it arrives.

Events arrive whenever the OS says so; the world advances only in fixed
steps. The two drivers differ only in where the events come from:

- **Windowed** (default): real input, the real clock, one clock read per
  iteration at the top of `update`.
- **Headless** (`--headless --input-trace NAME`): a recorded trace
  replayed against the same state machine, on a synthetic clock that
  reads no clock at all. No CI runner supplies keystrokes, so without
  this the binary could never be executed by a test — and an unexecuted
  binary is an untested one.

The scripted traces are files, in `traces/`, embedded at compile time
and read by the same parser that reads anything this sample records.
They are plain text and meant to be read: `walk.trace` is a header and
fourteen events. Each one is also exactly what recording it produces, so editing
one and re-recording is how you check you got what you meant.

Traces: `walk` (keys, pointer, wheel, focus and resize, ending in a
close request) and `idle` (no input at all — the shape a dedicated
server or a determinism harness has).

A trace file's header carries the run it was captured from — seed, tick
count, timestep. On `--replay-trace` that header **owns** the run. On
`--input-trace` it is provenance only, and the command line decides:
that is what lets one trace be replayed across many seeds.

## Command line

| Flag | Meaning |
|---|---|
| `--headless` | No window: replay a scripted trace on a synthetic clock. |
| `--input-trace NAME` | Which trace to replay (default `walk`). Refused outside `--headless`: a window is driven by the person at the keyboard. |
| `--frames N` | Frames of simulation to run (default 600, ten seconds at 60 Hz). Headless, one frame is exactly one step; windowed, the run ends once the simulation has advanced this many steps. A close request ends it sooner. |
| `--seed N` | Selects the movement speed. The seed axis is a placeholder until there is a random-number service; it feeds the world so the shape of the flag survives. |
| `--dump-stats PATH` | Write the JSON report there, after the run. |
| `--record-trace PATH` | Write the input this run saw to a trace file. Requires `--headless`, and refused alongside `--replay-trace`: this sample records what a script produced, so there is nothing to record from a window, and re-recording a replay would write back the file it just read. |
| `--replay-trace PATH` | Run the input in that file instead. The header owns the run, so `--input-trace`, `--frames` and `--seed` are refused alongside it. Requires `--headless`: replaying against a live window would mix recorded input with real input. |

The last line on stdout is always the digest line — the string the
cross-process determinism test compares. `--dump-stats` writes the
machine-readable document: the frame schedule, the state digest,
and what the input added up to.

There is **no `timing` section**, deliberately: this sample presents
nothing, and a drawn-versus-skipped split for a loop that never draws
would be a measurement of nothing.

Exit codes: `0` for a completed run or a skip, `1` for a failure, `2`
for a command line this sample cannot honour.

The two trace flags also have a front door that does not require
remembering them:

```
renew record --output run.trace input_echo --headless --input-trace walk
renew replay --input  run.trace input_echo --headless
```

The two digest lines are not identical — the replay reports
`source=replay` where the recording reported `source=walk`, which is how
you can tell it read the file rather than re-running the script. Every
field that determinism depends on **is** identical, `state_hash` and
`schedule_hash` included. That is the whole claim: a replay of a
recording reproduces the run that made it.

## Contract

- **`--headless` implies a synthetic time source.** The scripted driver
  reads no clock at all, so its digest line is a pure function of
  `(trace, frames, seed)` and is identical across runs, processes and
  machines.
- **Events change what the next step does, never the state.** The world
  is advanced by `step` and by nothing else.
- **The world counts input; it does not interpret it.** Which physical
  key means "left" is decided by a binding table in the driver, and what
  reaches the simulation is resolved intent — two axes, already OR-ed
  across both keys for a direction and already cancelled where opposites
  are held. The world still tallies raw events, because echoing input is
  what this sample is for, but a binding table is a fact about the
  machine someone is sitting at: it does not belong in simulation state,
  it must not appear in a recording, and it should not have to be `Copy`
  merely because the world is.
- **The state hash is a fingerprint of behaviour, not of configuration.**
  The seed is deliberately not folded into it. A digest that absorbs an
  input cannot show that input had an effect — every seed would produce
  its own number even if the seed never reached the simulation. Leaving
  it out means two digests differ only when two worlds differ, which is
  what lets the seed matrix assert something real. The configuration is
  still printed: the seed appears beside the hash on the same line.
- **The simulation is integer-only.** Positions are whole units and
  pointer coordinates are truncated on arrival: a state hash that
  absorbed float arithmetic would quietly become a cross-platform
  promise the engine does not make.
- **No display server is a skip, not a failure.** The windowed run
  prints `SKIP:` and exits zero where no window can be opened; set
  `RENEW_FRAME_STRICT=1` where a skip would be a lie.
- **Windowing is not a feature flag here.** This *is* the windowing
  sample, so the seam is always compiled in — a feature that can never
  be turned off would be a lie in the manifest. Headless mode compiles
  the same event vocabulary and simply opens no window.

## Why it exists twice over

It is the complementary proof to `hello_triangle`: a *running*
frame-loop sample in a workspace with the GPU crate removed. Its
dependencies are `renew-frame`, `renew-input`, `renew-platform` and
`renew-trace` — four, where this sentence said two until 2026-08-01,
having been written before the last two were added.

## Tests

`tests/cli_determinism.rs` runs the real binary three times and compares
the digest lines, checks that a different trace or seed moves them, and
reads back the stats file. It also runs a seed matrix: four seeds, three
processes each, where every seed must reproduce itself exactly and no two
seeds may hash alike. The second half is the one that catches a seed
which is read and then ignored — a failure identity alone cannot see.
`RENEW_DETERMINISM_RUNS` deepens the matrix without editing the file.

It needs no adapter and no compositor, so unlike its sibling it never
skips: on every machine and every lane, those assertions actually run.

The unit tests beside the source drive the window callbacks directly —
`event` and `update` with no window at all. Only `ready` needs one,
because it borrows a live OS window.

## Android

The sample is also the engine's first APK. The library gains a second
crate type (`cdylib`) and a second doorway beside `main.rs`:
`src/android.rs` defines the `android_main` the activity looks up by
name, wraps the same `EchoApp` in a logging adapter, and sends every
line the desktop prints to a log in the app's internal storage instead —
a phone has no terminal anyone watches, so the pulled file is the
sample's report: `ready`, every event as its one-line description, each
surface epoch closing as the app is backgrounded, and the loop's
outcome if one comes.

Two commands produce an installable debug APK (run the second from
`android/`):

```
cargo ndk -t arm64-v8a -t x86_64 -o samples/input_echo/android/app/src/main/jniLibs build -p renew-sample-input-echo --release
./gradlew assembleDebug
```

Two ABIs, because an APK with only `arm64-v8a` can be installed on a
phone and on nothing else. `x86_64` is what an emulator runs, and it is
what makes the app's lifecycle testable without hardware: install it,
send it to the background, bring it back, and the surface epochs are
observable in the log this sample writes.

## iOS

The same shape once more, with a doorway that never returns.
`src/ios.rs` is entered from `main` on that target and hands straight to
the event loop, which enters `UIApplicationMain` and owns the process
from then on. It wraps the same `EchoApp` in the same kind of logging
adapter, writing to `Documents` inside the app's sandbox — read back
from the host, since a simulator's filesystem is this machine's.

**No Xcode project, and that is a decision rather than an omission.** An
iOS application is a directory: an executable plus an `Info.plist`
naming it. A Rust `[[bin]]` built for `aarch64-apple-ios-sim` is that
executable, so `tools/ios-app-bundle.sh` assembles the bundle and
nothing generates or parses a `project.pbxproj`. A *device* build would
be different — installing on hardware needs signing and provisioning,
which is where a real project earns its keep — and this sample does not
do one.

```
bash tools/ios-app-bundle.sh renew-sample-input-echo input_echo com.renewengine.inputecho target/ios-app
xcrun simctl install booted target/ios-app/input_echo.app
xcrun simctl launch booted com.renewengine.inputecho
```

The log is at `$(xcrun simctl get_app_container booted com.renewengine.inputecho
data)/Documents/input_echo.log`. Launching another app (Settings, say)
backgrounds this one, and relaunching brings it back: the log then
carries `suspended` and `resumed` lines and — unlike Android — no
`surface lost` between them, because iOS keeps the window across an
interruption.

Both of these need macOS with Xcode's command-line tools; a simulator
build needs no signing identity.

`cargo-ndk` (pinned 4.1.2) needs `ANDROID_NDK_HOME` pointing at an
r28+ NDK — r28 aligns segments to the 16 KB pages newer devices
require, by default. Gradle needs `JAVA_HOME` and `ANDROID_HOME`; the
committed wrapper pins Gradle itself, the shell's build file pins the
Android plugin, and `assembleDebug` signs with the SDK's debug key, so
the artifact under `android/app/build/outputs/apk/debug/` is ready for
`adb install` with no further ceremony. The shell is owned source —
edited, never regenerated. `jniLibs/` is a build product and ignored.

The app draws nothing yet — rendering reaches Android in a later
change — so the screen is the system's; the log is where the sample
speaks: `adb shell run-as com.renewengine.inputecho cat
files/input_echo.log`.
