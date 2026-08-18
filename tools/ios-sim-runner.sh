#!/usr/bin/env bash
# Run one binary on a booted iOS simulator.
#
# Cargo's `CARGO_TARGET_<TRIPLE>_RUNNER` names this script, so
# `cargo run --target aarch64-apple-ios-sim` executes there instead of
# here — the same mechanism the Android runner uses, and the reason the
# determinism lane can produce a simulator leg through exactly the code
# path that produces a desktop one.
#
# Contract, identical in obligation to the Android runner's because the
# caller is the same and cannot tell the two apart:
#
#   * a spawn failure exits non-zero and `simctl`'s own message goes to
#     this script's stderr, so the step fails as plumbing rather than as
#     disagreement. Whether a caller shows that message is the caller's
#     business, and the one caller there is does not: `renew determinism
#     --emit` captures stdout and prints its own summary, so simctl's
#     reason for failing reaches the job log and not the error. The same
#     caveat is on the Android runner, for the same reason;
#   * the program's stdout is this script's stdout, and nothing else is
#     written there, because the caller parses its last JSON line;
#   * the program's exit code is this script's exit code.
#
# **Simpler than the Android runner, and the difference is real rather
# than stylistic.** There is no push: the simulator shares this
# machine's filesystem, so the binary is executed where it was built.
# And `simctl spawn` reports the child's own exit status, so none of the
# status-file machinery the `adb shell` contract forces is needed here.
# Neither is an accident of effort — they are properties of the two
# platforms' tooling, and pretending they were the same would mean
# carrying Android's workarounds where nothing requires them.
set -euo pipefail

binary="$1"
shift

if [ ! -f "$binary" ]; then
    echo "ios-sim-runner: no such binary: $binary" >&2
    exit 127
fi

# **The caller names the device, and `booted` is only the fallback.**
# `booted` resolves to whichever simulator is running, which is exactly
# one machine right up until it is two - a second device left booted by
# another job, or an iPad the lane did not choose - and then it resolves
# to an undocumented one of them. A lane that boots a specific device and
# then measures an unspecified one would put a digest against the wrong
# platform, which is the single thing this lane exists to get right.
#
# So the device is read from the environment, where the script that
# actually boots it can put it. The fallback keeps this runner usable by
# hand, where "the one I have running" is what a person means.
device="${RENEW_IOS_SIM_UDID:-booted}"

# Arguments are passed as separate argv entries, not through a shell, so
# nothing here needs quoting: `simctl spawn` execs the binary directly.
# That is worth stating because the Android runner next door must quote
# every argument, and a reader moving between them should know the
# difference is in the tools, not in the care taken.
exec xcrun simctl spawn "$device" "$binary" "$@"
