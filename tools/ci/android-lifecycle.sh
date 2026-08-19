#!/usr/bin/env bash
# The Android emulator lifecycle lane's body.
#
# **A file, not lines in the workflow.** The emulator action runs its
# `script:` input one line per `sh -c`: variables do not survive between
# lines, `set -eu` applies to nothing after itself, and any multi-line
# construct is a syntax error at the first line break. The determinism
# lane lost two CI runs learning that; this one inherits the lesson
# rather than repeating it.
set -euo pipefail

serial="$(adb devices | awk '$2 == "device" { print $1; exit }')"
[ -n "$serial" ] || {
    echo "the emulator action reported ready, but no authorised device is attached" >&2
    exit 1
}

# **The ABI, read with `getprop`, not the kernel arch from `uname`.** The
# APK this lane installs carries an x86_64 shared object and nothing
# else, and what decides whether that library loads is the ABI: a 64-bit
# kernel running a 32-bit system image reports `x86_64` from `uname -m`
# while its ABI is `x86`, so the weaker probe passes and the app then
# fails to start for a reason nothing here explains. The desk tool has
# always asked the right question; asking a different one in CI is how
# the two drift.
device_abi="$(adb -s "$serial" shell getprop ro.product.cpu.abi | tr -d '\r')"
echo "emulator reports ABI: $device_abi"
if [ "$device_abi" != "x86_64" ]; then
    echo "the emulator's ABI is '$device_abi'; this lane built an x86_64 library" >&2
    exit 1
fi

# **Two cycles, not the desk tool's three.** Each is seven seconds of
# sleeping, and the property under test — that backgrounding revokes the
# surface and returning grants a new one — is not more true at three
# than at two. What a third would buy is a longer lane on a runner that
# has already booted a virtual machine to get here.
CYCLES=2 exec bash tools/android-lifecycle-core.sh "$serial" \
    samples/input_echo/android/app/build/outputs/apk/debug/app-debug.apk emulator
