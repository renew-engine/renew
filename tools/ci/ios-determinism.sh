#!/usr/bin/env bash
# The iOS simulator determinism lane's body.
#
# A file rather than lines in the workflow, for the same reason the
# Android one is: a body that needs a shell with a memory should own its
# shell. This lane runs as a normal `run:` step rather than inside an
# action, so the constraint is weaker here - but two lanes doing the same
# job in two shapes is a reader's tax.
set -euo pipefail

# **Pick a simulator, and say so in this script's own words when there
# is none.** A traceback out of the selector would name a Python line
# instead of the thing that went wrong, on the lane least likely to have
# anyone watching.
device_json="$(xcrun simctl list devices available --json)"
selection="$(printf '%s' "$device_json" | python3 -c "
import json, sys

try:
    catalogue = json.load(sys.stdin)['devices']
except (ValueError, KeyError) as problem:
    raise SystemExit(f'simctl device list was not the shape expected: {problem}')

# Newest runtime first. The identifiers sort as text, so iOS-9 would beat
# iOS-26 on a plain sort; splitting on the dashes and comparing numbers
# keeps the newest one winning when an image carries several. Which
# runtime ran is printed below, because a leg records the platform and
# not the OS version, and this is the only place that ever says it.
def version(runtime):
    tail = runtime.rsplit('.', 1)[-1]
    parts = [p for p in tail.split('-') if p.isdigit()]
    return [int(p) for p in parts] or [0]

best = None
for runtime, devices in catalogue.items():
    if 'iOS' not in runtime:
        continue
    for device in devices:
        if not device.get('isAvailable'):
            continue
        if 'iPhone' not in device.get('name', ''):
            continue
        if best is None or version(runtime) > version(best[0]):
            best = (runtime, device.get('udid'), device.get('name'))

if best is None:
    raise SystemExit('no available iOS iPhone simulator on this runner')

runtime, udid, name = best
if not udid:
    raise SystemExit(f'the chosen simulator ({name}) has no udid')
print(f'{udid}\t{name}\t{runtime}')
")"

device="$(printf '%s' "$selection" | cut -f1)"
echo "simulator: $(printf '%s' "$selection" | cut -f2) on $(printf '%s' "$selection" | cut -f3)"

echo "booting $device"
xcrun simctl boot "$device" || true
xcrun simctl bootstatus "$device" -b

# **Neither probe below spawns a system tool, and the reason is a fact
# worth keeping.** The obvious check - ask the simulator its
# architecture with `uname` - cannot work. `simctl spawn` execs a path
# and does not search a PATH, so a bare `uname` is ENOENT; and the full
# path `/usr/bin/uname` resolves to the *host's* macOS binary, which
# aborts inside the simulator because the runtime supplies an
# iOS-simulator `libSystem` and dyld refuses it:
#
#     Library not loaded: /usr/lib/libSystem.B.dylib
#       Referenced from: /usr/bin/uname
#       Reason: ... incompatible platform (have 'iOS-simulator', need 'macOS')
#
# **That refusal is the guarantee this lane actually wants.** A binary
# whose platform does not match the simulator's does not run at all - it
# dies in the loader before `main`. So a pinned run that produces digests
# is proof that the simulator matched the platform its binary was built
# for, which is stronger than any string a probe could print. What is
# left to check is the other half: that the binaries were built for the
# platform the leg claims.

chmod +x tools/ios-sim-runner.sh
export RENEW_IOS_SIM_UDID="$device"
export CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUNNER="$PWD/tools/ios-sim-runner.sh"

# **The runner's central claim, exercised rather than asserted.**
# The runner carries no status file because `simctl spawn` returns the
# child's own exit status - unlike `adb shell`, which returns its
# shell's and forces the Android runner to carry the code back through a
# file. If that were wrong, a crashing simulation would look like a run
# that succeeded and printed nothing, and the lane would report a
# missing digest rather than a failure. Eleven pinned runs that all exit
# zero can never notice, so a failing one is run on purpose.
#
# `chess` answers a bad flag with exit 1 (its `run_cli` returns 1 on a
# parse error), which is the only non-zero code any pinned sample can be
# made to produce.
#
# **What this shows, exactly:** a program that fails on the simulator
# arrives here as a failure rather than as a success that printed
# nothing. **What it does not show:** that an arbitrary code is carried
# through unchanged, because 1 is also the status a wrapper would invent
# if it reported failures in its own words. Distinguishing those needs a
# program that can choose its exit code, and none of the pinned samples
# can. The weaker claim is the one worth having anyway: the failure this
# lane must never make is calling a crashed run a silent one.
set +e
propagation="$(cargo run --quiet --package renew-sample-chess \
    --target aarch64-apple-ios-sim -- --renew-not-a-real-flag 2>&1)"
propagated=$?
set -e
if [ "$propagated" -eq 101 ]; then
    echo "cargo could not build or start the check, so nothing was learned about \
exit codes: $propagation" >&2
    exit 1
fi
if [ "$propagated" -ne 1 ]; then
    echo "a simulator program that exits 1 reached this script as $propagated, so \
the runner cannot carry a program's own failure back and this lane cannot tell a \
crash from a silent run: $propagation" >&2
    exit 1
fi
echo "a failing program on the simulator arrives here as a failure"

# The other half, and its limits stated rather than dressed up: the
# binary in the triple's output directory really is an arm64 Mach-O.
# This is **not** independent of the triple - rustc names that directory
# after the triple and cannot emit a foreign architecture into it - so it
# does not audit the triple. What it does catch is a stale or hand-placed
# file surviving in the tree, and it makes the architecture a printed
# fact rather than an inference for anyone reading the log.
built="target/aarch64-apple-ios-sim/debug/chess"
built_arch="$(lipo -archs "$built")"
echo "the binary the simulator ran is: $built_arch"
if [ "$built_arch" != "arm64" ]; then
    echo "built '$built_arch', but the leg claims aarch64" >&2
    exit 1
fi

cargo run --package renew-cli --bin renew -- \
    determinism --emit ios-leg.json --target aarch64-apple-ios-sim

cat ios-leg.json
