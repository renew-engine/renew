#!/usr/bin/env bash
# The Android emulator determinism lane's body.
#
# **A file, not lines in the workflow, and that is load-bearing.** The
# emulator action runs its `script:` input one line per `sh -c`: variables
# do not survive from one line to the next, `set -eu` applies to nothing
# after itself, and any multi-line construct is a syntax error at the
# first line break. The lane's first two CI runs died on exactly that —
# once on `pipefail`, which dash does not have, and once on an `if` whose
# `fi` arrived in a different shell than its `if`. Worse, it would have
# taken the two `export` lines below with it: they would have been set in
# a shell that exited before cargo ran, and the runner would never have
# been consulted.
#
# So the workflow calls this in one line, and everything that needs a
# shell with a memory happens here.
set -euo pipefail

# **Asserted, not printed.** The leg's identity comes from the target
# triple, so the way it could become a lie is an attached device whose
# architecture is not the one the triple names. This is the line that
# catches that, and on an advisory lane it has to fail the step rather
# than scroll past in a log nobody opens.
#
# It is NOT a guard against the runner having executed on the host: the
# runner is x86_64 too, so that comparison would pass either way. What
# rules the host out is the binary itself. An x86_64-linux-android ELF
# names `/system/bin/linker64` as its interpreter — read out of one of
# these binaries to check, rather than assumed — and no Linux runner has
# that path, so the kernel refuses it before a single instruction runs.
# Digests that came back at all came back from a device.
device_arch="$(adb shell uname -m | tr -d '\r')"
echo "device reports: $device_arch"
if [ "$device_arch" != "x86_64" ]; then
    echo "the attached device is '$device_arch'; the leg would claim x86_64 \
from its triple" >&2
    exit 1
fi

chmod +x tools/android-runner.sh

# Cargo hands each built binary to the runner named here, which pushes it
# to the device and executes it there. This is the whole mechanism: with
# the variable unset, cargo would try to exec an Android binary on this
# runner, and it would fail rather than quietly measure the host.
export CARGO_TARGET_X86_64_LINUX_ANDROID_RUNNER="$PWD/tools/android-runner.sh"
export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$ANDROID_NDK_LATEST_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/x86_64-linux-android29-clang"

cargo run --package renew-cli --bin renew -- \
    determinism --emit android-leg.json --target x86_64-linux-android

cat android-leg.json
